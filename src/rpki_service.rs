//! In-process RPKI relying-party integration. Cryptographic validation and RTR
//! framing come from the attributed, pinned NLnet Labs Routinator/rpki crates.
use crate::{config::RpkiConfig, rpki::RoaTable};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use routinator::{
    engine::Engine,
    payload::SharedHistory,
    rpki::rtr::{
        payload::Timing,
        server::{NotifySender, PayloadSource, Server, Socket},
        state::State as RtrState,
    },
    slurm::LocalExceptions,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, watch},
};

struct Dataset {
    table: RoaTable,
    completed: Instant,
    timestamp: u64,
    serial: u32,
    rejected: u32,
    trust_anchors: usize,
    validation_seconds: f64,
    anchors: Vec<Value>,
}
struct Inner {
    dataset: Option<Arc<Dataset>>,
    state: &'static str,
}
pub struct Shared {
    config: Option<RpkiConfig>,
    inner: RwLock<Inner>,
}
impl Shared {
    pub fn new(config: Option<RpkiConfig>) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: RwLock::new(Inner {
                dataset: None,
                state: "starting",
            }),
        })
    }
    fn snapshot(&self) -> Option<Arc<Dataset>> {
        let limit = self.config.as_ref()?.expire_seconds;
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if ["publishing", "failed", "stopping"].contains(&inner.state) {
            return None;
        }
        inner
            .dataset
            .clone()
            .filter(|d| d.completed.elapsed() < Duration::from_secs(limit))
    }
    fn state(&self, state: &'static str) {
        self.inner.write().unwrap_or_else(|e| e.into_inner()).state = state;
    }
    pub fn status(&self) -> Value {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let enabled = self.config.is_some();
        let dataset = inner.dataset.as_ref();
        let usable = !["publishing", "failed", "stopping"].contains(&inner.state)
            && dataset.is_some_and(|d| {
                self.config
                    .as_ref()
                    .is_some_and(|c| d.completed.elapsed() < Duration::from_secs(c.expire_seconds))
            });
        json!({"enabled":enabled,"usable":usable,"state":if !enabled{"disabled"}else if dataset.is_some_and(|d|self.config.as_ref().is_some_and(|c|d.completed.elapsed() >= Duration::from_secs(c.expire_seconds))){"expired"}else{inner.state},
            "engine":"Routinator 0.15.3 / rpki-rs (embedded Rust library)","independent_validation":true,"transport":"RRDP; rsync disabled",
            "vrps":dataset.map(|d|d.table.len()),"vrps_ipv4":dataset.map(|d|d.table.family_counts().0),"vrps_ipv6":dataset.map(|d|d.table.family_counts().1),"serial":dataset.map(|d|d.serial),"last_success":dataset.map(|d|d.timestamp),
            "validation_seconds":dataset.map(|d|d.validation_seconds),"dataset_age_seconds":dataset.map(|d|d.completed.elapsed().as_secs()),"refresh_seconds":self.config.as_ref().map(|c|c.refresh_seconds),"expire_seconds":self.config.as_ref().map(|c|c.expire_seconds),"trust_anchors":dataset.map(|d|&d.anchors),
            "trust_anchors_with_valid_publication_points":dataset.map(|d|d.trust_anchors),"rejected_publication_points":dataset.map(|d|d.rejected),
            "rtr_listen":self.config.as_ref().and_then(|c|c.rtr_listen).map(|a|a.to_string()),"rtr_exposure":"loopback only",
            "irr_query_filtering":false,"message":if !enabled{"Integrated validator is compiled in but not enabled for this instance."}else if !usable{"No fresh, complete validation run is available. Origin checks return unavailable; this is not a NOT_FOUND verdict."}else{"Locally validated RPKI payloads from all five bundled trust anchors. Rejected repositories and RRDP-only transport can reduce coverage. RTR is private; IRR query suppression remains in development."}})
    }
    fn check(&self, prefix: &str, origin: u32) -> Result<Value> {
        let dataset = self
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("validated dataset unavailable or expired"))?;
        let result = dataset.table.validate(prefix, origin)?;
        let (covering_roas, truncated) = dataset.table.covering_payloads(prefix)?;
        Ok(
            json!({"prefix":prefix,"origin":origin,"state":result,"covering_roas":covering_roas,"covering_roas_truncated":truncated,"validated_vrps":dataset.table.len(),"dataset_serial":dataset.serial,"validated_at_unix_seconds":dataset.timestamp,"engine":"embedded Routinator 0.15.3","rejected_publication_points":dataset.rejected,"coverage":"RRDP validated subset; IRR data is separate"}),
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    prefix: String,
    origin: String,
}
async fn validate(State(shared): State<Arc<Shared>>, Query(query): Query<Check>) -> Response {
    if query.prefix.len() > 64 || query.origin.len() > 12 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let origin = query.origin.to_ascii_uppercase();
    let origin = origin.strip_prefix("AS").unwrap_or(&origin);
    let Ok(origin) = origin.parse::<u32>() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid origin ASN"})),
        )
            .into_response();
    };
    if shared.snapshot().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"RPKI validation dataset unavailable or expired"})),
        )
            .into_response();
    }
    match shared.check(&query.prefix, origin) {
        Ok(value) => Json(value).into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid route prefix"})),
        )
            .into_response(),
    }
}
pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route(
            "/v1/rpki/status",
            get(|State(s): State<Arc<Shared>>| async move { Json(s.status()) }),
        )
        .route("/v1/rpki/validate", get(validate).post(validate_batch))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(shared)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    routes: Vec<Check>,
}

