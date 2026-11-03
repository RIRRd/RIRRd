//! Bounded HTTPS retrieval from an operator-selected NRTMv4 origin.

use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tempfile::{NamedTempFile, TempPath};
use ureq::{
    Agent, Body,
    http::{Response, StatusCode, Uri, header},
    tls::{RootCerts, TlsConfig, TlsProvider},
};

use crate::nrtm4::{FileReference, MAX_NOTIFICATION_BYTES, valid_relative_path};

const MAX_ENDPOINT_BYTES: usize = 4096;
const MAX_HEADER_BYTES: usize = 16 * 1024;

pub struct Fetcher {
    agent: Agent,
    notification_uri: Uri,
    per_request_timeout: Duration,
    deadline: Instant,
}

impl Fetcher {
    pub fn new(
        notification_url: &str,
        per_request_timeout: Duration,
        overall_timeout: Duration,
    ) -> Result<Self> {
        ensure!(
            !per_request_timeout.is_zero() && !overall_timeout.is_zero(),
            "NRTMv4 fetch timeouts must be positive"
        );
        let notification_uri = parse_notification_uri(notification_url)?;
        let deadline = Instant::now()
            .checked_add(overall_timeout)
            .context("NRTMv4 fetch deadline is out of range")?;
        let agent = Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .max_response_header_size(MAX_HEADER_BYTES)
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::Rustls)
                    .root_certs(RootCerts::WebPki)
                    .build(),
            )
            .build()
            .into();
        Ok(Self {
            agent,
            notification_uri,
            per_request_timeout,
            deadline,
        })
    }

    pub fn notification(&self) -> Result<Vec<u8>> {
        let limit = MAX_NOTIFICATION_BYTES as u64;
        let mut response = self.get(&self.notification_uri, limit)?;
        let mut bytes = Vec::new();
        copy_bounded(
            response.body_mut().as_reader(),
            &mut bytes,
            limit,
            self.deadline,
        )
        .context("reading NRTMv4 notification")?;
        Ok(bytes)
    }

    /// Stage the exact HTTP response bytes for downstream signed-hash verification.
    pub fn file(&self, reference: &FileReference, remaining: u64) -> Result<TempPath> {
        ensure!(remaining > 0, "NRTMv4 compressed byte budget exhausted");
        let uri = reference_uri(&self.notification_uri, &reference.url)?;
        let mut response = self.get(&uri, remaining)?;
        let mut stage = NamedTempFile::new().context("creating NRTMv4 download staging file")?;
        copy_bounded(
            response.body_mut().as_reader(),
            stage.as_file_mut(),
            remaining,
            self.deadline,
        )
        .context("staging NRTMv4 referenced file")?;
        stage.as_file_mut().flush()?;
        require_time(self.deadline)?;
        Ok(stage.into_temp_path())
    }

    fn get(&self, uri: &Uri, limit: u64) -> Result<Response<Body>> {
        let timeout = require_time(self.deadline)?.min(self.per_request_timeout);
        let response = self
            .agent
            .get(uri.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .config()
            .timeout_global(Some(timeout))
            .timeout_resolve(Some(timeout))
            .build()
            .call()
            .context("NRTMv4 HTTPS request failed")?;
        ensure!(
            response.status() == StatusCode::OK,
            "unexpected NRTMv4 HTTPS status {}",
            response.status()
        );
        ensure!(
            response
                .headers()
                .get_all(header::CONTENT_ENCODING)
                .iter()
                .all(|encoding| encoding.as_bytes().eq_ignore_ascii_case(b"identity")),
            "NRTMv4 response must use identity HTTP content encoding"
        );
        ensure!(
            response
                .body()
                .content_length()
                .is_none_or(|length| length <= limit),
            "NRTMv4 response exceeds byte limit"
        );
        Ok(response)
    }
}

fn parse_notification_uri(url: &str) -> Result<Uri> {
    ensure!(
        url.len() <= MAX_ENDPOINT_BYTES
            && !url.bytes().any(|byte| byte.is_ascii_whitespace()
                || byte.is_ascii_control()
                || matches!(byte, b'#' | b'\\' | b'?')),
        "invalid NRTMv4 notification URL"
    );
    let uri: Uri = url.parse().context("invalid NRTMv4 notification URL")?;
    ensure!(
        uri.scheme_str() == Some("https"),
        "NRTMv4 notification URL must use HTTPS"
    );
    let authority = uri
        .authority()
        .context("missing NRTMv4 notification host")?;
    ensure!(
        !authority.as_str().contains('@'),
        "NRTMv4 notification URL cannot contain credentials"
    );
    let host = authority.host();
    ensure!(
        !host.is_empty() && !host.contains('%'),
        "invalid NRTMv4 notification host"
    );
    if authority.as_str() != host {
        ensure!(
            authority.port_u16().is_some_and(|port| port > 0),
            "invalid NRTMv4 notification port"
        );
    }
    Ok(uri)
}

