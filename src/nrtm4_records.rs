//! Offline reader for the pinned NRTMv4 snapshot and delta file profile.
//!
//! The caller verifies the file hash against the signed notification before
//! supplying the decoded bytes. Consume all records before accepting the file.

use std::io::BufRead;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, de::DeserializeOwned};

use crate::nrtm4::is_uuid_v4;
use crate::rpsl::{MAX_OBJECT_BYTES, Object, parse};

const RS: u8 = 0x1e;
// A JSON string can escape a single ASCII byte as six bytes.
const MAX_RECORD_BYTES: usize = 6 * MAX_OBJECT_BYTES + 4096;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Snapshot,
    Delta,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Record {
    Snapshot(Object),
    AddModify(Object),
    Delete { class: String, key: String },
}

pub struct Reader<R> {
    sequence: Sequence<R>,
    kind: FileKind,
    source: String,
    records: u64,
    finished: bool,
}

impl<R: BufRead> Reader<R> {
    pub fn new(
        input: R,
        kind: FileKind,
        expected_source: &str,
        expected_session: &str,
        expected_version: u64,
        max_bytes: u64,
    ) -> Result<Self> {
        ensure!(
            is_uuid_v4(expected_session),
            "invalid expected NRTMv4 session ID"
        );
        ensure!(
            expected_version > 0 && (kind == FileKind::Snapshot || expected_version > 1),
            "invalid expected NRTMv4 file version"
        );
        ensure!(
            !expected_source.is_empty()
                && expected_source.bytes().all(|byte| {
                    byte.is_ascii_uppercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                }),
            "invalid expected NRTMv4 source"
        );

        let mut sequence = Sequence {
            input,
            remaining: max_bytes,
            buffer: Vec::new(),
        };
        let data = sequence.read()?.context("missing NRTMv4 file header")?;
        let header: Header = decode(data).context("invalid NRTMv4 file header")?;
        ensure!(
            header.nrtm_version == 4,
            "NRTMv4 header protocol version mismatch"
        );
        ensure!(header.kind == kind, "NRTMv4 header file type mismatch");
        ensure!(
            header.source == expected_source,
            "NRTMv4 header source mismatch"
        );
        ensure!(
            is_uuid_v4(&header.session_id)
                && header.session_id.eq_ignore_ascii_case(expected_session),
            "NRTMv4 header session ID mismatch"
        );
        ensure!(
            header.version == expected_version,
            "NRTMv4 header version mismatch"
        );

        Ok(Self {
            sequence,
            kind,
            source: header.source,
            records: 0,
            finished: false,
        })
    }

    fn read(&mut self) -> Result<Option<Record>> {
        let Some(data) = self.sequence.read()? else {
            ensure!(
                self.kind != FileKind::Delta || self.records != 0,
                "empty NRTMv4 delta"
            );
            return Ok(None);
        };
        let record = match self.kind {
            FileKind::Snapshot => {
                let entry: SnapshotEntry = decode(data)?;
                Record::Snapshot(parse_object(&entry.object, &self.source)?)
            }
            FileKind::Delta => match decode(data)? {
                DeltaEntry::AddModify { object } => {
                    Record::AddModify(parse_object(&object, &self.source)?)
                }
                DeltaEntry::Delete {
                    object_class,
                    primary_key,
                } => delete_identity(&self.source, &object_class, &primary_key)?,
            },
        };
        self.records += 1;
        Ok(Some(record))
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.read() {
            Ok(Some(record)) => Some(Ok(record)),
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                self.finished = true;
                Some(Err(
                    error.context(format!("NRTMv4 record {}", self.records + 2))
                ))
            }
        }
    }
}

#[derive(Deserialize)]
struct Header {
    nrtm_version: u8,
    #[serde(rename = "type")]
    kind: FileKind,
    source: String,
    session_id: String,
    version: u64,
}