async fn validate_batch(State(shared): State<Arc<Shared>>, Json(batch): Json<Batch>) -> Response {
    if batch.routes.is_empty() || batch.routes.len() > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Provide between 1 and 100 routes"})),
        )
            .into_response();
    }
    // Pin one immutable dataset for the whole request, including its expiry
    // check. A concurrent refresh must not mix serials within the batch.
    let Some(dataset) = shared.snapshot() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"RPKI validation dataset unavailable or expired"})),
        )
            .into_response();
    };
    let mut results = Vec::with_capacity(batch.routes.len());
    for query in batch.routes {
        let origin = query.origin.to_ascii_uppercase();
        let origin = origin.strip_prefix("AS").unwrap_or(&origin);
        let parsed = origin.parse::<u32>();
        if query.prefix.len() > 64 || query.origin.len() > 12 || parsed.is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Invalid prefix or origin; no batch results returned"})),
            )
                .into_response();
        }
        let origin = parsed.unwrap();
        let Ok(state) = dataset.table.validate(&query.prefix, origin) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Invalid prefix or origin; no batch results returned"})),
            )
                .into_response();
        };
        results.push(json!({"prefix":query.prefix,"origin":origin,"state":state}));
    }
    Json(json!({"results":results,"dataset_serial":dataset.serial,"validated_at_unix_seconds":dataset.timestamp})).into_response()
}

