//! Bundled, read-only console. Registry HTTP remains loopback-only.
use anyhow::Result;
use axum::{
    Router,
    body::Bytes,
    extract::Request,
    extract::State,
    http::{
        StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::config::NodeConfig;

#[derive(Serialize)]
pub struct Metadata {
    pub mode: &'static str,
    pub version: &'static str,
    pub node: NodeConfig,
    pub sources: Vec<String>,
    pub whois_listen: String,
    pub max_connections: usize,
    pub database_cache_mib: usize,
}

async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CONTENT_SECURITY_POLICY,"default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    headers.insert(X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub fn router(metadata: Metadata, store: std::sync::Arc<crate::storage::Store>) -> Result<Router> {
    let metadata = Bytes::from(serde_json::to_vec(&metadata)?);
    Ok(Router::new()
        .route(
            "/",
            get(|| async { (StatusCode::FOUND, [(LOCATION, "/ui/")]) }),
        )
        .route(
            "/ui",
            get(|| async { (StatusCode::PERMANENT_REDIRECT, [(LOCATION, "/ui/")]) }),
        )
        .route(
            "/ui/",
            get(|| async { Html(include_str!("../web/dashboard/index.html")) }),
        )
        .route(
            "/ui/style.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/dashboard/style.css"),
                )
            }),
        )
        .route(
            "/ui/app.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/dashboard/app.js"),
                )
            }),
        )
        .route(
            "/ui/meta.json",
            get(|State(bytes): State<Bytes>| async {
                ([(CONTENT_TYPE, "application/json")], bytes).into_response()
            }),
        )
        .route(
            "/ui/mirrors.json",
            get(move || {
                let store = store.clone();
                async move { match store.object_count() { Ok(total) => axum::Json(serde_json::json!({"sources":store.mirror_status(), "total_objects":total})).into_response(), Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(), } }
            }),
        )
        .route(
            "/ui/network.json",
            get(|| async {
                (
                    [(CONTENT_TYPE, "application/json")],
                    include_str!("../web/dashboard/network.json"),
                )
            }),
        )
        .with_state(metadata)
        .layer(middleware::from_fn(headers)))
}
