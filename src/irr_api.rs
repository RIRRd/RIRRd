//! Bounded keyset browsing. Only HTTP-permitted routing sources/classes enter it.
use crate::storage::Store;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
#[derive(Clone)]
struct Context {
    store: Arc<Store>,
    sources: Vec<String>,
    slots: Arc<Semaphore>,
    deadline: Duration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    source: String,
    #[serde(rename = "class")]
    class_name: String,
    after: Option<String>,
    limit: Option<usize>,
}
async fn objects(State(context): State<Context>, Query(page): Query<Page>) -> Response {
    let source = page.source.to_ascii_uppercase();
    let class = page.class_name.to_ascii_lowercase();
    let limit = page.limit.unwrap_or(30);
    if !context.sources.contains(&source)
        || ![
            "aut-num",
            "as-set",
            "route-set",
            "route",
            "route6",
            "filter-set",
            "peering-set",
            "rtr-set",
            "inet-rtr",
        ]
        .contains(&class.as_str())
        || !(1..=100).contains(&limit)
        || page
            .after
            .as_ref()
            .is_some_and(|a| a.len() > 512 || a.chars().any(char::is_control))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid or unavailable source, class, cursor or limit"})),
        )
            .into_response();
    }
    let Ok(permit) = context.slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let after = page.after.map(|value| value.to_ascii_uppercase());
    let store = context.store.clone();
    let source_copy = source.clone();
    let class_copy = class.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        store.object_keys(&source_copy, &class_copy, after.as_deref(), limit)
    });
    match tokio::time::timeout(context.deadline, worker).await {
        Ok(Ok(Ok((keys, more)))) => {
            let next = if more { keys.last().cloned() } else { None };
            Json(json!({"source":source,"object_class":class,"keys":keys,"next_cursor":next,"pagination":"keyset; live data can change between pages"})).into_response()
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"object index unavailable"})),
        )
            .into_response(),
    }
}
pub fn router(store: Arc<Store>, sources: Vec<String>, deadline: Duration) -> Router {
    Router::new()
        .route("/v1/irr/objects", get(objects))
        .with_state(Context {
            store,
            sources,
            slots: Arc::new(Semaphore::new(8)),
            deadline,
        })
}
