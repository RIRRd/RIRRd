//! Verified NRTMv4 transition policy. A caller must commit the resulting state
//! in the same transaction as all file operations from the plan.

use std::{collections::BTreeSet, time::SystemTime};

use anyhow::{Context, Result, ensure};
use p256::pkcs8::EncodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::nrtm4::{FileReference, Notification, is_uuid_v4, parse_public_key, verify};

// Reaching a history cap requires explicit operator intervention. Evicting
// entries here would make retired sessions or signing keys usable again.
const MAX_RETIRED_SESSIONS: usize = 1024;
const MAX_RETIRED_KEYS: usize = 1024;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommittedState {
    pub source: String,
    pub session_id: String,
    pub version: u64,
    timestamp_nanos: i128,
    signed_body_sha256: [u8; 32],
    current_key: String,
    next_key: Option<String>,
    retired_sessions: BTreeSet<String>,
    retired_keys: BTreeSet<[u8; 32]>,
    snapshot: FileReference,
    deltas: Vec<FileReference>,
}

pub enum Trust<'a> {
    Initial { source: &'a str, key_pem: &'a str },
    Established(&'a CommittedState),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Apply {
    UpToDate,
    Deltas { first: u64 },
    Snapshot { first_delta: Option<u64> },
}

#[derive(Debug)]
pub struct Transition {
    pub notification: Notification,
    pub action: Apply,
    /// Publish only after verifying and atomically applying every selected file.
    pub next_state: CommittedState,
}

pub fn plan(trust: Trust<'_>, compact: &[u8], now: SystemTime) -> Result<Transition> {
    let (previous, signer, notification) = match trust {
        Trust::Initial { source, key_pem } => {
            (None, key_pem, verify(compact, key_pem, source, now)?)
        }
        Trust::Established(state) => {
            state.validate()?;
            let (signer, notification) = verify_established(state, compact, now)?;
            (Some(state), signer, notification)
        }
    };

    let timestamp_nanos =
        OffsetDateTime::parse(&notification.timestamp, &Rfc3339)?.unix_timestamp_nanos();
    let signed_end = compact
        .iter()
        .rposition(|byte| *byte == b'.')
        .context("missing NRTMv4 signature separator")?;
    let signed_body_sha256 = Sha256::digest(&compact[..signed_end]).into();
    let signer_id = key_id(signer)?;
    let mut retired_sessions = BTreeSet::new();
    let mut retired_keys = BTreeSet::new();
    let mut action = snapshot_plan(&notification);

    if let Some(previous) = previous {
        let same_session = notification.session_id == previous.session_id;
        ensure!(
            !previous.retired_sessions.contains(&notification.session_id),
            "NRTMv4 notification names a retired session"
        );
        ensure!(
            timestamp_nanos >= previous.timestamp_nanos,
            "NRTMv4 notification timestamp is older than committed state"
        );
        if timestamp_nanos == previous.timestamp_nanos {
            ensure!(
                same_session
                    && (notification.version > previous.version
                        || signed_body_sha256 == previous.signed_body_sha256),
                "NRTMv4 notification replay or conflicting timestamp"
            );
        }

        retired_sessions = previous.retired_sessions.clone();
        retired_keys = previous.retired_keys.clone();
        if same_session {
            ensure!(
                notification.version >= previous.version,
                "NRTMv4 version is older than committed state"
            );
            ensure_unchanged_references(previous, &notification)?;
            if notification.version == previous.version {
                action = Apply::UpToDate;
            } else {
                let next = previous.version + 1;
                if notification
                    .deltas
                    .last()
                    .is_some_and(|reference| reference.version == notification.version)
                    && notification
                        .deltas
                        .binary_search_by_key(&next, |reference| reference.version)
                        .is_ok()
                {
                    action = Apply::Deltas { first: next };
                }
            }
        } else {
            // A new session has independent versions and file hashes. A valid
            // signed rollover always starts from the new session's snapshot.
            ensure!(
                retired_sessions.len() < MAX_RETIRED_SESSIONS,
                "NRTMv4 retired session limit reached"
            );
            retired_sessions.insert(previous.session_id.clone());
        }
        let previous_key = key_id(&previous.current_key)?;
        if previous_key != signer_id {
            ensure!(
                retired_keys.len() < MAX_RETIRED_KEYS,
                "NRTMv4 retired key limit reached"
            );
            retired_keys.insert(previous_key);
        }
    }

    if let Some(key) = &notification.next_signing_key {
        let candidate = key_id(key)?;
        ensure!(
            candidate != signer_id,
            "NRTMv4 next signing key equals the active key"
        );
        ensure!(
            !retired_keys.contains(&candidate),
            "NRTMv4 notification attempts to reuse a retired signing key"
        );
    }

    let next_state = CommittedState {
        source: notification.source.clone(),
        session_id: notification.session_id.clone(),
        version: notification.version,
        timestamp_nanos,
        signed_body_sha256,
        current_key: signer.to_owned(),
        next_key: notification.next_signing_key.clone(),
        retired_sessions,
        retired_keys,
        snapshot: notification.snapshot.clone(),
        deltas: notification.deltas.clone(),
    };
    Ok(Transition {
        notification,
        action,
        next_state,
    })
}

