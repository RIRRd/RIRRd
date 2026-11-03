//! Offline verification of signed metadata and an explicitly supplied mirror file.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::Path,
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use flate2::bufread::MultiGzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    nrtm4::{self, FileReference, MAX_NOTIFICATION_BYTES, MAX_PUBLIC_KEY_BYTES},
    nrtm4_records::{FileKind, Reader, Record},
};

pub enum SelectedFile<'a> {
    Snapshot(&'a Path),
    Delta(&'a Path, u64),
}

pub fn verify(
    source: &str,
    notification: &Path,
    public_key: &Path,
    file: Option<SelectedFile<'_>>,
    max_bytes: u64,
) -> Result<Value> {
    let compact = read_small(notification, MAX_NOTIFICATION_BYTES)?;
    let key = read_small(public_key, MAX_PUBLIC_KEY_BYTES)?;
    let key = std::str::from_utf8(&key).context("public key must be a PEM file")?;
    let notification = nrtm4::verify(&compact, key, source, SystemTime::now())?;
    let file = match file {
        Some(SelectedFile::Snapshot(path)) => {
            Some((path, FileKind::Snapshot, &notification.snapshot))
        }
        Some(SelectedFile::Delta(path, version)) => {
            let reference = notification
                .deltas
                .iter()
                .find(|reference| reference.version == version)
                .context("delta version is absent from the signed notification")?;
            Some((path, FileKind::Delta, reference))
        }
        None => None,
    };
    let records = file
        .map(|(path, kind, reference)| {
            verify_file(
                path,
                reference,
                kind,
                &notification.source,
                &notification.session_id,
                max_bytes,
            )
        })
        .transpose()?;
    Ok(json!({ "notification": notification, "file": records }))
}

fn read_small(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "verification input exceeds {limit} bytes"
    );
    Ok(bytes)
}

fn verify_file(
    path: &Path,
    reference: &FileReference,
    kind: FileKind,
    source: &str,
    session: &str,
    max_bytes: u64,
) -> Result<Value> {
    let mut input = File::open(path).context("opening referenced NRTMv4 file")?;
    let mut staged = tempfile::tempfile().context("creating private verification file")?;
    let mut hasher = Sha256::new();
    let mut remaining = max_bytes;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            count as u64 <= remaining,
            "referenced NRTMv4 file exceeds the byte limit"
        );
        remaining -= count as u64;
        hasher.update(&buffer[..count]);
        staged.write_all(&buffer[..count])?;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    ensure!(
        digest == reference.sha256(),
        "NRTMv4 file hash does not match the signed notification"
    );
    staged.rewind()?;

    let mut input = BufReader::new(staged);
    let gzip = input.fill_buf()?.starts_with(&[0x1f, 0x8b]);
    ensure!(
        gzip == reference.url.ends_with(".gz"),
        "NRTMv4 compression does not match the signed filename"
    );
    let input: Box<dyn BufRead> = if gzip {
        Box::new(BufReader::new(MultiGzDecoder::new(input)))
    } else {
        Box::new(input)
    };
    let mut objects = 0_u64;
    let mut deletions = 0_u64;
    let mut identity_bytes = 0_u64;
    for record in Reader::new(input, kind, source, session, reference.version, max_bytes)? {
        match record? {
            Record::Snapshot(object) | Record::AddModify(object) => {
                objects += 1;
                identity_bytes += (object.class.len() + object.key.len()) as u64;
            }
            Record::Delete { class, key } => {
                deletions += 1;
                identity_bytes += (class.len() + key.len()) as u64;
            }
        }
    }
    Ok(
        json!({ "version": reference.version, "objects": objects, "deletions": deletions, "identity_bytes": identity_bytes }),
    )
}
