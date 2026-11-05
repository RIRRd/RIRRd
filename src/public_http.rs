//! Opt-in public query admission on the existing loopback HTTP server.
//! Caddy supplies one client-IP header; local monitoring without it is excluded.
use std::{
    collections::{HashMap, VecDeque},
    error::Error as _,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::PublicHttpPolicy;

const MAX_CLIENTS: usize = 4096;
const SAMPLES: usize = 512;

#[derive(Eq, PartialEq, Hash)]
enum Client {
    V4([u8; 4]),
    V6([u8; 8]),
}
impl From<IpAddr> for Client {
    fn from(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V4(ip) => Self::V4(ip.octets()),
            IpAddr::V6(ip) => Self::V6(ip.octets()[..8].try_into().unwrap()),
        }
    }
}

#[derive(Default)]
struct Limiter(HashMap<Client, (f64, Instant)>, Option<Instant>);
impl Limiter {
    fn allow(&mut self, client: Client, now: Instant, policy: PublicHttpPolicy) -> bool {
        if !self.0.contains_key(&client) && self.0.len() >= MAX_CLIENTS {
            if self
                .1
                .is_none_or(|swept| now.saturating_duration_since(swept) >= Duration::from_secs(1))
            {
                // An expired entry restarts with a full burst. Never expire it
                // before its previous budget would have naturally refilled.
                let idle = Duration::from_secs(
                    u64::from(policy.burst.div_ceil(policy.rate_per_second)).max(60),
                );
                self.0
                    .retain(|_, (_, previous)| now.saturating_duration_since(*previous) < idle);
                self.1 = Some(now);
            }
            if self.0.len() >= MAX_CLIENTS {
                return false;
            }
        }
        let burst = f64::from(policy.burst);
        let (tokens, previous) = self.0.entry(client).or_insert((burst, now));
        *tokens = (*tokens
            + now.saturating_duration_since(*previous).as_secs_f64()
                * f64::from(policy.rate_per_second))
        .min(burst);
        *previous = (*previous).max(now);
        if *tokens < 1.0 {
            return false;
        }
        *tokens -= 1.0;
        true
    }
}

#[derive(Default)]
struct Metrics {
    requests: u64,
    successful: u64,
    rate_limited: u64,
    admission_busy: u64,
    server_errors: u64,
    client_errors: u64,
    cancelled: u64,
    deadline_exceeded: u64,
    response_limited: u64,
    response_errors: u64,
    response_buffer_busy: u64,
    response_buffer_bytes: usize,
    peak_response_buffer_bytes: usize,
    active: usize,
    peak: usize,
    successful_us: VecDeque<u64>,
}
impl Metrics {
    fn finish(&mut self, status: StatusCode, elapsed: Duration) {
        if status.is_success() {
            self.successful += 1;
            if self.successful_us.len() == SAMPLES {
                self.successful_us.pop_front();
            }
            self.successful_us
                .push_back(elapsed.as_micros().min(u64::MAX as u128) as u64);
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            self.rate_limited += 1;
        } else if status.is_server_error() {
            self.server_errors += 1;
            if status == StatusCode::GATEWAY_TIMEOUT {
                self.deadline_exceeded += 1;
            }
        } else if status.is_client_error() {
            self.client_errors += 1;
        }
    }
    fn snapshot(&self, policy: PublicHttpPolicy) -> Value {
        let mut samples: Vec<_> = self.successful_us.iter().copied().collect();
        samples.sort_unstable();
        let percentile = |p: usize| {
            samples
                .get((samples.len() * p).div_ceil(100).saturating_sub(1))
                .map(|v| *v as f64 / 1000.0)
        };
        json!({"requests":self.requests,"successful":self.successful,"rate_limited":self.rate_limited,
            "admission_busy":self.admission_busy,"server_errors":self.server_errors,"client_errors":self.client_errors,
            "cancelled":self.cancelled,"deadline_exceeded":self.deadline_exceeded,
            "response_limited":self.response_limited,"response_errors":self.response_errors,
            "response_buffer_busy":self.response_buffer_busy,"response_buffer_bytes":self.response_buffer_bytes,
            "peak_response_buffer_bytes":self.peak_response_buffer_bytes,
            "active":self.active,"peak_active":self.peak,
            "successful_sample_count":samples.len(),"successful_p50_ms":percentile(50),"successful_p95_ms":percentile(95),
            "successful_p99_ms":percentile(99),"successful_max_ms":samples.last().map(|v| *v as f64 / 1000.0),
            "rate_per_second":policy.rate_per_second,"burst":policy.burst,"client_capacity":MAX_CLIENTS,
            "max_active":policy.max_active,"max_response_bytes":policy.max_response_bytes,
            "max_response_buffer_bytes":policy.max_response_buffer_bytes,
            "scope":"Forwarded public query attempts; excludes local monitoring. Successful handler latency includes body preparation, not TLS or network delivery. Last 512 successful HTTP responses; rejected requests never lower that percentile."})
    }
}