#[derive(Deserialize)]
struct SnapshotEntry {
    object: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum DeltaEntry {
    AddModify {
        object: String,
    },
    Delete {
        object_class: String,
        primary_key: String,
    },
}

struct Sequence<R> {
    input: R,
    remaining: u64,
    buffer: Vec<u8>,
}

impl<R: BufRead> Sequence<R> {
    fn read(&mut self) -> Result<Option<&str>> {
        let start = self.input.fill_buf()?.first().copied();
        let Some(start) = start else {
            return Ok(None);
        };
        ensure!(
            self.remaining != 0,
            "NRTMv4 file exceeds the configured byte limit"
        );
        ensure!(start == RS, "missing NRTMv4 JSON record separator");
        self.input.consume(1);
        self.remaining -= 1;
        self.buffer.clear();

        loop {
            let input = self.input.fill_buf()?;
            if input.is_empty() {
                break;
            }
            // Inspect at most one byte beyond either limit to distinguish a
            // record boundary from an oversized record without scanning ahead.
            let file_window =
                usize::try_from(self.remaining.saturating_add(1)).unwrap_or(usize::MAX);
            let window = (MAX_RECORD_BYTES - self.buffer.len() + 1).min(file_window);
            let input = &input[..input.len().min(window)];
            let next_record = input.iter().position(|&byte| byte == RS);
            let count = next_record.unwrap_or(input.len());
            ensure!(
                count as u64 <= self.remaining,
                "NRTMv4 file exceeds the configured byte limit"
            );
            ensure!(
                self.buffer.len() + count <= MAX_RECORD_BYTES,
                "NRTMv4 JSON record exceeds {MAX_RECORD_BYTES} bytes"
            );
            self.buffer.extend_from_slice(&input[..count]);
            self.input.consume(count);
            self.remaining -= count as u64;
            if next_record.is_some() {
                break;
            }
        }
        ensure!(
            self.buffer.last() == Some(&b'\n'),
            "NRTMv4 JSON record lacks a final line feed"
        );
        let text =
            std::str::from_utf8(&self.buffer).context("invalid UTF-8 in NRTMv4 JSON record")?;
        ensure!(
            text.trim_ascii_start().starts_with('{'),
            "NRTMv4 JSON record must be an object"
        );
        Ok(Some(text))
    }
}

fn decode<T: DeserializeOwned>(data: &str) -> Result<T> {
    // Serde messages can contain untrusted attribute data. Only report location.
    serde_json::from_str(data).map_err(|error| {
        anyhow!(
            "invalid NRTMv4 JSON fields at line {}, column {}",
            error.line(),
            error.column()
        )
    })
}

fn parse_object(text: &str, source: &str) -> Result<Object> {
    let object = parse(text).map_err(anyhow::Error::msg)?;
    ensure!(object.source == source, "NRTMv4 object source mismatch");
    Ok(object)
}

fn delete_identity(source: &str, class: &str, key: &str) -> Result<Record> {
    ensure!(
        !class.is_empty()
            && class.len() <= 32
            && class
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !key.is_empty()
            && key.len() <= MAX_OBJECT_BYTES
            && !key
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\t'),
        "invalid NRTMv4 delete identity"
    );
    let class = class.to_ascii_lowercase();
    let key = key.to_uppercase();
    let text = match class.as_str() {
        "route" | "route6" => {
            let (prefix, origin) = key
                .rsplit_once("AS")
                .context("invalid NRTMv4 route identity")?;
            format!("{class}: {prefix}\norigin: AS{origin}\nsource: {source}\n")
        }
        "person" | "role" => {
            format!("{class}: NRTMv4 delete identity\nnic-hdl: {key}\nsource: {source}\n")
        }
        _ => format!("{class}: {key}\nsource: {source}\n"),
    };
    let object = parse_object(&text, source).context("invalid NRTMv4 delete identity")?;
    ensure!(
        object.class == class && object.key == key,
        "non-canonical NRTMv4 delete identity"
    );
    Ok(Record::Delete { class, key })
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;

    use serde_json::{Value, json};

    use super::*;

    const SESSION: &str = "52bfd73d-3309-43ee-93c7-3d28698284f0";

    fn header(kind: &str) -> Value {
        json!({
            "nrtm_version": 4,
            "type": kind,
            "source": "TEST",
            "session_id": SESSION,
            "version": 5,
        })
    }

    fn sequence(records: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.push(RS);
            serde_json::to_writer_pretty(&mut bytes, record).unwrap();
            bytes.push(b'\n');
        }
        bytes
    }

    fn read(bytes: &[u8], kind: FileKind) -> Result<Vec<Record>> {
        Reader::new(bytes, kind, "TEST", SESSION, 5, bytes.len() as u64)?.collect()
    }

    #[test]
    fn reads_pretty_snapshot_records_across_buffer_boundaries() -> Result<()> {
        let bytes = sequence(&[
            header("snapshot"),
            json!({"object": "route6: 2001:db8::/32\norigin: AS64496\nsource: TEST\n"}),
            json!({"object": "aut-num: AS64496\nsource: TEST"}),
        ]);
        let input = BufReader::with_capacity(3, bytes.as_slice());
        let records: Vec<_> = Reader::new(
            input,
            FileKind::Snapshot,
            "TEST",
            SESSION,
            5,
            bytes.len() as u64,
        )?
        .collect::<Result<_>>()?;
        assert_eq!(records.len(), 2);
        assert!(
            matches!(&records[0], Record::Snapshot(object) if object.key == "2001:DB8::/32AS64496")
        );
        assert!(matches!(&records[1], Record::Snapshot(object) if object.key == "AS64496"));
        assert!(read(&sequence(&[header("snapshot")]), FileKind::Snapshot)?.is_empty());
        Ok(())
    }

