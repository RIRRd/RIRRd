use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::OwnedSemaphorePermit,
    task::JoinSet,
    time::timeout,
};

use crate::{config::Config, storage::Store};

const MAX_QUERY_BYTES: u64 = 512;
const MAX_RESULTS: usize = 16;

#[derive(Debug, PartialEq, Eq)]
enum Query {
    Exact(String),
    Object {
        class: String,
        key: String,
    },
    Origin {
        class: &'static str,
        origin: String,
    },
    Route {
        class: &'static str,
        prefix: String,
        origins_only: bool,
    },
    Version,
}

pub async fn serve(config: &Config, store: Arc<Store>) -> Result<()> {
    let listener = TcpListener::bind(config.listen).await?;
    let slots = Arc::new(tokio::sync::Semaphore::new(config.max_connections));
    let deadline = Duration::from_secs(config.io_timeout_seconds);
    let mut clients = JoinSet::new();
    let sources: Arc<[String]> = config.sources.clone().into();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    tracing::info!(address = %listener.local_addr()?, "WHOIS listening");
    loop {
        tokio::select! {
            biased;
            result = &mut shutdown => {
                result?;
                break;
            }
            Some(result) = clients.join_next(), if !clients.is_empty() => {
                if let Err(error) = result {
                    tracing::error!(%error, "WHOIS connection task failed");
                }
            }
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                if let Ok(permit) = slots.clone().try_acquire_owned() {
                    clients.spawn(handle(socket, store.clone(), sources.clone(), deadline, permit));
                }
            }
        }
    }
    drop(listener);
    while clients.join_next().await.is_some() {}
    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

async fn handle(
    mut socket: TcpStream,
    store: Arc<Store>,
    sources: Arc<[String]>,
    deadline: Duration,
    permit: OwnedSemaphorePermit,
) {
    let mut line = Vec::new();
    let mut input = BufReader::new(&mut socket).take(MAX_QUERY_BYTES + 1);
    if !matches!(timeout(deadline, input.read_until(b'\n', &mut line)).await, Ok(Ok(n)) if n > 0) {
        return;
    }
    let Some(query) = parse_query(&line) else {
        let response: &[u8] = if line.starts_with(b"!") {
            b"F Invalid or unsupported query\n"
        } else {
            b"% Invalid or unsupported query\r\n"
        };
        let _ = timeout(deadline, socket.write_all(response)).await;
        return;
    };
    let irrd = !matches!(query, Query::Exact(_));
    let lookup = tokio::task::spawn_blocking(move || {
        let result = execute(query, &store, &sources);
        (result, permit)
    });
    let (response, _permit) = match timeout(deadline, lookup).await {
        Ok(Ok((Ok(response), permit))) => (response, permit),
        Ok(Ok((Err(error), permit))) => {
            tracing::error!(%error, "WHOIS lookup failed");
            let response = if irrd {
                "F Lookup failed\n"
            } else {
                "% Lookup failed\r\n"
            };
            (response.to_owned(), permit)
        }
        _ => return,
    };
    let _ = timeout(deadline, socket.write_all(response.as_bytes())).await;
}

