//! Decode the supported password-authenticated IRRd HTTP submission shape.

use std::{collections::HashSet, future::poll_fn, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use axum::{
    Json, Router,
    body::{Body, HttpBody},
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Deserializer};
use serde_json::json;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use crate::{
    authoritative::{Change, MAX_CHANGES},
    maintainer_auth::MAX_PASSWORDS,
    rpsl::{MAX_OBJECT_BYTES, parse},
    storage::Store,
};

pub const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_PASSWORD_BYTES: usize = 72;
const MAX_BODY_READERS: usize = 4;

pub struct Submission {
    pub source: String,
    pub changes: Vec<Change>,
    pub passwords: Zeroizing<Vec<String>>,
}

struct SecretText(Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self(Zeroizing::new(text)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSubmission {
    objects: Vec<WireObject>,
    #[serde(default)]
    passwords: Vec<SecretText>,
    #[serde(default, rename = "override")]
    override_password: Option<SecretText>,
    #[serde(default)]
    api_keys: Vec<SecretText>,
    // Accepted wire metadata. The atomic storage API does not retain a reason.
    #[serde(default, rename = "delete_reason")]
    _delete_reason: Option<SecretText>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireObject {
    object_text: Option<SecretText>,
    attributes: Option<Vec<Attribute>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attribute {
    name: String,
    value: AttributeValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AttributeValue {
    Text(SecretText),
    List(Vec<SecretText>),
}

/// Decode one HTTP request. POST means upsert and DELETE means delete every
/// supplied object. The caller owns the raw request bytes and must limit and
/// protect them before calling this function.
pub fn decode(body: &[u8], delete: bool) -> Result<Submission> {
    ensure!(
        body.len() <= MAX_BODY_BYTES,
        "HTTP submission exceeds byte limit"
    );
    let wire: WireSubmission =
        serde_json::from_slice(body).map_err(|_| anyhow!("invalid HTTP submission JSON"))?;
    ensure!(
        wire.override_password.is_none() && wire.api_keys.is_empty(),
        "override and API key authentication are unsupported"
    );
    ensure!(
        (1..=MAX_CHANGES).contains(&wire.objects.len()),
        "invalid number of HTTP submission objects"
    );
    let passwords = collect_passwords(wire.passwords)?;
    let mut changes = Vec::with_capacity(wire.objects.len());
    let mut source = None;
    for (index, input) in wire.objects.into_iter().enumerate() {
        let text = input
            .into_rpsl()
            .with_context(|| format!("invalid HTTP submission object {}", index + 1))?;
        ensure!(
            !text.lines().any(inline_metadata),
            "inline submission metadata is unsupported in HTTP objects"
        );
        let object = parse(&text)
            .map_err(|_| anyhow!("invalid RPSL in HTTP submission object {}", index + 1))?;
        if let Some(expected) = &source {
            ensure!(
                expected == &object.source,
                "HTTP submission must use one source"
            );
        } else {
            source = Some(object.source.clone());
        }
        changes.push(if delete {
            Change::Delete(object)
        } else {
            Change::Upsert(object)
        });
    }
    Ok(Submission {
        source: source.context("HTTP submission has no source")?,
        changes,
        passwords,
    })
}

fn collect_passwords(input: Vec<SecretText>) -> Result<Zeroizing<Vec<String>>> {
    let mut passwords = Zeroizing::new(Vec::new());
    for SecretText(mut password) in input {
        ensure!(
            !password.is_empty()
                && password.len() <= MAX_PASSWORD_BYTES
                && !password
                    .as_bytes()
                    .iter()
                    .any(|byte| matches!(byte, 0 | b'\n' | b'\r')),
            "invalid HTTP submission password"
        );
        if !passwords.contains(&*password) {
            ensure!(
                passwords.len() < MAX_PASSWORDS,
                "too many distinct HTTP submission passwords"
            );
            passwords.push(std::mem::take(&mut *password));
        }
    }
    ensure!(!passwords.is_empty(), "HTTP submission requires a password");
    Ok(passwords)
}

impl WireObject {
    fn into_rpsl(self) -> Result<Zeroizing<String>> {
        let object_text = self.object_text.filter(|text| !text.0.is_empty());
        let attributes = self.attributes.filter(|attributes| !attributes.is_empty());
        match (object_text, attributes) {
            (Some(SecretText(text)), None) => {
                ensure!(
                    text.len() <= MAX_OBJECT_BYTES,
                    "RPSL object exceeds byte limit"
                );
                Ok(text)
            }
            (None, Some(attributes)) => render_attributes(attributes),
            _ => Err(anyhow!("provide either object_text or attributes")),
        }
    }
}

fn render_attributes(attributes: Vec<Attribute>) -> Result<Zeroizing<String>> {
    let mut text = Zeroizing::new(String::new());
    for Attribute { name, value } in attributes {
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "invalid RPSL attribute name"
        );
        ensure!(
            name.len() + 3 <= MAX_OBJECT_BYTES.saturating_sub(text.len()),
            "RPSL object exceeds byte limit"
        );
        text.push_str(&name);
        text.push_str(": ");
        match value {
            AttributeValue::Text(value) => append_value(&mut text, &value.0)?,
            AttributeValue::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        text.push_str(", ");
                    }
                    append_value(&mut text, &value.0)?;
                }
            }
        }
        text.push('\n');
        ensure!(
            text.len() <= MAX_OBJECT_BYTES,
            "RPSL object exceeds byte limit"
        );
    }
    Ok(text)
}

fn append_value(output: &mut String, value: &str) -> Result<()> {
    for (index, line) in value.split_inclusive('\n').enumerate() {
        let line = line
            .strip_suffix('\n')
            .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line));
        ensure!(
            (index == 0 || line.starts_with([' ', '\t', '+']))
                && !line
                    .bytes()
                    .any(|byte| byte.is_ascii_control() && byte != b'\t'),
            "invalid RPSL attribute value or continuation"
        );
    }
    ensure!(
        !value.ends_with('\n'),
        "attribute value ends with a newline"
    );
    ensure!(
        value.len() <= MAX_OBJECT_BYTES.saturating_sub(output.len()),
        "RPSL object exceeds byte limit"
    );
    output.push_str(value);
    Ok(())
}