pub struct PublicHttp {
    limiter: Mutex<Limiter>,
    metrics: Mutex<Metrics>,
    slots: Arc<Semaphore>,
    response_bytes: Arc<Semaphore>,
    deadline: Duration,
    policy: PublicHttpPolicy,
}
impl PublicHttp {
    pub fn new(deadline: Duration, policy: PublicHttpPolicy) -> Arc<Self> {
        Arc::new(Self {
            limiter: Mutex::new(Limiter::default()),
            metrics: Mutex::new(Metrics::default()),
            slots: Arc::new(Semaphore::new(policy.max_active)),
            response_bytes: Arc::new(Semaphore::new(policy.max_response_buffer_bytes)),
            deadline,
            policy,
        })
    }
    pub fn wrap(self: Arc<Self>, app: Router) -> Router {
        let monitor = Router::new()
            .route(
                "/ui/http-stats.json",
                get(|State(state): State<Arc<Self>>| async move {
                    Json(state.metrics.lock().unwrap().snapshot(state.policy))
                }),
            )
            .with_state(self.clone());
        app.merge(monitor)
            .layer(middleware::from_fn_with_state(self, admit))
    }
    fn response(&self, mut response: Response, started: Instant, id: u64) -> Response {
        let elapsed = started.elapsed();
        self.metrics
            .lock()
            .unwrap()
            .finish(response.status(), elapsed);
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
        headers.insert(
            "server-timing",
            format!("rirrd;dur={:.3}", elapsed.as_secs_f64() * 1000.0)
                .parse()
                .unwrap(),
        );
        headers.insert("x-rirrd-request-id", id.to_string().parse().unwrap());
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status() == StatusCode::SERVICE_UNAVAILABLE
        {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "1".parse().unwrap());
        }
        response
    }
}
struct Active {
    state: Arc<PublicHttp>,
    _permit: OwnedSemaphorePermit,
    _response_permit: Option<OwnedSemaphorePermit>,
    response_bytes: usize,
    complete: bool,
}
impl Drop for Active {
    fn drop(&mut self) {
        let mut metrics = self.state.metrics.lock().unwrap();
        metrics.active -= 1;
        metrics.response_buffer_bytes -= self.response_bytes;
        if !self.complete {
            metrics.cancelled += 1;
        }
    }
}

// The admission and byte permits follow the actual Bytes allocation, including
// Hyper's clones/slices, until the transport consumes or drops its final owner.
// A guard attached only to the handler future or Body would be released too soon.
struct AdmittedBytes {
    bytes: Bytes,
    _active: Active,
}
impl AsRef<[u8]> for AdmittedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

fn prepared_error(status: StatusCode, text: &'static str) -> (axum::http::response::Parts, Bytes) {
    let (parts, _) = status.into_response().into_parts();
    (parts, Bytes::from_static(text.as_bytes()))
}