fn reference_uri(notification: &Uri, relative: &str) -> Result<Uri> {
    ensure!(
        valid_relative_path(relative),
        "unsafe NRTMv4 file reference URL"
    );
    let directory = notification
        .path()
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    Uri::builder()
        .scheme("https")
        .authority(
            notification
                .authority()
                .context("missing NRTMv4 notification host")?
                .clone(),
        )
        .path_and_query(format!("{directory}/{relative}"))
        .build()
        .context("invalid resolved NRTMv4 reference URL")
}

fn require_time(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(
        !remaining.is_zero(),
        "NRTMv4 overall fetch deadline exceeded"
    );
    Ok(remaining)
}

fn copy_bounded(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
    deadline: Instant,
) -> Result<u64> {
    let mut buffer = [0; 64 * 1024];
    let mut written = 0;
    while written < limit {
        require_time(deadline)?;
        let capacity = (limit - written).min(buffer.len() as u64) as usize;
        let read = input.read(&mut buffer[..capacity])?;
        require_time(deadline)?;
        if read == 0 {
            return Ok(written);
        }
        output.write_all(&buffer[..read])?;
        written += read as u64;
    }
    require_time(deadline)?;
    ensure!(
        input.read(&mut buffer[..1])? == 0,
        "NRTMv4 response exceeds byte limit"
    );
    require_time(deadline)?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_stay_under_the_configured_https_origin_and_directory() -> Result<()> {
        for (endpoint, expected) in [
            (
                "https://mirror.example:8443/irr/notification.jws",
                "https://mirror.example:8443/irr/session/delta-5.json.gz",
            ),
            (
                "https://[2001:db8::1]:8443/notification.jws",
                "https://[2001:db8::1]:8443/session/delta-5.json.gz",
            ),
            (
                "https://mirror.example",
                "https://mirror.example/session/delta-5.json.gz",
            ),
        ] {
            let notification = parse_notification_uri(endpoint)?;
            let resolved = reference_uri(&notification, "session/delta-5.json.gz")?;
            assert_eq!(resolved.authority(), notification.authority());
            assert_eq!(resolved.to_string(), expected);
            for invalid in [
                "",
                "/snapshot",
                "//other.example/file",
                "https://other.example/file",
                "../snapshot",
                "./snapshot",
                "session/../snapshot",
                "session//snapshot",
                "session/%2e%2e/snapshot",
                "session\\snapshot",
                "snapshot?version=1",
                "snapshot#hash",
            ] {
                assert!(
                    reference_uri(&notification, invalid).is_err(),
                    "accepted {invalid}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn configured_notification_rejects_unsafe_uri_components() {
        for invalid in [
            "http://mirror.example/notification",
            "//mirror.example/notification",
            "https://user:password@mirror.example/notification",
            "https://user@/notification",
            "https://mirror.example/notification?key=secret",
            "https://mirror.example/notification#fragment",
            "https://mirror.example:0/notification",
            "https://mirror.example:65536/notification",
            "https://mirror.example:no-port/notification",
            "https://mirror.example\\attacker/notification",
            "https://mirror.example/notification\n",
            "https://:443/notification",
        ] {
            assert!(
                parse_notification_uri(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn copy_rejects_overflow_and_expired_deadlines_before_more_bytes_are_written() -> Result<()> {
        let future = Instant::now() + Duration::from_secs(10);
        let mut bytes = Vec::new();
        assert_eq!(copy_bounded(&b"abc"[..], &mut bytes, 3, future)?, 3);
        assert_eq!(bytes, b"abc");
        bytes.clear();
        let error = copy_bounded(&b"abcd"[..], &mut bytes, 3, future).unwrap_err();
        assert!(error.to_string().contains("exceeds byte limit"));
        assert_eq!(bytes, b"abc");
        bytes.clear();
        assert_eq!(copy_bounded(&b"ab"[..], &mut bytes, 3, future)?, 2);
        assert_eq!(bytes, b"ab");
        bytes.clear();
        assert!(copy_bounded(&b"a"[..], &mut bytes, 1, Instant::now()).is_err());
        assert!(bytes.is_empty());
        Ok(())
    }

    #[test]
    fn fetcher_pins_https_and_disables_proxies_and_redirects() -> Result<()> {
        let fetcher = Fetcher::new(
            "https://mirror.example/notification.jws",
            Duration::from_secs(1),
            Duration::from_secs(2),
        )?;
        let config = fetcher.agent.config();
        assert!(config.https_only());
        assert!(config.proxy().is_none());
        assert_eq!(config.max_redirects(), 0);
        assert_eq!(config.max_response_header_size(), MAX_HEADER_BYTES);
        assert!(matches!(
            config.tls_config().provider(),
            TlsProvider::Rustls
        ));
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::WebPki
        ));
        assert!(!config.tls_config().disable_verification());
        Ok(())
    }
}
