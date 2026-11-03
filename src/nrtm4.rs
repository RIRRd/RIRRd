//! Offline verification of signed NRTMv4 update notifications.

use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::{
    PublicKey,
    ecdsa::{Signature, VerifyingKey, signature::Verifier},
    pkcs8::DecodePublicKey,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::IgnoredAny};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MAX_NOTIFICATION_BYTES: usize = 14 * 1024 * 1024;
pub const MAX_PUBLIC_KEY_BYTES: usize = 4096;
const MAX_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 1024;
const MAX_URL_BYTES: usize = 2048;
const NANOS_PER_SECOND: i128 = 1_000_000_000;
const MAX_AGE_NANOS: i128 = 24 * 60 * 60 * NANOS_PER_SECOND;
const MAX_FUTURE_NANOS: i128 = 5 * 60 * NANOS_PER_SECOND;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedHeader {
    alg: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    nrtm_version: u8,
    #[serde(rename = "type")]
    kind: String,
    pub source: String,
    pub session_id: String,
    pub(crate) version: u64,
    pub(crate) timestamp: String,
    pub snapshot: FileReference,
    pub deltas: Vec<FileReference>,
    #[serde(
        default,
        deserialize_with = "deserialize_next_signing_key",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) next_signing_key: Option<String>,
    #[serde(default, skip_serializing, rename = "metadata")]
    _metadata: Option<IgnoredAny>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileReference {
    pub version: u64,
    pub url: String,
    #[serde(
        deserialize_with = "deserialize_sha256",
        serialize_with = "serialize_sha256"
    )]
    hash: [u8; 32],
}

impl FileReference {
    pub fn sha256(&self) -> [u8; 32] {
        self.hash
    }
}

/// Verify the pinned IRRd ES256 compact JWS profile and its notification
/// metadata using an independently configured P-256 SPKI public key.
///
/// The metadata authorizes only relative paths and carries expected hashes.
/// A caller must check referenced file contents separately. Verification is
/// stateless and does not establish monotonic history across notifications.
pub fn verify(
    compact: &[u8],
    trusted_key_pem: &str,
    expected_source: &str,
    now: SystemTime,
) -> Result<Notification> {
    ensure!(
        expected_source.len() <= 32
            && crate::rpsl::generic_name(expected_source)
            && !expected_source
                .bytes()
                .any(|byte| byte.is_ascii_lowercase()),
        "invalid expected NRTMv4 source name"
    );
    ensure!(
        compact.len() <= MAX_NOTIFICATION_BYTES,
        "NRTMv4 notification exceeds size limit"
    );
    let public_key = parse_public_key(trusted_key_pem).context("invalid trusted NRTMv4 key")?;
    let mut parts = compact.split(|byte| *byte == b'.');
    let header = parts.next().context("missing JWS protected header")?;
    let payload = parts.next().context("missing JWS payload")?;
    let signature = parts.next().context("missing JWS signature")?;
    ensure!(
        parts.next().is_none() && !header.is_empty() && !payload.is_empty(),
        "expected compact JWS with three non-empty segments"
    );
    ensure!(
        header.len() <= MAX_HEADER_BYTES,
        "JWS protected header exceeds size limit"
    );
    let header_json = URL_SAFE_NO_PAD
        .decode(header)
        .context("invalid JWS protected header encoding")?;
    let protected: ProtectedHeader = serde_json::from_slice(&header_json)
        .map_err(|_| anyhow::anyhow!("invalid JWS protected header"))?;
    ensure!(protected.alg == "ES256", "unsupported JWS algorithm");
    ensure!(signature.len() == 86, "invalid ES256 signature length");
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .context("invalid JWS signature encoding")?;
    let signature = Signature::from_slice(&signature).context("invalid ES256 signature")?;

    let signed_length = header.len() + 1 + payload.len();
    VerifyingKey::from(public_key)
        .verify(&compact[..signed_length], &signature)
        .context("NRTMv4 notification signature verification failed")?;

    ensure!(
        payload.len() <= (MAX_PAYLOAD_BYTES * 4).div_ceil(3),
        "NRTMv4 notification payload exceeds size limit"
    );
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .context("invalid JWS payload encoding")?;
    let mut notification: Notification =
        serde_json::from_slice(&payload).context("invalid NRTMv4 notification payload")?;
    notification.validate(expected_source, now)?;
    notification.session_id.make_ascii_lowercase();
    Ok(notification)
}

