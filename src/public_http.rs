//! Opt-in public query admission on the existing loopback HTTP server.
//! Caddy supplies one client-IP header; local monitoring without it is excluded.
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const BURST: f64 = 20.0;
const RATE: f64 = 2.0;
const MAX_CLIENTS: usize = 4096;
const MAX_ACTIVE: usize = 16;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
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
    fn allow(&mut self, client: Client, now: Instant) -> bool {
        if !self.0.contains_key(&client) && self.0.len() >= MAX_CLIENTS {
            if self
                .1
                .is_none_or(|swept| now.saturating_duration_since(swept) >= Duration::from_secs(1))
            {
                self.0.retain(|_, (_, previous)| {
                    now.saturating_duration_since(*previous) < Duration::from_secs(60)
                });
                self.1 = Some(now);
            }
            if self.0.len() >= MAX_CLIENTS {
                return false;
            }
        }
        let (tokens, previous) = self.0.entry(client).or_insert((BURST, now));
        *tokens =
            (*tokens + now.saturating_duration_since(*previous).as_secs_f64() * RATE).min(BURST);
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
        } else if status.is_client_error() {
            self.client_errors += 1;
        }
    }
    fn snapshot(&self) -> Value {
        let mut samples: Vec<_> = self.successful_us.iter().copied().collect();
        samples.sort_unstable();
        let percentile = |p: usize| {
            samples
                .get((samples.len() * p).div_ceil(100).saturating_sub(1))
                .map(|v| *v as f64 / 1000.0)
        };
        json!({"requests":self.requests,"successful":self.successful,"rate_limited":self.rate_limited,
            "admission_busy":self.admission_busy,"server_errors":self.server_errors,"client_errors":self.client_errors,
            "cancelled":self.cancelled,"active":self.active,"peak_active":self.peak,
            "successful_sample_count":samples.len(),"successful_p50_ms":percentile(50),"successful_p95_ms":percentile(95),
            "successful_p99_ms":percentile(99),"successful_max_ms":samples.last().map(|v| *v as f64 / 1000.0),
            "rate_per_second":RATE,"burst":BURST,"client_capacity":MAX_CLIENTS,"max_active":MAX_ACTIVE,
            "scope":"Forwarded public query attempts; excludes local monitoring. Successful handler latency includes body preparation, not TLS or network delivery. Last 512 successful HTTP responses; rejected requests never lower that percentile."})
    }
}

pub struct PublicHttp {
    limiter: Mutex<Limiter>,
    metrics: Mutex<Metrics>,
    slots: Arc<Semaphore>,
    deadline: Duration,
}
impl PublicHttp {
    pub fn new(deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            limiter: Mutex::new(Limiter::default()),
            metrics: Mutex::new(Metrics::default()),
            slots: Arc::new(Semaphore::new(MAX_ACTIVE)),
            deadline,
        })
    }
    pub fn wrap(self: Arc<Self>, app: Router) -> Router {
        let monitor = Router::new()
            .route(
                "/ui/http-stats.json",
                get(|State(state): State<Arc<Self>>| async move {
                    Json(state.metrics.lock().unwrap().snapshot())
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
    complete: bool,
}
impl Drop for Active {
    fn drop(&mut self) {
        let mut metrics = self.state.metrics.lock().unwrap();
        metrics.active -= 1;
        if !self.complete {
            metrics.cancelled += 1;
        }
    }
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
    if !state.limiter.lock().unwrap().allow(ip.into(), started) {
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
        complete: false,
    };
    let result = tokio::time::timeout(state.deadline, async move {
        let response = next.run(request).await;
        let (parts, body) = response.into_parts();
        match to_bytes(body, MAX_RESPONSE).await {
            Ok(bytes) => Response::from_parts(parts, Body::from(bytes)),
            Err(_) => (StatusCode::BAD_GATEWAY, "Response exceeds public limit").into_response(),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::GATEWAY_TIMEOUT.into_response());
    active.complete = true;
    state.response(result, started, id)
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
            assert!(limiter.allow(ip.into(), now));
        }
        assert!(!limiter.allow(ip.into(), now));
        assert!(limiter.allow(ip.into(), now + Duration::from_millis(500)));
        assert!(!limiter.allow(ip.into(), now));
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
            assert!(limiter.allow("2001:db8:1::1".parse::<IpAddr>().unwrap().into(), now));
        }
        assert!(!limiter.allow("2001:db8:1::ffff".parse::<IpAddr>().unwrap().into(), now));
        assert!(limiter.allow("2001:db8:2::1".parse::<IpAddr>().unwrap().into(), now));
    }
    #[test]
    fn client_table_does_not_evict_active_limits_and_expires_idle_entries() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for n in 0..MAX_CLIENTS {
            assert!(limiter.allow(IpAddr::V4(std::net::Ipv4Addr::from(n as u32)).into(), now));
        }
        assert!(!limiter.allow("192.0.2.1".parse::<IpAddr>().unwrap().into(), now));
        assert_eq!(limiter.0.len(), MAX_CLIENTS);
        assert!(limiter.allow(
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
            now + Duration::from_secs(61)
        ));
        assert_eq!(limiter.0.len(), 1);
    }
    #[test]
    fn rejected_requests_never_improve_successful_latency() {
        let mut metrics = Metrics::default();
        metrics.finish(StatusCode::OK, Duration::from_millis(200));
        for _ in 0..1000 {
            metrics.finish(StatusCode::TOO_MANY_REQUESTS, Duration::ZERO);
        }
        let report = metrics.snapshot();
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
        let state = PublicHttp::new(Duration::from_secs(10));
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
        let report = state.metrics.lock().unwrap().snapshot();
        assert_eq!(report["successful"], 16);
        assert_eq!(report["admission_busy"], 4);
        assert_eq!(report["rate_limited"], 1);
        assert_eq!(report["active"], 0);
        server.abort();
    }
    #[tokio::test]
    async fn oversized_responses_and_deadlines_are_explicit_errors() {
        for oversized in [true, false] {
            let state = PublicHttp::new(if oversized {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(20)
            });
            let app = state.clone().wrap(Router::new().route(
                "/v1/whois/",
                get(move || async move {
                    if !oversized {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    "x".repeat(if oversized { MAX_RESPONSE + 1 } else { 1 })
                }),
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let reply = request(address, "192.0.2.1").await;
            assert!(reply.starts_with(if oversized {
                "HTTP/1.1 502"
            } else {
                "HTTP/1.1 504"
            }));
            assert_eq!(state.metrics.lock().unwrap().snapshot()["active"], 0);
            server.abort();
        }
    }
}