impl CommittedState {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.version > 0
                && is_canonical_session(&self.session_id)
                && !self.retired_sessions.contains(&self.session_id)
                && self.retired_sessions.len() <= MAX_RETIRED_SESSIONS
                && self
                    .retired_sessions
                    .iter()
                    .all(|session| is_canonical_session(session))
                && self.retired_keys.len() <= MAX_RETIRED_KEYS
                && self.snapshot.version > 0
                && self.version
                    == self
                        .snapshot
                        .version
                        .max(self.deltas.last().map_or(0, |delta| delta.version))
                && self
                    .deltas
                    .windows(2)
                    .all(|pair| pair[0].version.checked_add(1) == Some(pair[1].version)),
            "invalid committed NRTMv4 state"
        );
        let active = key_id(&self.current_key)?;
        ensure!(
            !self.retired_keys.contains(&active),
            "committed active signing key is retired"
        );
        if let Some(next) = &self.next_key {
            let next = key_id(next)?;
            ensure!(
                next != active && !self.retired_keys.contains(&next),
                "invalid committed next signing key"
            );
        }
        Ok(())
    }
}

fn is_canonical_session(session: &str) -> bool {
    is_uuid_v4(session) && !session.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn verify_established<'a>(
    state: &'a CommittedState,
    compact: &[u8],
    now: SystemTime,
) -> Result<(&'a str, Notification)> {
    match verify(compact, &state.current_key, &state.source, now) {
        Ok(notification) => Ok((&state.current_key, notification)),
        Err(current_error) => {
            let Some(next) = &state.next_key else {
                return Err(current_error);
            };
            let notification = verify(compact, next, &state.source, now)
                .context("NRTMv4 notification failed verification with both committed keys")?;
            Ok((next, notification))
        }
    }
}

fn snapshot_plan(notification: &Notification) -> Apply {
    Apply::Snapshot {
        first_delta: (notification.snapshot.version < notification.version)
            .then(|| notification.snapshot.version + 1),
    }
}

fn ensure_unchanged_references(previous: &CommittedState, current: &Notification) -> Result<()> {
    ensure!(
        previous.snapshot.version != current.snapshot.version
            || previous.snapshot == current.snapshot,
        "NRTMv4 snapshot reference rewrites history within the session"
    );
    for reference in &current.deltas {
        if let Ok(index) = previous
            .deltas
            .binary_search_by_key(&reference.version, |old| old.version)
        {
            ensure!(
                previous.deltas[index] == *reference,
                "NRTMv4 delta reference rewrites history within the session"
            );
        }
    }
    Ok(())
}

