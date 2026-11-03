//! Apply a signed notification using locally supplied or fetched reference files.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    nrtm4::{FileReference, MAX_NOTIFICATION_BYTES, MAX_PUBLIC_KEY_BYTES},
    nrtm4_local::{open_verified_file, read_small},
    nrtm4_records::FileKind,
    nrtm4_state::{Apply, Trust, plan},
    storage::Store,
};

const MAX_LOCAL_FILES: usize = 64;
const MAX_REMOTE_FILES: usize = 512;

pub fn apply(
    store: &Store,
    source: &str,
    notification_path: &Path,
    public_key: Option<&Path>,
    paths: &[PathBuf],
    max_bytes: u64,
) -> Result<Value> {
    ensure!(
        paths.len() <= MAX_LOCAL_FILES,
        "too many NRTMv4 files for one application"
    );
    let key = public_key
        .map(|path| read_small(path, MAX_PUBLIC_KEY_BYTES))
        .transpose()?;
    let compact = read_small(notification_path, MAX_NOTIFICATION_BYTES)?;
    let mut local_paths = paths.iter();
    apply_with_files(
        store,
        source,
        &compact,
        key.as_deref(),
        max_bytes,
        Some(paths.len()),
        |_, _| local_paths.next().context("missing local NRTMv4 file"),
    )
}

/// Fetches authenticated references in snapshot-then-delta order, passing the
/// remaining wire budget. The temporary file must contain the raw response body.
pub fn apply_remote(
    store: &Store,
    source: &str,
    compact: &[u8],
    public_key: Option<&[u8]>,
    max_bytes: u64,
    fetch: impl FnMut(&FileReference, u64) -> Result<tempfile::TempPath>,
) -> Result<Value> {
    apply_with_files(store, source, compact, public_key, max_bytes, None, fetch)
}