#[derive(Clone)]
struct FreshHistory {
    history: SharedHistory,
    shared: Arc<Shared>,
}
impl PayloadSource for FreshHistory {
    type Set = <SharedHistory as PayloadSource>::Set;
    type Diff = <SharedHistory as PayloadSource>::Diff;
    fn ready(&self) -> bool {
        self.shared.snapshot().is_some() && self.history.ready()
    }
    fn notify(&self) -> RtrState {
        self.history.notify()
    }
    fn full(&self) -> (RtrState, Self::Set) {
        self.history.full()
    }
    fn diff(&self, state: RtrState) -> Option<(RtrState, Self::Diff)> {
        if self.ready() {
            self.history.diff(state)
        } else {
            None
        }
    }
    fn timing(&self) -> Timing {
        let mut timing = self.history.timing();
        let remaining = self
            .shared
            .snapshot()
            .and_then(|d| {
                self.shared.config.as_ref().map(|c| {
                    c.expire_seconds
                        .saturating_sub(d.completed.elapsed().as_secs())
                })
            })
            .unwrap_or(1)
            .max(1)
            .min(u64::from(u32::MAX)) as u32;
        timing.expire = timing.expire.min(remaining);
        timing.refresh = timing.refresh.min(timing.expire).max(1);
        timing.retry = timing.retry.min(timing.expire).max(1);
        timing
    }
}
struct LimitedSocket {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
}
impl AsyncRead for LimitedSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for LimitedSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
impl Socket for LimitedSocket {}
fn engine_config(config: &RpkiConfig) -> routinator::Config {
    let mut result = routinator::Config::default_with_paths(
        config.cache_dir.join("embedded.conf"),
        config.cache_dir.clone(),
    );
    result.disable_rsync = true;
    result.validation_threads = config.validation_threads;
    result.history_size = 2;
    result.refresh = Duration::from_secs(config.refresh_seconds);
    result.expire = Duration::from_secs(config.expire_seconds);
    result.unsafe_vrps = routinator::config::FilterPolicy::Reject;
    result.rrdp_timeout = Some(Duration::from_secs(300));
    result.rrdp_read_timeout = Some(Duration::from_secs(30));
    result.rrdp_connect_timeout = Some(Duration::from_secs(10));
    result
}
fn validator(
    config: RpkiConfig,
    history: SharedHistory,
    mut notify: NotifySender,
    shared: Arc<Shared>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    std::fs::create_dir_all(&config.cache_dir)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config.cache_dir.join(".rirrd-validator.lock"))?;
    lock.try_lock()?;
    let settings = engine_config(&config);
    Engine::init(&settings).map_err(|_| anyhow::anyhow!("RPKI cache initialization failed"))?;
    let mut engine = Engine::new(&settings, true)
        .map_err(|_| anyhow::anyhow!("RPKI engine initialization failed"))?;
    engine
        .ignite()
        .map_err(|_| anyhow::anyhow!("RPKI engine startup failed"))?;
    let mut initial = true;
    while !*shutdown.borrow() && shutdown.has_changed().is_ok() {
        shared.state("validating");
        let started = Instant::now();
        history.mark_update_start();
        let run = (|| -> Result<()> {
            let first = routinator::payload::ValidationReport::process(&engine, &settings, initial);
            // A cached quick pass can encounter a newly published repository.
            // Fall back to full validation immediately, rather than spending
            // the ordinary failure retry interval with no initial dataset.
            let (report, metrics) = match first {
                Ok(report) => report,
                Err(_) if initial && !*shutdown.borrow() => {
                    tracing::info!("RPKI cached quick pass incomplete; starting full validation");
                    routinator::payload::ValidationReport::process(&engine, &settings, false)
                        .map_err(|_| anyhow::anyhow!("RPKI validation run failed"))?
                }
                Err(_) => return Err(anyhow::anyhow!("RPKI validation run failed")),
            };
            let anchor_count = metrics
                .tals
                .iter()
                .filter(|t| t.publication.valid_points > 0)
                .count();
            ensure!(
                anchor_count >= 5,
                "not all five trust anchors produced valid publication points"
            );
            let rejected = metrics.publication.rejected_points;
            shared.state("publishing");
            let changed = history.update(report, &LocalExceptions::empty(), metrics);
            history.mark_update_done();
            // Contribution counters are finalized by into_snapshot() inside
            // history.update(), after global deduplication and filtering.
            let final_metrics = history
                .read()
                .metrics()
                .ok_or_else(|| anyhow::anyhow!("missing finalized RPKI metrics"))?;
            let anchors = final_metrics.tals.iter().map(|t| json!({"name":t.name(),"valid_publication_points":t.publication.valid_points,"rejected_publication_points":t.publication.rejected_points,"vrps_ipv4":t.payload.v4_origins.contributed,"vrps_ipv6":t.payload.v6_origins.contributed})).collect();
            let snapshot = history
                .read()
                .current()
                .ok_or_else(|| anyhow::anyhow!("missing RPKI payload snapshot"))?;
            let table = RoaTable::from_verified_prefixes(snapshot.origins().map(|(o, _)| {
                (
                    o.prefix.prefix().addr(),
                    o.prefix.prefix().len(),
                    o.asn.into_u32(),
                    o.prefix.resolved_max_len(),
                )
            }))?;
            ensure!(!table.is_empty(), "validated RPKI payload is empty");
            let serial = history.read().serial().into();
            let dataset = Arc::new(Dataset {
                table,
                completed: Instant::now(),
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                serial,
                rejected,
                trust_anchors: anchor_count,
                validation_seconds: started.elapsed().as_secs_f64(),
                anchors,
            });
            shared
                .inner
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .dataset = Some(dataset);
            shared.state("ready");
            if changed {
                notify.notify();
            }
            Ok(())
        })();
        initial = false;
        if let Err(error) = &run {
            let publishing =
                shared.inner.read().unwrap_or_else(|e| e.into_inner()).state == "publishing";
            shared.state(if publishing { "failed" } else { "degraded" });
            tracing::warn!(error = %error, "integrated RPKI validation failed; prior fresh payloads retained");
        }
        let delay = if run.is_ok() {
            config.refresh_seconds
        } else {
            60
        };
        for _ in 0..delay {
            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(())
}
pub async fn run(
    config: RpkiConfig,
    shared: Arc<Shared>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let history = SharedHistory::from_config(&engine_config(&config));
    let notify = NotifySender::new();
    let listener = if let Some(address) = config.rtr_listen {
        Some(TcpListener::bind(address).await?)
    } else {
        None
    };
    let thread_shared = shared.clone();
    let thread_history = history.clone();
    let thread_notify = notify.clone();
    let thread_shutdown = shutdown.clone();
    std::thread::Builder::new()
        .name("rirrd-rpki-validator".into())
        .spawn(move || {
            if validator(
                config,
                thread_history,
                thread_notify,
                thread_shared.clone(),
                thread_shutdown,
            )
            .is_err()
            {
                thread_shared.state("failed");
                tracing::error!("integrated RPKI engine failed to initialize");
            }
        })?;
    if let Some(listener) = listener {
        let slots = Arc::new(Semaphore::new(16));
        let stream = futures::stream::unfold((listener, slots), |(listener, slots)| async move {
            loop {
                match listener.accept().await {
                    Ok((socket, _)) => {
                        if let Ok(permit) = slots.clone().try_acquire_owned() {
                            return Some((
                                Ok(LimitedSocket {
                                    stream: socket,
                                    _permit: permit,
                                }),
                                (listener, slots),
                            ));
                        }
                    }
                    Err(error) => return Some((Err(error), (listener, slots))),
                }
            }
        });
        let server = Server::new(
            Box::pin(stream),
            notify,
            FreshHistory {
                history,
                shared: shared.clone(),
            },
        );
        tokio::select! {result=server.run()=>result?,_ = shutdown.changed()=>{}}
    } else {
        let _ = shutdown.changed().await;
    }
    shared.state("stopping");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Arc<Shared> {
        let shared = Shared::new(Some(RpkiConfig {
            cache_dir: "/unused-test-cache".into(),
            rtr_listen: None,
            refresh_seconds: 60,
            expire_seconds: 600,
            validation_threads: 1,
        }));
        let table = RoaTable::from_payloads([
            ("192.0.2.0/24".into(), 64496, 24),
            ("2001:db8::/32".into(), 64497, 48),
        ])
        .unwrap();
        {
            let mut inner = shared.inner.write().unwrap();
            inner.state = "ready";
            inner.dataset = Some(Arc::new(Dataset {
                table,
                completed: Instant::now(),
                timestamp: 0,
                serial: 1,
                rejected: 0,
                trust_anchors: 5,
                validation_seconds: 0.1,
                anchors: Vec::new(),
            }));
        }
        shared
    }

    #[tokio::test]
    async fn http_batch_has_one_serial_and_rejects_invalid_or_oversized_requests() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let shared = ready();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = router(shared.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        async fn request(address: std::net::SocketAddr, body: String) -> String {
            let mut socket = TcpStream::connect(address).await.unwrap();
            socket.write_all(format!("POST /v1/rpki/validate HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
            let mut response = Vec::new();
            tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut response))
                .await
                .unwrap()
                .unwrap();
            String::from_utf8(response).unwrap()
        }
        let body=json!({"routes":[{"prefix":"192.0.2.0/24","origin":"AS64496"},{"prefix":"192.0.2.0/24","origin":"AS64497"},{"prefix":"198.51.100.0/24","origin":"64496"}]}).to_string();
        let response = request(address, body.clone()).await;
        assert!(response.starts_with("HTTP/1.1 200"));
        let result: Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(result["dataset_serial"], 1);
        assert_eq!(result["results"][0]["state"], "VALID");
        assert_eq!(result["results"][1]["state"], "INVALID");
        assert_eq!(result["results"][2]["state"], "NOT_FOUND");
        for body in [
            json!({"routes":[]}).to_string(),
            json!({"routes":[{"prefix":"bad","origin":"64496"}]}).to_string(),
            json!({"routes":vec![json!({"prefix":"192.0.2.0/24","origin":"64496"});101]})
                .to_string(),
        ] {
            assert!(request(address, body).await.starts_with("HTTP/1.1 400"));
        }
        assert!(
            request(address, " ".repeat(16385))
                .await
                .starts_with("HTTP/1.1 413")
        );
        shared.state("failed");
        assert!(request(address, body).await.starts_with("HTTP/1.1 503"));
        task.abort();
    }
    #[tokio::test]
    async fn origin_api_never_turns_unavailable_or_expired_data_into_not_found() {
        let disabled = Shared::new(None);
        assert_eq!(disabled.status()["enabled"], false);
        let request = || {
            Query(Check {
                prefix: "192.0.2.0/24".into(),
                origin: "AS64496".into(),
            })
        };
        assert_eq!(
            validate(State(disabled), request()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let shared = ready();
        assert_eq!(
            shared.check("192.0.2.0/24", 64496).unwrap()["state"],
            "VALID"
        );
        assert_eq!(
            shared.check("192.0.2.0/24", 64497).unwrap()["state"],
            "INVALID"
        );
        assert_eq!(
            shared.check("198.51.100.0/24", 64496).unwrap()["state"],
            "NOT_FOUND"
        );
        assert_eq!(
            validate(
                State(shared.clone()),
                Query(Check {
                    prefix: "bad".into(),
                    origin: "AS64496".into()
                })
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        for state in ["publishing", "failed", "stopping"] {
            shared.state(state);
            assert_eq!(shared.status()["usable"], false);
            assert_eq!(
                validate(State(shared.clone()), request()).await.status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
        shared.state("ready");
        {
            let mut inner = shared.inner.write().unwrap();
            let old = inner.dataset.take().unwrap();
            inner.dataset = Some(Arc::new(Dataset {
                table: RoaTable::from_payloads([("192.0.2.0/24".into(), 64496, 24)]).unwrap(),
                completed: Instant::now() - Duration::from_secs(601),
                timestamp: old.timestamp,
                serial: 2,
                rejected: 0,
                trust_anchors: 5,
                validation_seconds: 0.1,
                anchors: Vec::new(),
            }));
        }
        assert_eq!(shared.status()["state"], "expired");
        assert_eq!(
            validate(State(shared), request()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[test]
    fn rtr_never_serves_an_uninitialized_payload_source() {
        let config = RpkiConfig {
            cache_dir: "/unused-test-cache".into(),
            rtr_listen: None,
            refresh_seconds: 60,
            expire_seconds: 600,
            validation_threads: 1,
        };
        let source = FreshHistory {
            history: SharedHistory::from_config(&engine_config(&config)),
            shared: ready(),
        };
        assert!(!source.ready());
        let timing = source.timing();
        assert!(timing.refresh <= timing.expire && timing.retry <= timing.expire);
    }
    #[tokio::test]
    async fn rtr_wire_reset_delta_and_unavailable_dataset() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let shared = ready();
        let config = engine_config(shared.config.as_ref().unwrap());
        let history = SharedHistory::from_config(&config);
        let populate = |asn: u32| {
            let exceptions=LocalExceptions::from_json(&format!(r#"{{"slurmVersion":1,"validationOutputFilters":{{"prefixFilters":[],"bgpsecFilters":[]}},"locallyAddedAssertions":{{"prefixAssertions":[{{"prefix":"192.0.2.0/24","asn":{asn},"maxPrefixLength":24}}],"bgpsecAssertions":[]}}}}"#),false).unwrap();
            history.update(
                routinator::payload::ValidationReport::new(&config),
                &exceptions,
                routinator::metrics::Metrics::new(),
            );
        };
        populate(64496);
        let source = FreshHistory {
            history: history.clone(),
            shared: shared.clone(),
        };
        assert!(source.ready());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stream = futures::stream::unfold(listener, |listener| async move {
            let (socket, _) = listener.accept().await.unwrap();
            Some((Ok(socket), (listener)))
        });
        let server = Server::new(Box::pin(stream), NotifySender::new(), source);
        let task = tokio::spawn(server.run());
        async fn pdu(socket: &mut TcpStream) -> Vec<u8> {
            let mut header = [0u8; 8];
            tokio::time::timeout(Duration::from_secs(2), socket.read_exact(&mut header))
                .await
                .unwrap()
                .unwrap();
            let len = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
            assert!((8..=4096).contains(&len));
            let mut result = header.to_vec();
            result.resize(len, 0);
            socket.read_exact(&mut result[8..]).await.unwrap();
            result
        }
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(&[1, 2, 0, 0, 0, 0, 0, 8]).await.unwrap();
        let response = pdu(&mut socket).await;
        assert_eq!(response[1], 3);
        let session = [response[2], response[3]];
        let prefix = pdu(&mut socket).await;
        assert_eq!(prefix[1], 4);
        assert_eq!(prefix[8], 1);
        assert_eq!(
            u32::from_be_bytes(prefix[16..20].try_into().unwrap()),
            64496
        );
        let end = pdu(&mut socket).await;
        assert_eq!(end[1], 7);
        let serial = u32::from_be_bytes(end[8..12].try_into().unwrap());
        populate(64497);
        let mut query = vec![1, 1, session[0], session[1], 0, 0, 0, 12];
        query.extend_from_slice(&serial.to_be_bytes());
        socket.write_all(&query).await.unwrap();
        assert_eq!(pdu(&mut socket).await[1], 3);
        let mut changes = Vec::new();
        loop {
            let item = pdu(&mut socket).await;
            if item[1] == 7 {
                break;
            }
            assert_eq!(item[1], 4);
            changes.push((
                item[8],
                u32::from_be_bytes(item[16..20].try_into().unwrap()),
            ));
        }
        changes.sort();
        assert_eq!(changes, vec![(0, 64496), (1, 64497)]);
        shared.state("failed");
        let mut unavailable = TcpStream::connect(address).await.unwrap();
        unavailable
            .write_all(&[1, 2, 0, 0, 0, 0, 0, 8])
            .await
            .unwrap();
        let error = pdu(&mut unavailable).await;
        assert_eq!(error[1], 10);
        assert_eq!(u16::from_be_bytes([error[2], error[3]]), 2);
        task.abort();
    }
}
