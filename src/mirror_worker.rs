//! Bounded scheduled pulls for configured NRTMv3 peers.

use std::{
    io::{BufReader, Read, Seek, SeekFrom, Write},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::watch;

use crate::{config::MirrorConfig, nrtm_client, nrtm3::Reader, storage::Store};

const MAX_STAGED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, serde::Serialize)]
pub struct Status {
    pub state: &'static str,
    pub checked_at_unix_seconds: u64,
    pub last_success_unix_seconds: Option<u64>,
    pub serial: Option<u64>,
    pub interval_seconds: u64,
    pub object_class_filter: Vec<String>,
    pub transport_authenticated: bool,
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

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

    let mut status = Status {
        state: "starting",
        checked_at_unix_seconds: timestamp(),
        last_success_unix_seconds: None,
        serial: store.source_serial(&source)?,
        interval_seconds: config.interval_seconds,
        object_class_filter: config.object_class_filter.iter().cloned().collect(),
        transport_authenticated: false,
    };
    status.object_class_filter.sort_unstable();
    store.set_mirror_status(source.clone(), status.clone());
    while !stopped(&shutdown) {
        let attempt = {
            let store = store.clone();
            let source = source.clone();
            let shutdown = shutdown.clone();
            let config = config.clone();
            tokio::task::spawn_blocking(move || {
                sync_once(&store, &source, &config, max_bytes, timeout, &shutdown)
            })
        };
        // Once an apply starts, wait for its commit or rollback. Before then
        // the blocking fetch has an absolute deadline and checks shutdown.
        let result = attempt.await.context("scheduled NRTMv3 task failed")?;
        if stopped(&shutdown) {
            break;
        }
        match result {
            Ok(serial) => {
                status.state = "following";
                status.last_success_unix_seconds = Some(timestamp());
                status.serial = serial
                    .or(store.source_serial(&source)?)
                    .or(config.base_serial);
                if let Some(serial) = serial {
                    tracing::info!(%source, serial, "scheduled NRTMv3 mirror advanced");
                }
            }
            Err(_) => {
                status.state = "error";
                tracing::warn!(%source, "scheduled NRTMv3 mirror failed; will retry");
            }
        }
        status.checked_at_unix_seconds = timestamp();
        store.set_mirror_status(source.clone(), status.clone());
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
    config: &MirrorConfig,
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
            let serial = config
                .base_serial
                .context("initial NRTMv3 mirror requires snapshot serial")?;
            (serial, Some(serial))
        }
    };
    let start = base.checked_add(1).context("NRTMv3 serial exhausted")?;
    let Some(mut response) =
        nrtm_client::fetch(config.endpoint, source, start, timeout, max_bytes)?
    else {
        if !config.object_class_filter.is_empty() && !stopped(shutdown) {
            store.confirm_mirror_base(source, base, &config.object_class_filter)?;
        }
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
    let transcript = if config.object_class_filter.is_empty() {
        Reader::new(BufReader::new(stage), source, max_bytes)
    } else {
        Reader::filtered(BufReader::new(stage), source, max_bytes)
    }
    .map_err(anyhow::Error::msg)?;
    let range = transcript.range();
    ensure!(
        range.0 == start,
        "NRTMv3 mirror response starts at an unexpected serial"
    );
    store
        .apply_nrtm3_filtered_if(
            source,
            range,
            initial_base,
            transcript,
            (!config.object_class_filter.is_empty()).then_some(&config.object_class_filter),
            || !stopped(shutdown),
        )
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
                object_class_filter: Default::default(),
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

    #[tokio::test]
    async fn filtered_mirror_follows_serials_without_retaining_credentials_or_partial_journals()
    -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = registry(dir.path())?;
        let listener = listener()?;
        let endpoint = listener.local_addr()?;
        let (observed, completed) = tokio::sync::oneshot::channel();
        let peer =
            thread::spawn(move || -> Result<()> {
                request(&listener, "-g TEST:3:10-LAST\n")?.write_all(concat!(
                "%START Version: 3 TEST 10-12\n\n",
                "ADD 10\n\nmntner: PRIVATE-MNT\nauth: BCRYPT-PW secret-proof\nsource: TEST\n\n",
                "ADD 11\n\naut-num: AS64497\nsource: TEST\n\n",
                "DEL 12\n\naut-num: AS64496\nsource: TEST\n\n%END TEST\n"
            ).as_bytes())?;
                request(&listener, "-g TEST:3:13-LAST\n")?
                    .write_all(b"% Warning: there are no newer updates available\n")?;
                let _ = observed.send(());
                Ok(())
            });
        let config = MirrorConfig {
            endpoint,
            interval_seconds: 1,
            base_serial: Some(9),
            object_class_filter: [String::from("aut-num")].into(),
        };
        let (stop, shutdown) = watch::channel(false);
        let worker = tokio::spawn(run(
            store.clone(),
            "TEST".into(),
            config.clone(),
            4096,
            Duration::from_secs(3),
            shutdown,
        ));
        tokio::time::timeout(Duration::from_secs(8), completed).await??;
        stop.send(true)?;
        tokio::time::timeout(Duration::from_secs(5), worker).await???;
        peer.join().expect("peer thread")?;
        assert_eq!(store.source_serial("TEST")?, Some(12));
        assert_eq!(store.verify()?, 1);
        assert!(store.nrtm3_journal("TEST", 10, None, 3, 4096).is_err());
        let sources = [String::from("TEST")];
        assert!(store.exact("PRIVATE-MNT", &sources, 1)?.is_empty());
        assert!(store.exact("AS64496", &sources, 1)?.is_empty());
        assert_eq!(store.exact("AS64497", &sources, 1)?.len(), 1);
        let status = store.mirror_status();
        assert_eq!(status["TEST"].state, "following");
        assert_eq!(status["TEST"].serial, Some(12));
        assert!(status["TEST"].last_success_unix_seconds.is_some());
        drop(store);
        tokio::task::spawn_blocking(move || -> Result<()> {
        let store = Store::open_for_service(&dir.path().join("registry.redb"))?;
        let transcript = b"%START Version: 3 TEST 13-13\n\nADD 13\n\naut-num: AS64498\nsource: TEST\n\n%END TEST\n";
        // A restart must retain the filter policy; changing it requires a fresh snapshot.
        assert!(
            store
                .apply_nrtm3(
                    "TEST",
                    (13, 13),
                    None,
                    Reader::new(&transcript[..], "TEST", 4096).unwrap()
                )
                .is_err()
        );
        let bad = b"%START Version: 3 TEST 13-14\n\nADD 13\n\naut-num: AS64498\nsource: TEST\n\nADD 14\n\nmntner: WRONG\nsource: OTHER\n\n%END TEST\n";
        assert!(
            store
                .apply_nrtm3_filtered_if(
                    "TEST",
                    (13, 14),
                    None,
                    Reader::new(&bad[..], "TEST", 4096).unwrap(),
                    Some(&config.object_class_filter),
                    || true
                )
                .is_err()
        );
        assert_eq!(store.source_serial("TEST")?, Some(12));
        assert!(store.exact("AS64498", &sources, 1)?.is_empty());
        assert_eq!(store.verify()?, 1);
        Ok(())
        }).await??;
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
            &MirrorConfig {
                endpoint,
                base_serial: Some(9),
                interval_seconds: 1,
                object_class_filter: Default::default(),
            },
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

    #[test]
    fn quiet_filtered_source_persists_its_cursor_and_rejects_policy_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = registry(dir.path())?;
        let listener = listener()?;
        let endpoint = listener.local_addr()?;
        let peer = thread::spawn(move || -> Result<()> {
            for _ in 0..2 {
                request(&listener, "-g TEST:3:10-LAST\n")?
                    .write_all(b"% Warning: there are no newer updates available\n")?;
            }
            Ok(())
        });
        let (_stop, shutdown) = watch::channel(false);
        let mut config = MirrorConfig {
            endpoint,
            base_serial: Some(9),
            interval_seconds: 60,
            object_class_filter: [String::from("aut-num")].into(),
        };
        assert_eq!(
            sync_once(
                &store,
                "TEST",
                &config,
                4096,
                Duration::from_secs(3),
                &shutdown
            )?,
            None
        );
        assert_eq!(store.source_serial("TEST")?, Some(9));
        assert_eq!(store.verify()?, 1);
        drop(store);
        let store = Store::open_for_service(&dir.path().join("registry.redb"))?;
        config.object_class_filter = [String::from("route")].into();
        assert!(
            sync_once(
                &store,
                "TEST",
                &config,
                4096,
                Duration::from_secs(3),
                &shutdown
            )
            .is_err()
        );
        peer.join().expect("peer thread")?;
        assert_eq!(store.source_serial("TEST")?, Some(9));
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
                object_class_filter: Default::default(),
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