pub(crate) fn parse_public_key(pem: &str) -> Result<PublicKey> {
    ensure!(
        pem.len() <= MAX_PUBLIC_KEY_BYTES,
        "public key exceeds size limit"
    );
    PublicKey::from_public_key_pem(pem).context("expected a P-256 SPKI public key in PEM")
}

fn deserialize_next_signing_key<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl Notification {
    fn validate(&self, expected_source: &str, now: SystemTime) -> Result<()> {
        ensure!(self.nrtm_version == 4, "unexpected NRTM protocol version");
        ensure!(self.kind == "notification", "unexpected NRTMv4 file type");
        ensure!(
            self.source == expected_source,
            "NRTMv4 source does not match"
        );
        ensure!(
            is_uuid_v4(&self.session_id),
            "invalid NRTMv4 UUIDv4 session ID"
        );
        validate_timestamp(&self.timestamp, now)?;

        if let Some(key) = &self.next_signing_key {
            parse_public_key(key).context("invalid NRTMv4 next signing key")?;
        }

        let mut paths = HashSet::new();
        for reference in std::iter::once(&self.snapshot).chain(&self.deltas) {
            ensure!(
                reference.version > 0,
                "NRTMv4 file reference version must be positive"
            );
            ensure!(
                valid_relative_path(&reference.url),
                "unsafe NRTMv4 file reference URL"
            );
            ensure!(
                paths.insert(reference.url.as_str()),
                "duplicate NRTMv4 file reference URL"
            );
        }

        ensure!(
            self.deltas
                .windows(2)
                .all(|pair| pair[0].version.checked_add(1) == Some(pair[1].version)),
            "NRTMv4 delta versions must be ordered and contiguous"
        );
        let highest_delta = self.deltas.last().map_or(0, |delta| delta.version);
        ensure!(
            self.version == self.snapshot.version.max(highest_delta),
            "NRTMv4 notification version does not match its references"
        );
        if highest_delta > self.snapshot.version {
            ensure!(
                self.deltas[0].version <= self.snapshot.version + 1,
                "NRTMv4 deltas cannot advance the advertised snapshot to the current version"
            );
        }
        Ok(())
    }
}

pub(crate) fn is_uuid_v4(session: &str) -> bool {
    let bytes = session.as_bytes();
    bytes.len() == 36
        && bytes[14] == b'4'
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn validate_timestamp(timestamp: &str, now: SystemTime) -> Result<()> {
    ensure!(
        timestamp.ends_with('Z'),
        "NRTMv4 timestamp must use UTC Z offset"
    );
    let timestamp =
        OffsetDateTime::parse(timestamp, &Rfc3339).context("invalid NRTMv4 timestamp")?;
    let now = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    let age = now - timestamp.unix_timestamp_nanos();
    ensure!(
        age <= MAX_AGE_NANOS,
        "NRTMv4 notification is older than 24 hours"
    );
    ensure!(
        age >= -MAX_FUTURE_NANOS,
        "NRTMv4 notification timestamp is in the future"
    );
    Ok(())
}

fn valid_relative_path(path: &str) -> bool {
    path.len() <= MAX_URL_BYTES
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !matches!(segment, "." | "..")
                && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                })
        })
}

fn deserialize_sha256<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
    let hex = String::deserialize(deserializer)?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom(
            "expected a 64-digit SHA-256 digest",
        ));
    }
    let mut hash = [0; 32];
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    for (output, pair) in hash.iter_mut().zip(pairs) {
        let high = hex_nibble(pair[0]);
        let low = hex_nibble(pair[1]);
        *output = (high << 4) | low;
    }
    Ok(hash)
}