fn inline_metadata(line: &str) -> bool {
    let line = line.trim_ascii_start();
    let line = line.strip_prefix('+').map_or(line, str::trim_ascii_start);
    line.split_once(':').is_some_and(|(name, _)| {
        ["password", "override", "api-key", "delete", "suspension"]
            .iter()
            .any(|marker| name.trim_ascii_end().eq_ignore_ascii_case(marker))
    })
}

#[derive(Clone)]
struct HttpState {
    store: Arc<Store>,
    sources: Arc<HashSet<String>>,
    timeout: Duration,
    body_slots: Arc<Semaphore>,
    write_slot: Arc<Semaphore>,
}

/// Build the opt-in submission endpoint. A request timeout applies while
/// collecting input. An accepted write is awaited through its commit result.
pub fn router(store: Arc<Store>, sources: HashSet<String>, timeout: Duration) -> Router {
    let state = HttpState {
        store,
        sources: Arc::new(sources),
        timeout,
        body_slots: Arc::new(Semaphore::new(MAX_BODY_READERS)),
        write_slot: Arc::new(Semaphore::new(1)),
    };
    Router::new()
        .route("/v1/submit/", post(submit).delete(submit))
        .with_state(state)
}

async fn submit(State(state): State<HttpState>, request: Request) -> Response {
    let Ok(body_slot) = state.body_slots.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let delete = request.method() == Method::DELETE;
    let body = match tokio::time::timeout(state.timeout, read_body(request.into_body())).await {
        Ok(Ok(body)) => body,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    let submission = decode(&body, delete);
    drop(body);
    drop(body_slot);
    let Ok(submission) = submission else {
        return (StatusCode::BAD_REQUEST, "invalid HTTP submission").into_response();
    };
    let identities = submission
        .changes
        .iter()
        .map(|change| {
            let object = change.object();
            (object.class.clone(), object.key.clone())
        })
        .collect::<Vec<_>>();
    if !state.sources.contains(&submission.source) {
        return report(identities, delete, None);
    }
    let Ok(slot) = state.write_slot.try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        state.store.apply_authoritative_report(
            &submission.source,
            submission.changes,
            &submission.passwords,
        )
    })
    .await;
    match result {
        Ok(Ok(actions)) if actions.len() == identities.len() => {
            report(identities, delete, Some(actions))
        }
        Ok(Err(_)) => report(identities, delete, None),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn read_body(mut body: Body) -> Result<Zeroizing<Vec<u8>>, StatusCode> {
    if body.size_hint().lower() > MAX_BODY_BYTES as u64 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(
        body.size_hint()
            .upper()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(MAX_BODY_BYTES)
            .min(MAX_BODY_BYTES),
    ));
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Ok(chunk) = frame.into_data() {
            if chunk.len() > MAX_BODY_BYTES - bytes.len() {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok(bytes)
}

fn report(
    identities: Vec<(String, String)>,
    delete: bool,
    actions: Option<Vec<&'static str>>,
) -> Response {
    let count = identities.len();
    let successful = actions.is_some();
    let count_action = |name| {
        actions
            .iter()
            .flatten()
            .filter(|&&action| action == name)
            .count()
    };
    let objects = identities
        .into_iter()
        .enumerate()
        .map(|(index, (class, key))| {
            let action = actions
                .as_ref()
                .map(|actions| actions[index])
                .or_else(|| delete.then_some("delete"));
            json!({
                "successful": successful,
                "type": action,
                "object_class": class,
                "rpsl_pk": successful.then_some(key),
                "info_messages": [],
                "error_messages": if successful { &[][..] } else { &["submission rejected"][..] },
                "new_object_text": null,
                "submitted_object_text": null,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "request_meta": {},
        "summary": {
            "objects_found": count,
            "successful": if successful { count } else { 0 },
            "successful_create": count_action("create"),
            "successful_modify": count_action("modify"),
            "successful_delete": count_action("delete"),
            "failed": if successful { 0 } else { count },
            "failed_create": 0,
            "failed_modify": 0,
            "failed_delete": if !successful && delete { count } else { 0 },
        },
        "objects": objects,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn contact(source: &str) -> Value {
        json!({"object_text":format!("person: Example Person\nsource: {source}\nnic-hdl: TEST1-TEST\n")})
    }

    fn body(objects: Vec<Value>) -> Value {
        json!({"objects": objects, "passwords": ["synthetic-password"]})
    }

    fn run(body: &Value, delete: bool) -> Result<Submission> {
        decode(&serde_json::to_vec(body)?, delete)
    }

    fn rejected(body: &Value, delete: bool) -> String {
        run(body, delete)
            .err()
            .expect("invalid request accepted")
            .to_string()
    }

    #[test]
    fn decodes_ordered_text_and_attributes_for_one_source() -> Result<()> {
        let second = json!({"attributes":[
            {"name":"role","value":"Example Role"},
            {"name":"mnt-by","value":["ONE-MNT","TWO-MNT"]},
            {"name":"remarks","value":"First line\r\n+ continuation"},
            {"name":"nic-hdl","value":"TEST2-TEST"},
            {"name":"source","value":"TEST"}
        ]});
        let mut input = body(vec![contact("test"), second]);
        input["passwords"] = json!(["one", "one", "two"]);
        let result = run(&input, false)?;
        assert_eq!(result.source, "TEST");
        assert_eq!(&*result.passwords, &["one", "two"]);
        assert_eq!(result.changes.len(), 2);
        assert!(matches!(&result.changes[0], Change::Upsert(object) if object.key == "TEST1-TEST"));
        assert!(matches!(&result.changes[1], Change::Upsert(object)
            if object.key == "TEST2-TEST"
                && object.text.contains("mnt-by: ONE-MNT, TWO-MNT\n")
                && object.text.contains("remarks: First line\r\n+ continuation\n")));
        Ok(())
    }

    #[test]
    fn uses_delete_method_and_rejects_inline_metadata() -> Result<()> {
        let mut input = body(vec![contact("TEST")]);
        input["delete_reason"] = json!("synthetic deletion reason");
        assert!(matches!(&run(&input, true)?.changes[0], Change::Delete(_)));
        assert!(matches!(&run(&input, false)?.changes[0], Change::Upsert(_)));
        for meta in [
            "delete: synthetic reason",
            "Password: synthetic-password",
            "+ override: synthetic-secret",
            "api-key: synthetic-key",
            "suspension: suspend",
        ] {
            let mut object = contact("TEST");
            object["object_text"] = json!(format!(
                "{}{meta}\n",
                object["object_text"].as_str().unwrap()
            ));
            let message = rejected(&body(vec![object]), false);
            assert_eq!(
                message,
                "inline submission metadata is unsupported in HTTP objects"
            );
            assert!(!message.contains("synthetic"));
        }
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_shapes_mixed_sources_and_injected_attributes() {
        assert_eq!(
            rejected(&body(vec![contact("TEST"), contact("OTHER")]), false),
            "HTTP submission must use one source"
        );
        let mut both = contact("TEST");
        both["attributes"] = json!([{"name":"source", "value":"TEST"}]);
        assert!(rejected(&body(vec![both]), false).contains("object 1"));

        for injected in [
            json!({"name":"remarks\nsource","value":"TEST"}),
            json!({"name":"remarks","value":"comment\nsource: OTHER"}),
            json!({"name":"remarks","value":["comment\n","source: OTHER"]}),
        ] {
            let object = json!({"attributes":[
                {"name":"person","value":"Example"},
                injected,
                {"name":"nic-hdl","value":"TEST1-TEST"},
                {"name":"source","value":"TEST"}
            ]});
            assert!(rejected(&body(vec![object]), false).contains("object 1"));
        }
    }

    #[test]
    fn rejects_unsupported_credentials_and_size_limits_with_redacted_errors() {
        for (field, value) in [
            ("override", json!("sensitive override")),
            ("api_keys", json!(["sensitive token"])),
            ("passwords", json!(["sensitive\nsecond"])),
            ("passwords", json!(["é".repeat(37)])),
            ("passwords", json!([])),
        ] {
            let mut input = body(vec![contact("TEST")]);
            input[field] = value;
            assert!(!rejected(&input, false).contains("sensitive"));
        }
        let mut input = body(vec![contact("TEST")]);
        input["passwords"] = json!(
            (0..=MAX_PASSWORDS)
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            rejected(&input, false),
            "too many distinct HTTP submission passwords"
        );
        assert!(rejected(&body(vec![contact("TEST"); MAX_CHANGES + 1]), false).contains("number"));
        let oversized = vec![b' '; MAX_BODY_BYTES + 1];
        assert_eq!(
            decode(&oversized, false).err().unwrap().to_string(),
            "HTTP submission exceeds byte limit"
        );
        let malformed = b"{\"passwords\":[\"sensitive-token\"],\"objects\":[";
        assert_eq!(
            decode(malformed, false).err().unwrap().to_string(),
            "invalid HTTP submission JSON"
        );
    }

    #[tokio::test]
    async fn bounds_http_body_and_builds_redacted_atomic_reports() -> Result<()> {
        assert!(matches!(
            read_body(Body::from(vec![b' '; MAX_BODY_BYTES + 1])).await,
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        ));
        let body = read_body(Body::from("test input")).await.unwrap();
        assert_eq!(&*body, b"test input");

        let objects = vec![
            ("person".into(), "TEST1-TEST".into()),
            ("mntner".into(), "TEST-MNT".into()),
        ];
        let success = report(objects.clone(), false, Some(vec!["create", "modify"]));
        assert_eq!(success.status(), StatusCode::OK);
        let payload: Value = serde_json::from_slice(
            &axum::body::to_bytes(success.into_body(), MAX_BODY_BYTES).await?,
        )?;
        assert_eq!(payload["summary"]["successful_create"], 1);
        assert_eq!(payload["summary"]["successful_modify"], 1);
        assert_eq!(payload["objects"][1]["type"], "modify");
        assert_eq!(payload["objects"][1]["rpsl_pk"], "TEST-MNT");
        assert!(payload["objects"][1]["submitted_object_text"].is_null());
        assert!(payload["objects"][1]["new_object_text"].is_null());

        let failed = report(objects, true, None);
        let payload: Value = serde_json::from_slice(
            &axum::body::to_bytes(failed.into_body(), MAX_BODY_BYTES).await?,
        )?;
        assert_eq!(payload["summary"]["failed_delete"], 2);
        assert_eq!(payload["objects"][0]["type"], "delete");
        assert!(payload["objects"][0]["rpsl_pk"].is_null());
        assert_eq!(
            payload["objects"][0]["error_messages"],
            json!(["submission rejected"])
        );
        Ok(())
    }
}