fn apply_with_files<P: AsRef<Path>>(
    store: &Store,
    source: &str,
    compact: &[u8],
    key: Option<&[u8]>,
    max_bytes: u64,
    local_count: Option<usize>,
    mut open: impl FnMut(&FileReference, u64) -> Result<P>,
) -> Result<Value> {
    ensure!(
        compact.len() <= MAX_NOTIFICATION_BYTES,
        "NRTMv4 notification exceeds the byte limit"
    );
    ensure!(
        key.is_none_or(|key| key.len() <= MAX_PUBLIC_KEY_BYTES),
        "NRTMv4 public key exceeds the byte limit"
    );
    let committed = store.nrtm4_state(source)?;
    let trust = if let Some(committed) = &committed {
        ensure!(key.is_none(), "NRTMv4 source already has a trusted key");
        Trust::Established(committed)
    } else {
        let key = key.context("first NRTMv4 apply requires --public-key")?;
        let key_pem = std::str::from_utf8(key).context("public key must be a PEM file")?;
        Trust::Initial { source, key_pem }
    };
    let transition = plan(trust, compact, SystemTime::now())?;
    let notification = &transition.notification;
    let mut selected = Vec::new();
    let mut replace = false;
    let first_delta = match transition.action {
        Apply::UpToDate => None,
        Apply::Deltas { first } => Some(first),
        Apply::Snapshot { first_delta } => {
            replace = true;
            selected.push((FileKind::Snapshot, &notification.snapshot));
            first_delta
        }
    };
    if let Some(first) = first_delta {
        selected.extend(
            notification
                .deltas
                .iter()
                .filter(|file| file.version >= first)
                .map(|file| (FileKind::Delta, file)),
        );
    }
    let file_limit = if local_count.is_some() {
        MAX_LOCAL_FILES
    } else {
        MAX_REMOTE_FILES
    };
    ensure!(
        selected.len() <= file_limit,
        "too many NRTMv4 files for one application"
    );
    if let Some(count) = local_count {
        ensure!(
            selected.len() == count,
            "signed plan needs {} local files in snapshot-then-delta order, received {count}",
            selected.len()
        );
    }
    let mut staged = Vec::new();
    let mut remaining = max_bytes;
    let decoded = Rc::new(Cell::new(max_bytes));
    for (kind, reference) in selected {
        let path = open(reference, remaining)?;
        let (reader, bytes) = open_verified_file(
            path.as_ref(),
            reference,
            kind,
            source,
            &notification.session_id,
            remaining,
            decoded.clone(),
        )?;
        remaining -= bytes;
        staged.push(reader);
    }
    let records = store.apply_nrtm4(
        source,
        committed.as_ref(),
        &transition.next_state,
        replace,
        staged.into_iter().flatten(),
    )?;
    Ok(json!({
        "source": source, "session_id": transition.next_state.session_id,
        "version": transition.next_state.version, "records": records,
        "replaced_snapshot": replace,
    }))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::{
        ecdsa::{Signature, SigningKey, signature::Signer},
        pkcs8::{EncodePublicKey, LineEnding},
    };
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn remote_does_not_fetch_for_an_unverified_notification() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("test.redb"))?;
        let key = SigningKey::from_slice(&[7; 32])?
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)?;

        let result = apply_remote(
            &store,
            "TEST",
            b"unverified notification",
            Some(key.as_bytes()),
            4096,
            |_, _| panic!("attempted a fetch before notification verification"),
        );

        assert!(result.is_err());
        assert!(store.nrtm4_state("TEST")?.is_none());
        Ok(())
    }

    #[test]
    fn remote_retries_snapshot_and_delta_after_second_hash_failure() -> Result<()> {
        const SOURCE: &str = "TEST";
        const SESSION: &str = "52bfd73d-3309-43ee-93c7-3d28698284f0";
        const BUDGET: u64 = 4096;

        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("test.redb"))?;
        let file = |kind, version, record: Value| {
            let header = json!({
                "nrtm_version": 4, "type": kind, "source": SOURCE,
                "session_id": SESSION, "version": version,
            });
            format!("\x1e{header}\n\x1e{record}\n")
        };
        let snapshot = file(
            "snapshot",
            1,
            json!({"object": "aut-num: AS64496\nsource: TEST\n"}),
        );
        let delta = file(
            "delta",
            2,
            json!({"action": "add_modify", "object": "aut-num: AS64497\nsource: TEST\n"}),
        );
        let reference = |url, version, file: &str| {
            let hash: String = Sha256::digest(file.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            json!({"version": version, "url": url, "hash": hash})
        };
        let now = time::OffsetDateTime::now_utc();
        let timestamp = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
        );
        let notification = json!({
            "nrtm_version": 4, "type": "notification", "source": SOURCE,
            "session_id": SESSION, "version": 2, "timestamp": timestamp,
            "snapshot": reference("snapshot.json", 1, &snapshot),
            "deltas": [reference("delta.json", 2, &delta)],
        });
        let signing_key = SigningKey::from_slice(&[7; 32])?;
        let key = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)?;
        let body = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&notification)?),
        );
        let signature: Signature = signing_key.sign(body.as_bytes());
        let compact = format!("{body}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()));

        let run = |delta_bytes: &[u8]| {
            let mut expected = [
                ("snapshot.json", BUDGET, snapshot.as_bytes()),
                ("delta.json", BUDGET - snapshot.len() as u64, delta_bytes),
            ]
            .into_iter();
            let result = apply_remote(
                &store,
                SOURCE,
                compact.as_bytes(),
                Some(key.as_bytes()),
                BUDGET,
                |reference, remaining| {
                    let (url, budget, raw) = expected.next().expect("unexpected reference");
                    assert_eq!(reference.url, url);
                    assert_eq!(remaining, budget);
                    let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
                    file.write_all(raw)?;
                    file.flush()?;
                    Ok(file.into_temp_path())
                },
            );
            assert!(expected.next().is_none());
            result
        };

        let sources = [SOURCE.to_owned()];
        let error = run(b"corrupted delta").unwrap_err();
        assert!(error.to_string().contains("hash does not match"));
        assert!(store.nrtm4_state(SOURCE)?.is_none());
        assert!(store.exact("AS64496", &sources, 2)?.is_empty());
        assert!(store.exact("AS64497", &sources, 2)?.is_empty());

        let result = run(delta.as_bytes())?;
        assert_eq!(result["records"], 2);
        assert_eq!(store.nrtm4_state(SOURCE)?.unwrap().version, 2);
        assert_eq!(store.exact("AS64496", &sources, 2)?.len(), 1);
        assert_eq!(store.exact("AS64497", &sources, 2)?.len(), 1);
        Ok(())
    }
}