fn execute(query: Query, store: &Store, sources: &[String]) -> Result<String> {
    match query {
        Query::Version => Ok(frame_irrd(format!(
            "rust-irrd {}",
            env!("CARGO_PKG_VERSION")
        ))),
        Query::Object { class, key } => {
            let Some(object) = store.first(&class, &key, sources)? else {
                return Ok("D\n".to_owned());
            };
            let body = object.public_text().lines().collect::<Vec<_>>().join("\n");
            Ok(frame_irrd(body))
        }
        Query::Origin { class, origin } => {
            let prefixes = store.origin_prefixes(class, &origin, sources, 4097)?;
            if prefixes.is_empty() {
                return Ok("D\n".to_owned());
            }
            if prefixes.len() > 4096 {
                return Ok("F Too many results\n".to_owned());
            }
            Ok(frame_irrd(prefixes.join(" ")))
        }
        Query::Route {
            class,
            prefix,
            origins_only,
        } => {
            let routes = store.routes_at_prefix(class, &prefix, sources, MAX_RESULTS + 1)?;
            if routes.is_empty() {
                return Ok("D\n".to_owned());
            }
            if routes.len() > MAX_RESULTS {
                return Ok("F Too many results\n".to_owned());
            }
            let body = if origins_only {
                let origins = routes
                    .iter()
                    .map(|route| {
                        route
                            .key
                            .strip_prefix(&prefix)
                            .context("invalid route key in storage")
                    })
                    .collect::<Result<Vec<_>>>()?;
                origins.join(" ")
            } else {
                routes
                    .iter()
                    .map(|route| route.public_text().lines().collect::<Vec<_>>().join("\n"))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            Ok(frame_irrd(body))
        }
        Query::Exact(key) => {
            let objects = store.exact(&key, sources, MAX_RESULTS + 1)?;
            if objects.len() > MAX_RESULTS {
                return Ok("% Too many results\r\n".to_owned());
            }
            if objects.is_empty() {
                return Ok("% No entries found\r\n".to_owned());
            }
            let mut response = String::new();
            for object in objects {
                for line in object.public_text().lines() {
                    response.push_str(line);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
            }
            Ok(response)
        }
    }
}

fn frame_irrd(body: String) -> String {
    format!("A{}\n{body}\nC\n", body.len() + 1)
}

fn parse_query(line: &[u8]) -> Option<Query> {
    if line.len() as u64 > MAX_QUERY_BYTES {
        return None;
    }
    let line = std::str::from_utf8(line.strip_suffix(b"\n")?).ok()?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.chars().any(char::is_control) {
        return None;
    }
    let line = line.trim();
    if line == "!v" {
        return Some(Query::Version);
    }
    if let Some(origin) = line.strip_prefix("!g") {
        return Some(Query::Origin {
            class: "route",
            origin: parse_origin(origin)?,
        });
    }
    if let Some(origin) = line.strip_prefix("!6") {
        return Some(Query::Origin {
            class: "route6",
            origin: parse_origin(origin)?,
        });
    }
    if let Some(argument) = line.strip_prefix("!r") {
        let (prefix, origins_only) = argument
            .strip_suffix(",o")
            .map_or((argument, false), |prefix| (prefix, true));
        let ipv6 = prefix.contains(':');
        let prefix = if prefix.contains('/') {
            prefix.to_owned()
        } else if ipv6 {
            format!("{prefix}/128")
        } else {
            format!("{prefix}/32")
        };
        return Some(Query::Route {
            class: if ipv6 { "route6" } else { "route" },
            prefix: crate::rpsl::canonical_prefix(&prefix, ipv6)
                .ok()?
                .to_ascii_uppercase(),
            origins_only,
        });
    }
    if let Some(argument) = line.strip_prefix("!m") {
        let (class, key) = argument.split_once(',')?;
        let key = key.trim();
        if class.is_empty()
            || !class
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || key.is_empty()
        {
            return None;
        }
        let key = if matches!(class, "route" | "route6") {
            key.replace([' ', '-'], "").to_ascii_uppercase()
        } else {
            key.to_uppercase()
        };
        return Some(Query::Object {
            class: class.to_owned(),
            key,
        });
    }
    (!line.is_empty() && !line.starts_with(['!', '-'])).then(|| Query::Exact(line.to_uppercase()))
}

fn parse_origin(value: &str) -> Option<String> {
    let value = value.to_ascii_uppercase();
    let digits = value.strip_prefix("AS")?;
    if !digits
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let number = if let Some((high, low)) = digits.split_once('.') {
        let high: u16 = high.parse().ok()?;
        let low: u16 = low.parse().ok()?;
        (u32::from(high) << 16) | u32::from(low)
    } else {
        digits.parse::<u32>().ok()?
    };
    Some(format!("AS{number}"))
}

#[cfg(test)]
mod tests {
    use super::{Query, frame_irrd, parse_query};

    #[test]
    fn parses_one_bounded_line_and_normalises_legacy_route_keys() {
        assert_eq!(
            parse_query(b"as64496\r\n"),
            Some(Query::Exact("AS64496".into()))
        );
        assert_eq!(
            parse_query(b"!mroute6,::/0-as64496\n"),
            Some(Query::Object {
                class: "route6".into(),
                key: "::/0AS64496".into(),
            })
        );
        assert_eq!(parse_query(b"!v\n"), Some(Query::Version));
        assert_eq!(
            parse_query("!mperson,nóć@example.net\r\n".as_bytes()),
            Some(Query::Object {
                class: "person".into(),
                key: "NÓĆ@EXAMPLE.NET".into()
            })
        );
        assert_eq!(
            parse_query(b"!6as1.0\n"),
            Some(Query::Origin {
                class: "route6",
                origin: "AS65536".into(),
            })
        );
        assert_eq!(frame_irrd("é".into()), "A3\né\nC\n");
        for invalid in [
            b"-i origin\r\n".as_slice(),
            b"as1",
            b"\r\n",
            b"as1\0\r\n",
            b"!m,AS1\n",
        ] {
            assert_eq!(parse_query(invalid), None);
        }
        assert_eq!(parse_query(&[b'A'; 513]), None);
    }
}