async fn admit(State(state): State<Arc<PublicHttp>>, request: Request, next: Next) -> Response {
    // The listener is validated as loopback-only. The public reverse proxy must
    // overwrite this header from its immediate connection; never expose /ui/*.
    let mut clients = request.headers().get_all("x-rirrd-client").iter();
    let Some(client) = clients.next() else {
        return next.run(request).await;
    };
    let started = Instant::now();
    let id = {
        let mut metrics = state.metrics.lock().unwrap();
        metrics.requests += 1;
        metrics.requests
    };
    let path = request.uri().path();
    let permitted = (request.method() == Method::GET
        && ["/v1/whois/", "/v1/irr/objects"].contains(&path))
        || (request.method() == Method::POST
            && path == "/graphql/"
            && request.uri().query().is_none());
    let early = if !permitted {
        Some(StatusCode::NOT_FOUND)
    } else if request.uri().to_string().len() > 4096 {
        Some(StatusCode::URI_TOO_LONG)
    } else if clients.next().is_some() || request.headers().contains_key(header::TRANSFER_ENCODING)
    {
        Some(StatusCode::BAD_REQUEST)
    } else {
        None
    };
    if let Some(status) = early {
        return state.response(status.into_response(), started, id);
    }
    let Ok(ip) = client.to_str().unwrap_or("").parse::<IpAddr>() else {
        return state.response(StatusCode::BAD_REQUEST.into_response(), started, id);
    };
    let lengths: Vec<_> = request
        .headers()
        .get_all(header::CONTENT_LENGTH)
        .iter()
        .collect();
    if lengths.len() > 1 {
        return state.response(StatusCode::BAD_REQUEST.into_response(), started, id);
    }
    if let Some(length) = lengths.first() {
        let Ok(length) = length.to_str().unwrap_or("").parse::<u64>() else {
            return state.response(StatusCode::BAD_REQUEST.into_response(), started, id);
        };
        let status = if length > 16 * 1024 {
            Some(StatusCode::PAYLOAD_TOO_LARGE)
        } else if request.method() == Method::GET && length > 0 {
            Some(StatusCode::BAD_REQUEST)
        } else {
            None
        };
        if let Some(status) = status {
            return state.response(status.into_response(), started, id);
        }
    }
    if !state
        .limiter
        .lock()
        .unwrap()
        .allow(ip.into(), started, state.policy)
    {
        return state.response(StatusCode::TOO_MANY_REQUESTS.into_response(), started, id);
    }
    let Ok(permit) = state.slots.clone().try_acquire_owned() else {
        state.metrics.lock().unwrap().admission_busy += 1;
        return state.response(StatusCode::SERVICE_UNAVAILABLE.into_response(), started, id);
    };
    {
        let mut metrics = state.metrics.lock().unwrap();
        metrics.active += 1;
        metrics.peak = metrics.peak.max(metrics.active);
    }
    let mut active = Active {
        state: state.clone(),
        _permit: permit,
        _response_permit: None,
        response_bytes: 0,
        complete: false,
    };
    let result = tokio::time::timeout(state.deadline, async {
        let response = next.run(request).await;
        let (parts, body) = response.into_parts();
        match to_bytes(body, state.policy.max_response_bytes).await {
            Ok(bytes) => (parts, bytes),
            Err(error)
                if error
                    .source()
                    .is_some_and(|source| source.is::<http_body_util::LengthLimitError>()) =>
            {
                state.metrics.lock().unwrap().response_limited += 1;
                prepared_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Response exceeds configured public limit; narrow the query",
                )
            }
            Err(_) => {
                state.metrics.lock().unwrap().response_errors += 1;
                prepared_error(
                    StatusCode::BAD_GATEWAY,
                    "Unable to prepare complete response",
                )
            }
        }
    })
    .await
    .unwrap_or_else(|_| prepared_error(StatusCode::GATEWAY_TIMEOUT, "Query deadline exceeded"));
    let (parts, bytes) = result;
    let (parts, bytes) = match state
        .response_bytes
        .clone()
        .try_acquire_many_owned(bytes.len() as u32)
    {
        Ok(permit) => {
            active.response_bytes = bytes.len();
            active._response_permit = Some(permit);
            let mut metrics = state.metrics.lock().unwrap();
            metrics.response_buffer_bytes += bytes.len();
            metrics.peak_response_buffer_bytes = metrics
                .peak_response_buffer_bytes
                .max(metrics.response_buffer_bytes);
            (parts, bytes)
        }
        Err(_) => {
            state.metrics.lock().unwrap().response_buffer_busy += 1;
            // Empty overload body needs no byte reservation. Drop the prepared
            // reply before releasing its admission permit; never send a prefix.
            prepared_error(StatusCode::SERVICE_UNAVAILABLE, "")
        }
    };
    active.complete = true;
    let bytes = Bytes::from_owner(AdmittedBytes {
        bytes,
        _active: active,
    });
    state.response(Response::from_parts(parts, Body::from(bytes)), started, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn burst_refill_and_clock_regression_are_bounded() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        let ip = "192.0.2.1".parse::<IpAddr>().unwrap();
        for _ in 0..20 {
            assert!(limiter.allow(ip.into(), now, PublicHttpPolicy::default()));
        }
        assert!(!limiter.allow(ip.into(), now, PublicHttpPolicy::default()));
        assert!(limiter.allow(
            ip.into(),
            now + Duration::from_millis(500),
            PublicHttpPolicy::default()
        ));
        assert!(!limiter.allow(ip.into(), now, PublicHttpPolicy::default()));
    }
    #[test]
    fn mapped_v4_and_ipv6_subnets_share_the_correct_budget() {
        assert!(
            Client::from("::ffff:192.0.2.1".parse::<IpAddr>().unwrap())
                == Client::from("192.0.2.1".parse::<IpAddr>().unwrap())
        );
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for _ in 0..20 {
            assert!(limiter.allow(
                "2001:db8:1::1".parse::<IpAddr>().unwrap().into(),
                now,
                PublicHttpPolicy::default()
            ));
        }
        assert!(!limiter.allow(
            "2001:db8:1::ffff".parse::<IpAddr>().unwrap().into(),
            now,
            PublicHttpPolicy::default()
        ));
        assert!(limiter.allow(
            "2001:db8:2::1".parse::<IpAddr>().unwrap().into(),
            now,
            PublicHttpPolicy::default()
        ));
    }
    #[test]
    fn client_table_does_not_evict_active_limits_and_expires_idle_entries() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for n in 0..MAX_CLIENTS {
            assert!(limiter.allow(
                IpAddr::V4(std::net::Ipv4Addr::from(n as u32)).into(),
                now,
                PublicHttpPolicy::default()
            ));
        }
        assert!(!limiter.allow(
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
            now,
            PublicHttpPolicy::default()
        ));
        assert_eq!(limiter.0.len(), MAX_CLIENTS);
        assert!(limiter.allow(
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
            now + Duration::from_secs(61),
            PublicHttpPolicy::default()
        ));
        assert_eq!(limiter.0.len(), 1);
    }
    #[test]
    fn configured_bursts_cannot_refill_early_through_idle_expiry() {
        let policy = PublicHttpPolicy {
            rate_per_second: 1,
            burst: 120,
            ..PublicHttpPolicy::default()
        };
        let mut limiter = Limiter::default();
        let now = Instant::now();
        let client = || "192.0.2.1".parse::<IpAddr>().unwrap().into();
        for _ in 0..policy.burst {
            assert!(limiter.allow(client(), now, policy));
        }
        assert!(!limiter.allow(client(), now, policy));
        for n in 0..MAX_CLIENTS - 1 {
            assert!(limiter.allow(
                IpAddr::V4(std::net::Ipv4Addr::from(n as u32)).into(),
                now,
                policy
            ));
        }
        assert!(!limiter.allow(
            "192.0.2.2".parse::<IpAddr>().unwrap().into(),
            now + Duration::from_secs(61),
            policy
        ));
        for _ in 0..61 {
            assert!(limiter.allow(client(), now + Duration::from_secs(61), policy));
        }
        assert!(!limiter.allow(client(), now + Duration::from_secs(61), policy));
        assert_eq!(limiter.0.len(), MAX_CLIENTS);
    }
    #[test]
    fn rejected_requests_never_improve_successful_latency() {
        let mut metrics = Metrics::default();
        metrics.finish(StatusCode::OK, Duration::from_millis(200));
        for _ in 0..1000 {
            metrics.finish(StatusCode::TOO_MANY_REQUESTS, Duration::ZERO);
        }
        let report = metrics.snapshot(PublicHttpPolicy::default());
        assert_eq!(report["successful_p95_ms"], 200.0);
        assert_eq!(report["successful_sample_count"], 1);
        assert_eq!(report["rate_limited"], 1000);
    }
    async fn request(address: std::net::SocketAddr, client: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("GET /v1/whois/?q=!6AS64496 HTTP/1.1\r\nHost: localhost\r\nX-RIRRd-Client: {client}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }
    #[tokio::test]
    async fn admission_rejections_do_not_queue_behind_slow_queries() {
        let state = PublicHttp::new(Duration::from_secs(10), PublicHttpPolicy::default());
        let release = Arc::new(tokio::sync::Notify::new());
        let signal = release.clone();
        let app = state.clone().wrap(Router::new().route(
            "/v1/whois/",
            get(move || {
                let signal = signal.clone();
                async move {
                    signal.notified().await;
                    "valid reply"
                }
            }),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut tasks = Vec::new();
        for _ in 0..16 {
            tasks.push(tokio::spawn(request(address, "192.0.2.1")));
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if state.metrics.lock().unwrap().active == 16 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        for _ in 0..4 {
            let reply = tokio::time::timeout(Duration::from_secs(1), request(address, "192.0.2.1"))
                .await
                .unwrap();
            assert!(reply.starts_with("HTTP/1.1 503"));
            assert!(reply.to_lowercase().contains("retry-after: 1"));
        }
        let reply = tokio::time::timeout(Duration::from_secs(1), request(address, "192.0.2.1"))
            .await
            .unwrap();
        assert!(reply.starts_with("HTTP/1.1 429"));
        release.notify_waiters();
        for task in tasks {
            assert!(task.await.unwrap().starts_with("HTTP/1.1 200"));
        }
        let report = state
            .metrics
            .lock()
            .unwrap()
            .snapshot(PublicHttpPolicy::default());
        assert_eq!(report["successful"], 16);
        assert_eq!(report["admission_busy"], 4);
        assert_eq!(report["rate_limited"], 1);
        assert_eq!(report["active"], 0);
        server.abort();
    }
    #[tokio::test]
    async fn oversized_responses_and_deadlines_are_explicit_errors() {
        for oversized in [true, false] {
            let state = PublicHttp::new(
                if oversized {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(20)
                },
                PublicHttpPolicy {
                    max_response_bytes: 1024,
                    ..PublicHttpPolicy::default()
                },
            );
            let app = state.clone().wrap(Router::new().route(
                "/v1/whois/",
                get(move || async move {
                    if !oversized {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    "x".repeat(if oversized { 1025 } else { 1 })
                }),
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let reply = request(address, "192.0.2.1").await;
            assert!(reply.starts_with(if oversized {
                "HTTP/1.1 422"
            } else {
                "HTTP/1.1 504"
            }));
            assert_eq!(
                state
                    .metrics
                    .lock()
                    .unwrap()
                    .snapshot(PublicHttpPolicy::default())["active"],
                0
            );
            let report = state.metrics.lock().unwrap().snapshot(state.policy);
            assert_eq!(report["response_limited"], u64::from(oversized));
            assert_eq!(report["deadline_exceeded"], u64::from(!oversized));
            assert_eq!(state.slots.available_permits(), state.policy.max_active);
            server.abort();
        }
    }

    #[tokio::test]
    async fn configured_concurrency_bounds_queries_but_not_local_monitoring() {
        let policy = PublicHttpPolicy {
            rate_per_second: 2000,
            burst: 2000,
            max_active: 2,
            ..PublicHttpPolicy::default()
        };
        let state = PublicHttp::new(Duration::from_secs(3), policy);
        let release = Arc::new(Semaphore::new(0));
        let signal = release.clone();
        let app = state.clone().wrap(Router::new().route(
            "/v1/whois/",
            get(move || {
                let signal = signal.clone();
                async move {
                    signal.acquire().await.unwrap().forget();
                    "complete reply"
                }
            }),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let first = tokio::spawn(request(address, "192.0.2.1"));
        let second = tokio::spawn(request(address, "192.0.2.1"));
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.metrics.lock().unwrap().active < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            request(address, "192.0.2.1")
                .await
                .starts_with("HTTP/1.1 503")
        );
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut monitor = tokio::net::TcpStream::connect(address).await.unwrap();
        monitor
            .write_all(
                b"GET /ui/http-stats.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut report = String::new();
        tokio::time::timeout(Duration::from_secs(1), monitor.read_to_string(&mut report))
            .await
            .unwrap()
            .unwrap();
        let report: Value = serde_json::from_str(report.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(report["active"], 2);
        assert_eq!(report["requests"], 3);
        assert_eq!(report["rate_per_second"], 2000);
        assert_eq!(report["max_active"], 2);
        release.add_permits(3);
        for task in [first, second] {
            assert!(task.await.unwrap().ends_with("complete reply"));
        }
        assert!(
            request(address, "192.0.2.1")
                .await
                .ends_with("complete reply")
        );
        assert_eq!(state.slots.available_permits(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn real_body_failure_is_not_misreported_as_a_response_limit() {
        let state = PublicHttp::new(Duration::from_secs(2), PublicHttpPolicy::default());
        let app = state.clone().wrap(Router::new().route(
            "/v1/whois/",
            get(|| async {
                Body::from_stream(futures::stream::once(async {
                    Err::<String, _>(std::io::Error::other("synthetic body failure"))
                }))
            }),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(
            request(address, "192.0.2.1")
                .await
                .starts_with("HTTP/1.1 502")
        );
        let report = state.metrics.lock().unwrap().snapshot(state.policy);
        assert_eq!(report["response_errors"], 1);
        assert_eq!(report["response_limited"], 0);
        assert_eq!(report["active"], 0);
        server.abort();
    }

    #[tokio::test]
    async fn slow_consumers_hold_admission_and_global_bytes_until_disconnect() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;
        let size = 20 * 1024 * 1024;
        let policy = PublicHttpPolicy {
            max_active: 2,
            max_response_bytes: size,
            max_response_buffer_bytes: size,
            ..PublicHttpPolicy::default()
        };
        let state = PublicHttp::new(Duration::from_secs(5), policy);
        let calls = Arc::new(AtomicUsize::new(0));
        let app = state.clone().wrap(Router::new().route(
            "/v1/whois/",
            get(move || {
                let calls = calls.clone();
                async move {
                    if calls.fetch_add(1, Ordering::Relaxed) < 2 {
                        "x".repeat(size)
                    } else {
                        "recovered".to_owned()
                    }
                }
            }),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(1024).unwrap();
        let mut slow = socket.connect(address).await.unwrap();
        slow.write_all(b"GET /v1/whois/?q=!6AS64496 HTTP/1.1\r\nHost: localhost\r\nX-RIRRd-Client: 192.0.2.1\r\nConnection: close\r\n\r\n").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.metrics.lock().unwrap().successful != 1 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        // The handler has returned, but its bytes are still owned by transport.
        assert_eq!(state.metrics.lock().unwrap().active, 1);
        assert_eq!(state.metrics.lock().unwrap().response_buffer_bytes, size);
        assert_eq!(state.slots.available_permits(), 1);
        assert_eq!(state.response_bytes.available_permits(), 0);
        assert!(
            request(address, "192.0.2.1")
                .await
                .starts_with("HTTP/1.1 503")
        );
        assert_eq!(state.metrics.lock().unwrap().response_buffer_busy, 1);
        assert_eq!(
            state.metrics.lock().unwrap().peak_response_buffer_bytes,
            size
        );
        drop(slow);
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.slots.available_permits() != 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(state.response_bytes.available_permits(), size);
        assert_eq!(state.metrics.lock().unwrap().response_buffer_bytes, 0);
        assert!(request(address, "192.0.2.1").await.ends_with("recovered"));
        server.abort();
    }
}