    #[test]
    fn keeps_delta_order_and_normalises_exact_delete_identities() -> Result<()> {
        let bytes = sequence(&[
            header("delta"),
            json!({"action":"add_modify","object":"route: 192.0.2.0/24\norigin: AS64496\nsource: TEST"}),
            json!({"action":"delete","object_class":"ROUTE","primary_key":"192.0.2.0/24as64496"}),
            json!({"action":"delete","object_class":"person","primary_key":"Nóc@example.net"}),
        ]);
        let records = read(&bytes, FileKind::Delta)?;
        assert!(
            matches!(&records[0], Record::AddModify(object) if object.key == "192.0.2.0/24AS64496")
        );
        assert_eq!(
            records[1],
            Record::Delete {
                class: "route".into(),
                key: "192.0.2.0/24AS64496".into()
            }
        );
        assert_eq!(
            records[2],
            Record::Delete {
                class: "person".into(),
                key: "NÓC@EXAMPLE.NET".into()
            }
        );
        assert!(read(&sequence(&[header("delta")]), FileKind::Delta).is_err());
        Ok(())
    }

    #[test]
    fn rejects_wrong_headers_sources_and_identities() {
        for (field, wrong) in [
            ("source", json!("OTHER")),
            ("version", json!(6)),
            ("type", json!("delta")),
            ("nrtm_version", json!(3)),
            ("session_id", json!("52bfd73d-3309-43ee-93c7-3d28698284f1")),
            ("session_id", json!("52bfd73d-3309-13ee-93c7-3d28698284f0")),
        ] {
            let mut wrong_header = header("snapshot");
            wrong_header[field] = wrong;
            assert!(
                read(&sequence(&[wrong_header]), FileKind::Snapshot).is_err(),
                "{field}"
            );
        }

        let mixed = sequence(&[
            header("snapshot"),
            json!({"object":"aut-num: AS64496\nsource: OTHER"}),
        ]);
        assert!(read(&mixed, FileKind::Snapshot).is_err());
        for bad in [
            json!({"action":"add_modify","object":"aut-num: AS64496\nsource: OTHER"}),
            json!({"action":"delete","object_class":"aut-num","primary_key":"AS064496"}),
            json!({"action":"delete","object_class":"route","primary_key":"192.0.2.0/24"}),
            json!({"action":"delete","object_class":"route","primary_key":"192.0.2.1/24AS64496"}),
            json!({"action":"delete","object_class":"role","primary_key":"X\nsource: OTHER"}),
            json!({"action":"delete","object_class":"unknown","primary_key":"EXAMPLE"}),
            json!({"action":"delete","object_class":"aut-num"}),
        ] {
            assert!(read(&sequence(&[header("delta"), bad]), FileKind::Delta).is_err());
        }
    }

    #[test]
    fn rejects_truncation_invalid_utf8_and_oversized_records() {
        let valid = sequence(&[
            header("snapshot"),
            json!({"object":"aut-num: AS64496\nsource: TEST"}),
        ]);
        assert!(read(&valid[..valid.len() - 1], FileKind::Snapshot).is_err());
        assert!(read(&valid[..valid.len() - 2], FileKind::Snapshot).is_err());
        assert!(read(&valid[1..], FileKind::Snapshot).is_err());
        let mut corrupt = valid.clone();
        let payload = corrupt.iter().rposition(|byte| *byte == RS).unwrap();
        corrupt[payload + 2] = 0xff;
        assert!(read(&corrupt, FileKind::Snapshot).is_err());
        let mut reader = Reader::new(
            valid.as_slice(),
            FileKind::Snapshot,
            "TEST",
            SESSION,
            5,
            valid.len() as u64 - 1,
        )
        .unwrap();
        assert!(
            reader
                .next()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("record 2")
        );
        assert!(reader.next().is_none());

        let oversized = sequence(&[
            header("snapshot"),
            json!({"object": "x".repeat(MAX_RECORD_BYTES)}),
        ]);
        let error = read(&oversized, FileKind::Snapshot).unwrap_err();
        assert!(format!("{error:#}").contains("JSON record exceeds"));
    }

    #[test]
    fn malformed_record_never_resynchronises_to_following_data() -> Result<()> {
        let mut bytes = sequence(&[header("snapshot")]);
        bytes.extend_from_slice(b"\x1e{}\n");
        bytes.extend_from_slice(&sequence(&[
            json!({"object":"aut-num: AS64496\nsource: TEST"}),
        ]));
        let mut reader = Reader::new(
            bytes.as_slice(),
            FileKind::Snapshot,
            "TEST",
            SESSION,
            5,
            bytes.len() as u64,
        )?;
        assert!(reader.next().unwrap().is_err());
        assert!(reader.next().is_none());
        Ok(())
    }
}
