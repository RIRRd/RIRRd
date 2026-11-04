//! One-shot NRTMv3 client transport for an explicitly configured peer.

use std::{
    io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};

const MAX_PREAMBLE_BYTES: u64 = 8 * 1024;
const NO_UPDATES: &[u8] = b"% Warning: there are no newer updates available";
const NO_UPDATES_LEGACY: &[u8] = b"% Warning: there are no updates available";
const NO_UPDATES_NUMBERED: &[u8] = b"% Warning (1): there are no newer updates available";

/// Finish the bounded network transfer before taking a database write lock.
/// The transport deadline must not expire while a valid batch is being indexed.
pub fn fetch_staged(
    endpoint: SocketAddr,
    source: &str,
    start: u64,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Option<BufReader<std::fs::File>>> {
    fetch_staged_until(endpoint, source, start, None, timeout, max_bytes)
}

/// A finite requested range keeps bootstrap batches within the normal byte cap.
pub fn fetch_staged_until(
    endpoint: SocketAddr,
    source: &str,
    start: u64,
    end: Option<u64>,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Option<BufReader<std::fs::File>>> {
    let max_bytes = max_bytes.min(64 * 1024 * 1024);
    let Some(response) = fetch_until(endpoint, source, start, end, timeout, max_bytes)? else {
        return Ok(None);
    };
    let mut stage = tempfile::tempfile().context("creating private NRTMv3 staging file")?;
    let bytes = io::copy(&mut response.take(max_bytes + 1), &mut stage)
        .context("staging complete NRTMv3 response")?;
    ensure!(
        bytes <= max_bytes,
        "NRTMv3 response exceeds staging byte limit"
    );
    stage.seek(SeekFrom::Start(0))?;
    Ok(Some(BufReader::new(stage)))
}

/// Fetch the finite stream beginning at `start` from the exact TCP endpoint.
/// Returns `None` for a complete, recognized no-updates response. The transport
/// has no peer authentication or encryption. The caller must independently
/// enforce its byte limit when parsing the returned stream.
pub fn fetch(
    endpoint: SocketAddr,
    source: &str,
    start: u64,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Option<impl BufRead>> {
    fetch_until(endpoint, source, start, None, timeout, max_bytes)
}

fn fetch_until(
    endpoint: SocketAddr,
    source: &str,
    start: u64,
    end: Option<u64>,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Option<impl BufRead>> {
    ensure!(
        !endpoint.ip().is_unspecified() && endpoint.port() != 0,
        "NRTMv3 endpoint must specify a peer IP address and port"
    );
    ensure!(start != 0, "NRTMv3 start serial must be positive");
    ensure!(
        end.is_none_or(|end| end >= start),
        "NRTMv3 end serial precedes start serial"
    );
    ensure!(
        source.len() <= 32 && crate::rpsl::generic_name(source),
        "invalid NRTMv3 source name"
    );
    ensure!(max_bytes != 0, "NRTMv3 byte limit must be positive");
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("NRTMv3 timeout is out of range")?;
    let socket = TcpStream::connect_timeout(&endpoint, remaining(deadline)?)
        .with_context(|| format!("connecting to NRTMv3 peer {endpoint}"))?;
    let end = end.map_or_else(|| "LAST".to_owned(), |value| value.to_string());
    let query = format!("-g {}:3:{start}-{end}\n", source.to_ascii_uppercase());
    write_query(&socket, query.as_bytes(), deadline).context("sending NRTMv3 request")?;

    let mut reader = TimedReader {
        inner: BufReader::new(socket),
        deadline,
    };
    let limit = max_bytes.min(MAX_PREAMBLE_BYTES);
    let mut prefix = Vec::new();
    let no_updates = loop {
        let start = prefix.len();
        ensure!(
            read_preamble_line(&mut reader, &mut prefix, limit)?,
            "NRTMv3 peer returned an empty response"
        );
        match complete_line(&prefix[start..]) {
            Some(b"") => continue,
            Some(line)
                if line.eq_ignore_ascii_case(NO_UPDATES)
                    || line.eq_ignore_ascii_case(NO_UPDATES_NUMBERED)
                    || line.eq_ignore_ascii_case(NO_UPDATES_LEGACY) =>
            {
                break true;
            }
            Some(line) if preamble_comment(line) => continue,
            _ => break false,
        }
    };

    if no_updates {
        loop {
            let start = prefix.len();
            if !read_preamble_line(&mut reader, &mut prefix, limit)? {
                return Ok(None);
            }
            ensure!(
                complete_line(&prefix[start..]) == Some(b""),
                "unexpected data after NRTMv3 no-updates warning"
            );
        }
    }

    Ok(Some(Cursor::new(prefix).chain(reader)))
}

fn preamble_comment(line: &[u8]) -> bool {
    if line.starts_with(b"#") {
        return true;
    }
    let Some(body) = line.strip_prefix(b"%") else {
        return false;
    };
    let mut body = body;
    while let Some(rest) = body.strip_prefix(b"%") {
        body = rest;
    }
    let body = body.trim_ascii_start();
    let keyword = body
        .split(|byte| byte.is_ascii_whitespace() || *byte == b':')
        .next()
        .unwrap_or_default();
    ![b"START".as_slice(), b"END", b"ERROR"]
        .iter()
        .any(|directive| keyword.eq_ignore_ascii_case(directive))
}

fn complete_line(line: &[u8]) -> Option<&[u8]> {
    line.strip_suffix(b"\n")
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

fn read_preamble_line(reader: &mut impl BufRead, bytes: &mut Vec<u8>, limit: u64) -> Result<bool> {
    let count = reader
        .take(limit - bytes.len() as u64 + 1)
        .read_until(b'\n', bytes)
        .context("reading NRTMv3 response preamble")?;
    ensure!(
        bytes.len() as u64 <= limit,
        "NRTMv3 response preamble exceeds byte limit"
    );
    Ok(count != 0)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "NRTMv3 transport deadline exceeded",
        ))
    } else {
        Ok(remaining)
    }
}

fn write_query(mut socket: &TcpStream, mut query: &[u8], deadline: Instant) -> io::Result<()> {
    while !query.is_empty() {
        socket.set_write_timeout(Some(remaining(deadline)?))?;
        match socket.write(query) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => query = &query[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    remaining(deadline).map(|_| ())
}

struct TimedReader {
    inner: BufReader<TcpStream>,
    deadline: Instant,
}

impl Read for TimedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let timeout = remaining(self.deadline)?;
        if self.inner.buffer().is_empty() {
            self.inner.get_ref().set_read_timeout(Some(timeout))?;
        }
        let count = self.inner.read(buffer)?;
        remaining(self.deadline)?;
        Ok(count)
    }
}

impl BufRead for TimedReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let timeout = remaining(self.deadline)?;
        if self.inner.buffer().is_empty() {
            self.inner.get_ref().set_read_timeout(Some(timeout))?;
        }
        let bytes = self.inner.fill_buf()?;
        remaining(self.deadline)?;
        Ok(bytes)
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
    }
}

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, thread};

    use super::*;

    fn peer(
        write_response: impl FnOnce(TcpStream) -> io::Result<()> + Send + 'static,
    ) -> io::Result<(SocketAddr, thread::JoinHandle<io::Result<()>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let mut stream = BufReader::new(stream);
            let mut request = String::new();
            stream.read_line(&mut request)?;
            assert_eq!(request, "-g TEST:3:10-LAST\n");
            write_response(stream.into_inner())
        });
        Ok((address, worker))
    }

    #[test]
    fn sends_upstream_wire_and_preserves_first_lines() -> Result<()> {
        let response = b"\r\n% Copyright banner\n%START Version: 3 TEST 10-10\n\n%END TEST\n";
        let (address, worker) = peer(move |mut stream| stream.write_all(response))?;
        let mut reader = fetch(address, "test", 10, Duration::from_secs(2), 4096)?
            .context("expected updates")?;
        let mut received = Vec::new();
        reader.read_to_end(&mut received)?;
        assert_eq!(received, response);
        worker.join().unwrap()?;
        Ok(())
    }

    #[test]
    fn a_server_error_cannot_be_hidden_before_a_no_update_warning() -> Result<()> {
        for prefix in ["%ERROR: unavailable", "%% ERROR: unavailable"] {
            let response = format!("{prefix}\n% Warning: there are no newer updates available\n");
            let (address, worker) = peer(move |mut stream| stream.write_all(response.as_bytes()))?;
            let received = fetch_staged(address, "TEST", 10, Duration::from_secs(2), 4096)?
                .context("server errors must not become an up-to-date result")?;
            assert!(crate::nrtm3::Reader::new(received, "TEST", 4096).is_err());
            worker.join().unwrap()?;
        }
        Ok(())
    }

    #[test]
    fn staged_transfer_keeps_its_budget_and_has_no_network_deadline_during_local_apply()
    -> Result<()> {
        let response = b"%START Version: 3 TEST 10-10\n\nADD 10\n\naut-num: AS64496\nsource: TEST\n\n%END TEST\n";
        let (address, worker) = peer(move |mut stream| stream.write_all(response))?;
        let mut staged = fetch_staged(address, "TEST", 10, Duration::from_secs(1), 4096)?
            .context("expected a staged response")?;
        worker.join().unwrap()?;
        // Indexing starts after the download deadline and must still read every byte.
        thread::sleep(Duration::from_millis(1050));
        let mut bytes = Vec::new();
        staged.read_to_end(&mut bytes)?;
        assert_eq!(bytes, response);
        let (address, worker) = peer(move |mut stream| {
            stream.write_all(response)?;
            stream.write_all(&[b' '; 256])
        })?;
        assert!(fetch_staged(address, "TEST", 10, Duration::from_secs(2), 128).is_err());
        worker.join().unwrap()?;
        Ok(())
    }

    #[test]
    fn recognizes_only_complete_bounded_no_update_replies() -> Result<()> {
        for (response, max_bytes, no_updates) in [
            (
                &b"% Warning: there are no updates available\n\n\n"[..],
                4096,
                true,
            ),
            (
                &b"% Copyright banner\n% WARNING: there are no updates available\r\n"[..],
                4096,
                true,
            ),
            (
                &b"% Warning: there are no updates available\n% ERROR\n"[..],
                4096,
                false,
            ),
            (
                &b"\n% Warning: there are no newer updates available\n\n"[..],
                4096,
                true,
            ),
            (
                &b"% Warning (1): there are no newer updates available\r\n"[..],
                4096,
                true,
            ),
            (
                &b"% Published database banner\n% See publisher documentation\n\n% WARNING: there are no newer updates available\n\n"[..],
                4096,
                true,
            ),
            (
                &b"# Database banner\n% Warning (1): there are no newer updates available\n"[..],
                4096,
                true,
            ),
            (
                &b"% Warning: there are no newer updates available\n% ERROR\n"[..],
                4096,
                false,
            ),
            (
                &b"% Warning: there are no newer updates available\n\n"[..],
                10,
                false,
            ),
            (&b"\r\n"[..], 4096, false),
            (&b"% Copyright banner only\n"[..], 4096, false),
            (&b"% Unrecognised warning\n"[..], 4096, false),
            (&b""[..], 4096, false),
        ] {
            let (address, worker) = peer(move |mut stream| stream.write_all(response))?;
            let outcome = fetch(address, "TEST", 10, Duration::from_secs(2), max_bytes);
            assert_eq!(matches!(outcome, Ok(None)), no_updates);
            if !no_updates {
                assert!(outcome.is_err());
            }
            worker.join().unwrap()?;
        }
        Ok(())
    }

    #[test]
    fn continuous_trickle_does_not_reset_deadline() -> Result<()> {
        let (address, worker) = peer(|mut stream| {
            stream.write_all(b"%START Version: 3 TEST 10-10\n")?;
            for _ in 0..30 {
                thread::sleep(Duration::from_millis(20));
                if stream.write_all(b" ").is_err() {
                    break;
                }
            }
            Ok(())
        })?;
        let mut reader = fetch(address, "TEST", 10, Duration::from_millis(250), 4096)?
            .context("expected updates")?;
        let mut response = Vec::new();
        let error = reader.read_to_end(&mut response).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        drop(reader);
        worker.join().unwrap()?;
        Ok(())
    }

    #[test]
    fn rejects_bad_input_before_connecting() -> Result<()> {
        let address: SocketAddr = "127.0.0.1:1".parse()?;
        let error = fetch(address, "TEST\n-g OTHER", 10, Duration::from_secs(2), 4096)
            .err()
            .context("invalid source unexpectedly accepted")?;
        assert_eq!(error.to_string(), "invalid NRTMv3 source name");
        assert!(fetch(address, "TEST", 10, Duration::ZERO, 4096).is_err());
        Ok(())
    }
}