fn hex_nibble(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0'
    } else {
        byte.to_ascii_lowercase() - b'a' + 10
    }
}

fn serialize_sha256<S: Serializer>(hash: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for byte in hash {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    serializer.serialize_str(&hex)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use p256::{
        ecdsa::{SigningKey, signature::Signer},
        pkcs8::{EncodePublicKey, LineEnding},
    };
    use serde_json::{Value, json};

    use super::*;

    const SESSION: &str = "CA128382-78D9-41D1-8927-1ECEF15275BE";
    const DIGEST: &str = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn test_key(seed: u8) -> Result<SigningKey> {
        SigningKey::from_slice(&[seed; 32]).context("test key")
    }

    fn public_pem(key: &SigningKey) -> Result<String> {
        PublicKey::from(key.verifying_key())
            .to_public_key_pem(LineEnding::LF)
            .context("test public key")
    }

    fn sign(key: &SigningKey, header: &str, payload: &[u8]) -> String {
        let encoded_header = URL_SAFE_NO_PAD.encode(header);
        let encoded_payload = URL_SAFE_NO_PAD.encode(payload);
        let signed = format!("{encoded_header}.{encoded_payload}");
        let signature: Signature = key.sign(signed.as_bytes());
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    fn notification() -> Value {
        json!({
            "nrtm_version": 4,
            "type": "notification",
            "source": "TEST",
            "session_id": SESSION,
            "version": 4,
            "timestamp": "2023-11-14T22:13:20Z",
            "snapshot": {"version": 3, "url": "session/nrtm-snapshot.3.a.json.gz", "hash": DIGEST},
            "deltas": [
                {"version": 2, "url": "nrtm-delta.2.b.json", "hash": DIGEST},
                {"version": 3, "url": "nrtm-delta.3.c.json", "hash": DIGEST},
                {"version": 4, "url": "nrtm-delta.4.d.json", "hash": DIGEST}
            ],
            "metadata": {"note": "synthetic"}
        })
    }

    fn verify_value(key: &SigningKey, pem: &str, payload: &Value) -> Result<Notification> {
        let compact = sign(key, r#"{"alg":"ES256"}"#, &serde_json::to_vec(payload)?);
        verify(compact.as_bytes(), pem, "TEST", now())
    }

    #[test]
    fn verifies_pinned_metadata_and_preserves_authentic_file_references() -> Result<()> {
        let key = test_key(7)?;
        let pem = public_pem(&key)?;
        let next_key = public_pem(&test_key(8)?)?;
        let mut payload = notification();
        payload["next_signing_key"] = json!(next_key);
        let verified = verify_value(&key, &pem, &payload)?;

        assert_eq!(verified.source, "TEST");
        assert_eq!(verified.session_id, SESSION.to_ascii_lowercase());
        assert_eq!(verified.version, 4);
        assert_eq!(verified.snapshot.url, "session/nrtm-snapshot.3.a.json.gz");
        assert_eq!(verified.snapshot.sha256()[..4], [1, 35, 69, 103]);
        assert_eq!(verified.deltas.len(), 3);
        assert_eq!(verified.next_signing_key, Some(next_key));
        let output = serde_json::to_value(&verified)?;
        assert_eq!(output["snapshot"]["hash"], DIGEST.to_ascii_lowercase());
        assert!(output.get("metadata").is_none());
        Ok(())
    }

    #[test]
    fn rejects_wrong_keys_algorithms_headers_and_tampering() -> Result<()> {
        let key = test_key(7)?;
        let pem = public_pem(&key)?;
        let unrelated = public_pem(&test_key(8)?)?;
        let payload = serde_json::to_vec(&notification())?;
        let compact = sign(&key, r#"{"alg":"ES256"}"#, &payload);
        assert!(verify(compact.as_bytes(), &unrelated, "TEST", now()).is_err());
        assert!(verify(compact.as_bytes(), &pem, "OTHER", now()).is_err());

        let mut tampered = compact.clone().into_bytes();
        let payload_offset = tampered.iter().position(|byte| *byte == b'.').unwrap() + 1;
        tampered[payload_offset] = if tampered[payload_offset] == b'A' {
            b'B'
        } else {
            b'A'
        };
        assert!(verify(&tampered, &pem, "TEST", now()).is_err());
        assert!(verify(format!("{compact}.extra").as_bytes(), &pem, "TEST", now()).is_err());

        for header in [
            r#"{"alg":"none"}"#,
            r#"{"alg":"HS256"}"#,
            r#"{"alg":"ES256","alg":"ES256"}"#,
            r#"{"alg":"ES256","crit":["b64"],"b64":false}"#,
            r#"{"alg":"ES256","jku":"https://example.invalid"}"#,
        ] {
            let signed = sign(&key, header, &payload);
            assert!(
                verify(signed.as_bytes(), &pem, "TEST", now()).is_err(),
                "{header}"
            );
        }
        let (input, _) = compact.rsplit_once('.').unwrap();
        let short = format!("{input}.{}", URL_SAFE_NO_PAD.encode([1; 63]));
        assert!(verify(short.as_bytes(), &pem, "TEST", now()).is_err());
        Ok(())
    }

    #[test]
    fn rejects_invalid_authenticated_fields_and_unsafe_references() -> Result<()> {
        let key = test_key(7)?;
        let pem = public_pem(&key)?;
        for (field, value) in [
            ("nrtm_version", json!(3)),
            ("type", json!("delta")),
            ("source", json!("test")),
            ("session_id", json!("ca128382-78d9-51d1-8927-1ecef15275be")),
            ("session_id", json!("ca128382-78d9-41d1-1927-1ecef15275be")),
            ("version", json!(0)),
            ("version", json!(5)),
            ("timestamp", json!("2023-11-13T22:13:19Z")),
            ("timestamp", json!("2023-11-14T22:18:21Z")),
            ("timestamp", json!("2023-11-14T22:13:20+00:00")),
            ("timestamp", json!("2023-02-29T22:13:20Z")),
            ("next_signing_key", json!("not a key")),
            ("next_signing_key", Value::Null),
        ] {
            let mut payload = notification();
            payload[field] = value;
            assert!(verify_value(&key, &pem, &payload).is_err(), "{field}");
        }
        for path in [
            "/absolute.json",
            "https://example.invalid/absolute.json",
            "//example.invalid/absolute.json",
            "some/../escape.json",
            "some/%2e%2e/escape.json",
            r"some\escape.json",
            "some/file.json?query",
            "some/file.json#fragment",
            "some//file.json",
            "nrtm-delta.4.d.json",
        ] {
            let mut payload = notification();
            payload["snapshot"]["url"] = json!(path);
            assert!(verify_value(&key, &pem, &payload).is_err(), "{path}");
        }
        let mut payload = notification();
        payload["snapshot"]["hash"] = json!("bad");
        assert!(verify_value(&key, &pem, &payload).is_err());
        Ok(())
    }

    #[test]
    fn enforces_delta_order_contiguity_and_snapshot_coverage() -> Result<()> {
        let key = test_key(7)?;
        let pem = public_pem(&key)?;
        let mut payload = notification();
        payload["deltas"].as_array_mut().unwrap().swap(0, 1);
        assert!(verify_value(&key, &pem, &payload).is_err());

        let mut payload = notification();
        payload["deltas"].as_array_mut().unwrap().remove(1);
        assert!(verify_value(&key, &pem, &payload).is_err());

        let mut payload = notification();
        payload["snapshot"]["version"] = json!(1);
        payload["deltas"].as_array_mut().unwrap().remove(0);
        assert!(verify_value(&key, &pem, &payload).is_err());

        let mut payload = notification();
        payload["deltas"] = json!([]);
        payload["version"] = json!(3);
        assert!(verify_value(&key, &pem, &payload).is_ok());
        Ok(())
    }
}
