//! Bounded scheduled pulls for configured NRTMv3 peers.

use std::{
    io::{BufReader, Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::watch;

use crate::{config::MirrorConfig, nrtm_client, nrtm3::Reader, storage::Store};

const MAX_STAGED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn run(
    store: Arc<Store>,
    source: String,
    config: MirrorConfig,
    max_bytes: u64,
    timeout: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    ensure!(
        (1..=86400).contains(&config.interval_seconds),
        "invalid NRTMv3 mirror interval"
    );
    let interval = Duration::from_secs(config.interval_seconds);
    let max_bytes = max_bytes.min(MAX_STAGED_BYTES);
    let timeout = timeout.min(MAX_FETCH_TIMEOUT);
    ensure!(
        max_bytes > 0 && !timeout.is_zero(),
        "invalid NRTMv3 mirror limits"
    );

    while !stopped(&shutdown) {
        let attempt = {
            let store = store.clone();
            let source = source.clone();
            let shutdown = shutdown.clone();
            tokio::task::spawn_blocking(move || {
                sync_once(
                    &store,
                    &source,
                    config.endpoint,
                    config.base_serial,
                    max_bytes,
                    timeout,
                    &shutdown,
                )
            })
        };
        // Once an apply starts, wait for its commit or rollback. Before then
        // the blocking fetch has an absolute deadline and checks shutdown.
        let result = attempt.await.context("scheduled NRTMv3 task failed")?;
        if stopped(&shutdown) {
            break;
        }
        match result {
            Ok(Some(serial)) => tracing::info!(%source, serial, "scheduled NRTMv3 mirror advanced"),
            Ok(None) => {}
            Err(_) => tracing::warn!(%source, "scheduled NRTMv3 mirror failed; will retry"),
        }
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stopped| *stopped) => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    Ok(())
}

fn sync_once(
    store: &Store,
    source: &str,
    endpoint: SocketAddr,
    base_serial: Option<u64>,
    max_bytes: u64,
    timeout: Duration,
    shutdown: &watch::Receiver<bool>,
) -> Result<Option<u64>> {
    if stopped(shutdown) {
        return Ok(None);
    }
    ensure!(
        store.nrtm4_state(source)?.is_none(),
        "source has committed NRTMv4 state"
    );
    let (base, initial_base) = match store.source_serial(source)? {
        Some(serial) => (serial, None),
        None => {
            let serial = base_serial.context("initial NRTMv3 mirror requires snapshot serial")?;
            (serial, Some(serial))
        }
    };
    let start = base.checked_add(1).context("NRTMv3 serial exhausted")?;
    let Some(mut response) = nrtm_client::fetch(endpoint, source, start, timeout, max_bytes)?
    else {
        return Ok(None);
    };
    let mut stage = tempfile::tempfile().context("creating private NRTMv3 staging file")?;
    let mut remaining = max_bytes;
    let mut buffer = [0; 16 * 1024];
    loop {
        if stopped(shutdown) {
            return Ok(None);
        }
        if remaining == 0 {
            ensure!(
                response.read(&mut buffer[..1])? == 0,
                "NRTMv3 mirror response exceeds staging byte limit"
            );
            break;
        }
        let capacity = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let bytes = response
            .read(&mut buffer[..capacity])
            .context("reading NRTMv3 mirror response")?;
        if bytes == 0 {
            break;
        }
        stage
            .write_all(&buffer[..bytes])
            .context("staging NRTMv3 mirror response")?;
        remaining -= bytes as u64;
    }

    if stopped(shutdown) {
        return Ok(None);
    }
    stage.seek(SeekFrom::Start(0))?;
    let transcript =
        Reader::new(BufReader::new(stage), source, max_bytes).map_err(anyhow::Error::msg)?;
    let range = transcript.range();
    ensure!(
        range.0 == start,
        "NRTMv3 mirror response starts at an unexpected serial"
    );
    store
        .apply_nrtm3_if(source, range, initial_base, transcript, || {
            !stopped(shutdown)
        })
        .map(Some)
}

fn stopped(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, BufRead},
        net::{TcpListener, TcpStream},
        thread,
        time::Instant,
    };

    use super::*;

    const VALID: &[u8] =
        b"%START Version: 3 TEST 10-10\n\nADD 10\n\naut-num: AS64497\nsource: TEST\n\n%END TEST\n";
    const MALFORMED: &[u8] =
        b"%START Version: 3 TEST 10-10\n\nADD 10\n\naut-num: AS64498\nsource: TEST\n\n%END OTHER\n";

    fn registry(path: &std::path::Path) -> Result<Arc<Store>> {
        let store = Store::open(&path.join("registry.redb"))?;
        let object =
            crate::rpsl::parse("aut-num: AS64496\nsource: TEST\n").map_err(anyhow::Error::msg)?;
        store.replace_source("TEST", [Ok(object)])?;
        Ok(Arc::new(store))
    }

    fn listener() -> io::Result<TcpListener> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    fn request(listener: &TcpListener, expected: &str) -> Result<TcpStream> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ensure!(
                        Instant::now() < deadline,
                        "test peer timed out accepting a request"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line)?;
        assert_eq!(line, expected);
        Ok(stream.into_inner())
    }

    #[tokio::test]
    async fn worker_retries_failed_batch_and_uses_the_committed_cursor() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = registry(dir.path())?;
        let listener = listener()?;
        let endpoint = listener.local_addr()?;
        let (observed, completed) = tokio::sync::oneshot::channel();
        let peer = thread::spawn(move || -> Result<()> {
            request(&listener, "-g TEST:3:10-LAST\n")?.write_all(MALFORMED)?;
            request(&listener, "-g TEST:3:10-LAST\n")?.write_all(VALID)?;
            request(&listener, "-g TEST:3:11-LAST\n")?
                .write_all(b"% Warning: there are no newer updates available\n")?;
            let _ = observed.send(());
            Ok(())
        });
        let (stop, shutdown) = watch::channel(false);
        let worker = tokio::spawn(run(
            store.clone(),
            String::from("TEST"),
            MirrorConfig {
                endpoint,
                interval_seconds: 1,
                base_serial: Some(9),
            },
            4096,
            Duration::from_secs(3),
            shutdown,
        ));
        tokio::time::timeout(Duration::from_secs(8), completed).await??;
        stop.send(true)?;
        tokio::time::timeout(Duration::from_secs(5), worker).await???;
        peer.join().expect("peer thread")?;
        let sources = [String::from("TEST")];
        assert_eq!(store.source_serial("TEST")?, Some(10));
        assert!(store.exact("AS64498", &sources, 1)?.is_empty());
        assert_eq!(store.exact("AS64497", &sources, 1)?.len(), 1);
        assert_eq!(
            store
                .nrtm3_journal("TEST", 10, None, 1, 4096)?
                .unwrap()
                .1
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn oversized_response_never_starts_an_apply() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = registry(dir.path())?;
        let listener = listener()?;
        let endpoint = listener.local_addr()?;
        let peer = thread::spawn(move || -> Result<()> {
            let mut response = VALID.to_vec();
            response.extend([b'\n'; 256]);
            request(&listener, "-g TEST:3:10-LAST\n")?.write_all(&response)?;
            Ok(())
        });
        let (_stop, shutdown) = watch::channel(false);
        let error = sync_once(
            &store,
            "TEST",
            endpoint,
            Some(9),
            128,
            Duration::from_secs(3),
            &shutdown,
        )
        .unwrap_err();
        peer.join().expect("peer thread")?;
        assert!(error.to_string().contains("staging byte limit"));
        assert_eq!(store.source_serial("TEST")?, None);
        assert_eq!(store.verify()?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_prevents_a_staged_apply_waiting_for_queries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = registry(dir.path())?;
        let view = store.query_gate().read().await;
        let listener = listener()?;
        let endpoint = listener.local_addr()?;
        let peer = thread::spawn(move || -> Result<()> {
            request(&listener, "-g TEST:3:10-LAST\n")?.write_all(VALID)?;
            Ok(())
        });
        let (stop, shutdown) = watch::channel(false);
        let worker = tokio::spawn(run(
            store.clone(),
            String::from("TEST"),
            MirrorConfig {
                endpoint,
                interval_seconds: 1,
                base_serial: Some(9),
            },
            4096,
            Duration::from_secs(3),
            shutdown,
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            // The fair lock refuses another reader once the writer is queued.
            while store.query_gate().try_read().is_ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        stop.send(true)?;
        drop(view);
        tokio::time::timeout(Duration::from_secs(5), worker).await???;
        peer.join().expect("peer thread")?;
        assert_eq!(store.source_serial("TEST")?, None);
        assert_eq!(store.verify()?, 1);
        Ok(())
    }
}
