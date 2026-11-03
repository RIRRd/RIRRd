//! Apply a signed notification using explicitly supplied, local reference files.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    nrtm4::{MAX_NOTIFICATION_BYTES, MAX_PUBLIC_KEY_BYTES},
    nrtm4_local::{open_verified_file, read_small},
    nrtm4_records::FileKind,
    nrtm4_state::{Apply, Trust, plan},
    storage::Store,
};

const MAX_FILES: usize = 64;

pub fn apply(
    store: &Store,
    source: &str,
    notification_path: &Path,
    public_key: Option<&Path>,
    paths: &[PathBuf],
    max_bytes: u64,
) -> Result<Value> {
    ensure!(
        paths.len() <= MAX_FILES,
        "too many NRTMv4 files for one application"
    );
    let committed = store.nrtm4_state(source)?;
    let key = public_key
        .map(|path| read_small(path, MAX_PUBLIC_KEY_BYTES))
        .transpose()?;
    let trust = if let Some(committed) = &committed {
        ensure!(key.is_none(), "NRTMv4 source already has a trusted key");
        Trust::Established(committed)
    } else {
        let key = key
            .as_deref()
            .context("first NRTMv4 apply requires --public-key")?;
        let key_pem = std::str::from_utf8(key).context("public key must be a PEM file")?;
        Trust::Initial { source, key_pem }
    };
    let compact = read_small(notification_path, MAX_NOTIFICATION_BYTES)?;
    let transition = plan(trust, &compact, SystemTime::now())?;
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
    ensure!(
        selected.len() == paths.len(),
        "signed plan needs {} local files in snapshot-then-delta order, received {}",
        selected.len(),
        paths.len()
    );
    let mut staged = Vec::new();
    let mut remaining = max_bytes;
    let decoded = Rc::new(Cell::new(max_bytes));
    for ((kind, reference), path) in selected.into_iter().zip(paths) {
        let (reader, bytes) = open_verified_file(
            path,
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