fn key_id(pem: &str) -> Result<[u8; 32]> {
    let der = parse_public_key(pem)?.to_public_key_der()?;
    Ok(Sha256::digest(der.as_bytes()).into())
}

#[cfg(test)]
pub(crate) use tests::fixture_state;

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::{
        PublicKey,
        ecdsa::{Signature, SigningKey, signature::Signer},
        pkcs8::LineEnding,
    };
    use serde_json::{Value, json};

    use super::*;

    const SESSION_A: &str = "52bfd73d-3309-43ee-93c7-3d28698284f0";
    const SESSION_B: &str = "710f9825-bf1a-4b84-8553-a129377e4bde";
    const SESSION_C: &str = "328bfa3b-864b-4c14-b312-83e9427fba75";

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_039)
    }

    fn key(seed: u8) -> Result<SigningKey> {
        SigningKey::from_slice(&[seed; 32]).context("fixture signing key")
    }

    fn pem(key: &SigningKey) -> Result<String> {
        PublicKey::from(key.verifying_key())
            .to_public_key_pem(LineEnding::LF)
            .context("fixture public key")
    }

    fn sign(key: &SigningKey, notification: &Value) -> Vec<u8> {
        let body = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(notification).unwrap())
        );
        let signature: Signature = key.sign(body.as_bytes());
        format!("{body}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes())).into_bytes()
    }

    fn notification(session: &str, snapshot: u64, latest: u64, second: u8) -> Value {
        let file = |kind, version| {
            json!({
                "version": version,
                "url": format!("{session}/{kind}-{version}.json"),
                "hash": "a".repeat(64),
            })
        };
        let deltas = (snapshot..latest)
            .map(|previous| file("delta", previous + 1))
            .collect::<Vec<_>>();
        json!({
            "nrtm_version": 4,
            "type": "notification",
            "source": "TEST",
            "session_id": session,
            "version": latest,
            "timestamp": format!("2023-11-14T22:13:{second:02}Z"),
            "snapshot": file("snapshot", snapshot),
            "deltas": deltas,
        })
    }

    fn initial(key_pem: &str, compact: &[u8]) -> Result<Transition> {
        plan(
            Trust::Initial {
                source: "TEST",
                key_pem,
            },
            compact,
            now(),
        )
    }

    fn established(state: &CommittedState, key: &SigningKey, data: &Value) -> Result<Transition> {
        plan(Trust::Established(state), &sign(key, data), now())
    }

    pub(crate) fn fixture_state(source: &str, session: &str, version: u64) -> CommittedState {
        let key = key(1).unwrap();
        let key_pem = pem(&key).unwrap();
        let mut data = notification(session, version, version, 20);
        data["source"] = json!(source);
        plan(
            Trust::Initial {
                source,
                key_pem: &key_pem,
            },
            &sign(&key, &data),
            now(),
        )
        .unwrap()
        .next_state
    }

    #[test]
    fn initial_trust_is_explicit_and_delta_updates_are_contiguous() -> Result<()> {
        let signer = key(1)?;
        let data = notification(SESSION_A, 2, 4, 20);
        let compact = sign(&signer, &data);
        assert!(initial(&pem(&key(2)?)?, &compact).is_err());
        let first = initial(&pem(&signer)?, &compact)?;
        assert_eq!(
            first.action,
            Apply::Snapshot {
                first_delta: Some(3)
            }
        );
        assert_eq!(first.notification.version, 4);
        assert_eq!(
            established(&first.next_state, &signer, &data)?.action,
            Apply::UpToDate
        );
        let next = established(
            &first.next_state,
            &signer,
            &notification(SESSION_A, 2, 5, 21),
        )?;
        assert_eq!(next.action, Apply::Deltas { first: 5 });
        assert_eq!(next.next_state.version, 5);
        assert_eq!(
            established(
                &next.next_state,
                &signer,
                &notification(SESSION_A, 8, 9, 22),
            )?
            .action,
            Apply::Snapshot {
                first_delta: Some(9)
            }
        );
        let mut old_delta_window = notification(SESSION_A, 9, 9, 22);
        old_delta_window["deltas"] = notification(SESSION_A, 5, 7, 22)["deltas"].clone();
        assert_eq!(
            established(&next.next_state, &signer, &old_delta_window)?.action,
            Apply::Snapshot { first_delta: None }
        );
        Ok(())
    }

    #[test]
    fn accepts_new_session_snapshot_despite_lower_version_and_reused_reference_numbers()
    -> Result<()> {
        let signer = key(1)?;
        let prior = initial(
            &pem(&signer)?,
            &sign(&signer, &notification(SESSION_A, 2, 7, 20)),
        )?;
        let mut rollover = notification(SESSION_B, 2, 2, 21);
        rollover["snapshot"]["hash"] = json!("b".repeat(64));
        let next = established(&prior.next_state, &signer, &rollover)?;
        assert_eq!(next.action, Apply::Snapshot { first_delta: None });
        assert_eq!(next.next_state.session_id, SESSION_B);
        let persisted = serde_json::to_vec(&next.next_state)?;
        let recovered: CommittedState = serde_json::from_slice(&persisted)?;
        assert!(
            established(&recovered, &signer, &notification(SESSION_A, 2, 7, 22))
                .unwrap_err()
                .to_string()
                .contains("retired session")
        );
        assert!(
            established(&recovered, &signer, &notification(SESSION_C, 1, 1, 20))
                .unwrap_err()
                .to_string()
                .contains("older than committed")
        );
        Ok(())
    }

    #[test]
    fn rejects_same_session_downgrades_and_reference_rewrites() -> Result<()> {
        let signer = key(1)?;
        let previous = initial(
            &pem(&signer)?,
            &sign(&signer, &notification(SESSION_A, 2, 4, 20)),
        )?;
        let state = &previous.next_state;
        assert!(
            established(state, &signer, &notification(SESSION_A, 2, 3, 21))
                .unwrap_err()
                .to_string()
                .contains("version is older")
        );
        for target in ["snapshot", "delta"] {
            let mut rewrite = notification(SESSION_A, 2, 5, 21);
            if target == "snapshot" {
                rewrite["snapshot"]["hash"] = json!("b".repeat(64));
            } else {
                rewrite["deltas"][0]["hash"] = json!("b".repeat(64));
            }
            assert!(
                established(state, &signer, &rewrite)
                    .unwrap_err()
                    .to_string()
                    .contains("rewrites history")
            );
        }
        let mut conflict = notification(SESSION_A, 2, 4, 20);
        conflict["metadata"] = json!({"note":"different signed body"});
        assert!(established(state, &signer, &conflict).is_err());
        Ok(())
    }

    #[test]
    fn session_rollover_can_promote_only_a_previously_announced_key() -> Result<()> {
        let signer = key(1)?;
        let next_key = key(2)?;
        let other_key = key(3)?;
        let mut announced = notification(SESSION_A, 1, 1, 20);
        announced["next_signing_key"] = json!(pem(&next_key)?);
        let old = initial(&pem(&signer)?, &sign(&signer, &announced))?;
        let next = notification(SESSION_B, 1, 1, 21);
        assert!(established(&old.next_state, &other_key, &next).is_err());
        let promoted = established(&old.next_state, &next_key, &next)?;
        assert_eq!(promoted.action, Apply::Snapshot { first_delta: None });

        let mut later = notification(SESSION_B, 1, 2, 22);
        assert!(established(&promoted.next_state, &signer, &later).is_err());
        assert_eq!(
            established(&promoted.next_state, &next_key, &later)?.action,
            Apply::Deltas { first: 2 }
        );
        later["next_signing_key"] = json!(pem(&signer)?);
        assert!(
            established(&promoted.next_state, &next_key, &later)
                .unwrap_err()
                .to_string()
                .contains("retired signing key")
        );
        Ok(())
    }
}
