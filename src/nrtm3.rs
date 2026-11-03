//! Streaming reader for finite, contiguous NRTMv3 transcripts.

use std::io::{BufRead, Read};

use crate::rpsl::{MAX_OBJECT_BYTES, MAX_SNAPSHOT_OBJECT_BYTES, Object, parse_snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Add,
    Delete,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Operation {
    pub kind: Kind,
    pub serial: u64,
    pub object: Object,
}

/// Read one finite NRTMv3 response. Consume the iterator to exhaustion before
/// committing changes, since the terminal read validates the trailer and EOF.
pub struct Reader<R> {
    lines: Lines<R>,
    source: String,
    range: (u64, u64),
    next_serial: Option<u64>,
    finished: bool,
    allow_gaps: bool,
}

impl<R: BufRead> Reader<R> {
    pub fn new(reader: R, expected_source: &str, max_bytes: u64) -> Result<Self, String> {
        Self::with_profile(reader, expected_source, max_bytes, false)
    }

    /// Published v3 feeds may omit serials for withheld classes. Retain their
    /// advertised cursor, without exposing an incomplete downstream journal.
    pub fn filtered(reader: R, expected_source: &str, max_bytes: u64) -> Result<Self, String> {
        Self::with_profile(reader, expected_source, max_bytes, true)
    }

    fn with_profile(
        reader: R,
        expected_source: &str,
        max_bytes: u64,
        allow_gaps: bool,
    ) -> Result<Self, String> {
        let mut lines = Lines {
            reader,
            remaining: max_bytes,
            number: 0,
            max_object_bytes: if allow_gaps {
                MAX_SNAPSHOT_OBJECT_BYTES
            } else {
                MAX_OBJECT_BYTES
            },
        };
        let header = lines.next_framing()?.ok_or("missing NRTMv3 START header")?;
        let (source, range) = parse_header(logical_line(&header))?;
        if !source.eq_ignore_ascii_case(expected_source) {
            return Err("NRTMv3 header source does not match configured source".into());
        }
        Ok(Self {
            lines,
            source: source.to_ascii_uppercase(),
            range,
            next_serial: Some(range.0),
            finished: false,
            allow_gaps,
        })
    }

    pub fn range(&self) -> (u64, u64) {
        self.range
    }

    fn read_operation(&mut self, expected_serial: u64) -> Result<Operation, String> {
        let header = self
            .lines
            .next_framing()?
            .ok_or("missing NRTMv3 operation")?;
        self.read_operation_header(expected_serial, header)
    }

    fn read_operation_header(
        &mut self,
        expected_serial: u64,
        header: String,
    ) -> Result<Operation, String> {
        let mut fields = logical_line(&header).split_ascii_whitespace();
        let kind = match fields.next() {
            Some("ADD") => Kind::Add,
            Some("DEL") => Kind::Delete,
            _ => {
                return Err(format!(
                    "expected NRTMv3 operation at serial {expected_serial}"
                ));
            }
        };
        let actual_serial = fields
            .next()
            .and_then(parse_serial)
            .filter(|_| fields.next().is_none())
            .ok_or("invalid NRTMv3 operation serial")?;
        if actual_serial > self.range.1
            || (if self.allow_gaps {
                actual_serial < expected_serial
            } else {
                expected_serial != actual_serial
            })
        {
            return Err(format!(
                "NRTMv3 serial {actual_serial} is not the expected serial {expected_serial}"
            ));
        }
        if !self
            .lines
            .next()?
            .is_some_and(|line| logical_line(&line).is_empty())
        {
            return Err(format!(
                "NRTMv3 operation at serial {actual_serial} is missing its blank separator"
            ));
        }

        let mut text = String::new();
        while let Some(line) = self.lines.next()? {
            let body = logical_line(&line);
            if body.is_empty() {
                if text.is_empty() {
                    continue;
                }
                break;
            }
            if matches!(directive(body), Some(("START" | "END" | "ERROR", _))) {
                return Err(format!(
                    "unexpected NRTMv3 directive in object at serial {actual_serial}"
                ));
            }
            if text.len() + line.len() > self.lines.max_object_bytes {
                return Err(format!(
                    "NRTMv3 object at serial {actual_serial} exceeds {} bytes",
                    self.lines.max_object_bytes
                ));
            }
            text.push_str(&line);
        }
        if text.is_empty() {
            return Err(format!("missing NRTMv3 object at serial {actual_serial}"));
        }
        if kind == Kind::Delete && !has_source(&text) {
            // The pinned IRRd parser accepts a DEL containing only its key.
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("source: ");
            text.push_str(&self.source);
            text.push('\n');
        }
        let object = parse_snapshot(&text)
            .map_err(|error| format!("NRTMv3 object at serial {actual_serial}: {error}"))?;
        if object.source != self.source {
            return Err(format!(
                "NRTMv3 object source mismatch at serial {actual_serial}"
            ));
        }
        Ok(Operation {
            kind,
            serial: actual_serial,
            object,
        })
    }

    fn read_end(&mut self) -> Result<(), String> {
        let end = self
            .lines
            .next_framing()?
            .ok_or("missing NRTMv3 END trailer")?;
        self.validate_end(end)
    }

    fn validate_end(&mut self, end: String) -> Result<(), String> {
        let Some(("END", source)) = directive(logical_line(&end)) else {
            return Err("expected NRTMv3 END trailer after the final serial".into());
        };
        if !source.trim_ascii().eq_ignore_ascii_case(&self.source) {
            return Err("NRTMv3 trailer source does not match configured source".into());
        }
        while let Some(line) = self.lines.next()? {
            if !logical_line(&line).is_empty() {
                return Err("unexpected data after the NRTMv3 END trailer".into());
            }
        }
        Ok(())
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Result<Operation, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if self.allow_gaps {
            let header = match self.lines.next_framing() {
                Ok(Some(header)) => header,
                Ok(None) => {
                    self.finished = true;
                    return Some(Err("missing NRTMv3 END trailer".into()));
                }
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            };
            if matches!(directive(logical_line(&header)), Some(("END", _))) {
                self.finished = true;
                return self.validate_end(header).err().map(Err);
            }
            let Some(expected) = self.next_serial else {
                self.finished = true;
                return Some(Err("operation exceeds advertised NRTMv3 range".into()));
            };
            let operation = self.read_operation_header(expected, header);
            match &operation {
                Ok(operation) => {
                    self.next_serial =
                        (operation.serial < self.range.1).then(|| operation.serial + 1)
                }
                Err(_) => self.finished = true,
            }
            return Some(operation);
        }
        let Some(serial) = self.next_serial else {
            self.finished = true;
            return self.read_end().err().map(Err);
        };
        let operation = self.read_operation(serial);
        if operation.is_ok() {
            self.next_serial = (serial < self.range.1).then(|| serial + 1);
        } else {
            self.finished = true;
        }
        Some(operation)
    }
}

struct Lines<R> {
    reader: R,
    remaining: u64,
    number: u64,
    max_object_bytes: usize,
}

impl<R: BufRead> Lines<R> {
    fn next(&mut self) -> Result<Option<String>, String> {
        let mut line = String::new();
        let limit = ((self.max_object_bytes + 1) as u64).min(self.remaining.saturating_add(1));
        let count = (&mut self.reader)
            .take(limit)
            .read_line(&mut line)
            .map_err(|_| format!("could not read UTF-8 NRTMv3 line {}", self.number + 1))?;
        if count == 0 {
            return Ok(None);
        }
        self.number += 1;
        if count as u64 > self.remaining {
            return Err("NRTMv3 input exceeds configured byte limit".into());
        }
        self.remaining -= count as u64;
        if count > self.max_object_bytes {
            return Err(format!(
                "NRTMv3 line {} exceeds {} bytes",
                self.number, self.max_object_bytes
            ));
        }
        if logical_line(&line)
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(format!(
                "invalid control character on NRTMv3 line {}",
                self.number
            ));
        }
        Ok(Some(line))
    }

    fn next_framing(&mut self) -> Result<Option<String>, String> {
        while let Some(line) = self.next()? {
            let body = logical_line(&line);
            match directive(body) {
                Some(("ERROR", _)) => return Err("NRTMv3 response contains a server error".into()),
                Some(("START" | "END", _)) => return Ok(Some(line)),
                _ if body.is_empty() || body.starts_with(['%', '#']) => {}
                _ => return Ok(Some(line)),
            }
        }
        Ok(None)
    }
}

fn logical_line(line: &str) -> &str {
    line.strip_suffix('\n')
        .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line))
}

fn directive(line: &str) -> Option<(&'static str, &str)> {
    let body = line
        .strip_prefix('%')?
        .trim_start_matches('%')
        .trim_ascii_start();
    let end = body
        .find(|character: char| character.is_ascii_whitespace() || character == ':')
        .unwrap_or(body.len());
    let (keyword, value) = body.split_at(end);
    ["START", "END", "ERROR"]
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(keyword))
        .map(|known| (known, value.trim_ascii_start()))
}

fn parse_header(header: &str) -> Result<(&str, (u64, u64)), String> {
    let Some(("START", header)) = directive(header) else {
        return Err("missing NRTMv3 START header".into());
    };
    let mut fields = header.split_ascii_whitespace();
    if fields.next() != Some("Version:") || fields.next() != Some("3") {
        return Err("NRTM START header must declare version 3".into());
    }
    let source = fields.next().ok_or("missing NRTMv3 header source")?;
    // RIPE documents a colon in its example. IRRd emits a space.
    let (source, range) = if let Some((source, range)) = source.split_once(':') {
        (source, range)
    } else {
        (source, fields.next().ok_or("missing NRTMv3 header range")?)
    };
    if fields.next().is_some() {
        return Err("unsupported flags in NRTMv3 START header".into());
    }
    if source.is_empty()
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid NRTMv3 header source".into());
    }
    let (first, last) = range
        .split_once('-')
        .and_then(|(first, last)| Some((parse_serial(first)?, parse_serial(last)?)))
        .ok_or("invalid NRTMv3 header range")?;
    if first > last {
        return Err("reversed NRTMv3 header range".into());
    }
    Ok((source, (first, last)))
}

fn parse_serial(value: &str) -> Option<u64> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|serial| *serial > 0)
}

fn has_source(text: &str) -> bool {
    text.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("source"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    const VALID: &str = concat!(
        "% A test transcript\r\n",
        "%START Version: 3 test 41-42\r\n\r\n",
        "ADD 41\r\n\r\n",
        "route: 192.0.2.0/24\r\n",
        "origin: AS64496\r\n",
        "remarks: a folded attribute\r\n",
        "+ continuation\r\n",
        "source: TEST\r\n\r\n",
        "DEL 42\r\n\r\n",
        "aut-num: AS64497\r\n\r\n",
        "%END TEST",
    );

    fn parse_all(input: &str) -> Result<Vec<Operation>, String> {
        Reader::new(input.as_bytes(), "TEST", input.len() as u64)?.collect()
    }

    #[test]
    fn streams_contiguous_operations_and_defaults_source_for_delete() {
        let input = BufReader::with_capacity(3, VALID.as_bytes());
        let mut reader = Reader::new(input, "TEST", VALID.len() as u64).unwrap();
        assert_eq!(reader.range(), (41, 42));
        let add = reader.next().unwrap().unwrap();
        assert_eq!((add.kind, add.serial), (Kind::Add, 41));
        assert_eq!(add.object.key, "192.0.2.0/24AS64496");
        assert!(add.object.text.contains("+ continuation\r\n"));
        let delete = reader.next().unwrap().unwrap();
        assert_eq!((delete.kind, delete.serial), (Kind::Delete, 42));
        assert_eq!(delete.object.key, "AS64497");
        assert_eq!(delete.object.source, "TEST");
        assert!(reader.next().is_none());
        assert!(reader.next().is_none());
    }

    #[test]
    fn handles_colon_header_and_maximum_serial_without_wrapping() {
        let input = format!(
            "%START Version: 3 TEST:{0}-{0}\n\nDEL {0}\n\naut-num: AS64496\n\n%END TEST\n",
            u64::MAX
        );
        let operations = parse_all(&input).unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].serial, u64::MAX);
    }

    #[test]
    fn filtered_profile_accepts_upstream_omissions_and_large_bounded_objects() {
        let input = "%START Version: 3 TEST 10-15\n\nADD 11\n\naut-num: AS64496\nsource: TEST\n\nDEL 13\n\naut-num: AS64497\nsource: TEST\n\n%END TEST\n";
        let operations = Reader::filtered(input.as_bytes(), "TEST", 4096)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            operations.iter().map(|op| op.serial).collect::<Vec<_>>(),
            vec![11, 13]
        );
        for bad in [
            input.replace("DEL 13", "DEL 11"),
            input.replace("DEL 13", "DEL 16"),
            input.replace("%END TEST", "%END OTHER"),
            input.replace("%END TEST", ""),
            input.replace("source: TEST", "source: OTHER"),
        ] {
            assert!(
                Reader::filtered(bad.as_bytes(), "TEST", 4096)
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .is_err()
            );
        }
        let empty = "%START Version: 3 TEST 10-15\n\n%END TEST\n";
        assert!(
            Reader::filtered(empty.as_bytes(), "TEST", 4096)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .is_empty()
        );
        let large = format!(
            "%START Version: 3 TEST 10-10\n\nADD 10\n\naut-num: AS64496\nremarks: {}\nsource: TEST\n\n%END TEST\n",
            "x".repeat(MAX_OBJECT_BYTES + 1)
        );
        assert!(
            Reader::new(large.as_bytes(), "TEST", 200000)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .is_err()
        );
        assert_eq!(
            Reader::filtered(large.as_bytes(), "TEST", 200000)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len(),
            1
        );
        assert!(
            Reader::filtered(large.as_bytes(), "TEST", 4000)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .is_err()
        );
    }

    #[test]
    fn rejects_non_contiguous_serials_and_non_strict_headers() {
        for input in [
            VALID.replace("ADD 41", "ADD 42"),
            VALID.replace("DEL 42", "DEL 41"),
            VALID.replace("DEL 42", "DEL 43"),
            VALID.replace("DEL 42", "DEL"),
            VALID.replace("41-42", "42-41"),
            VALID.replace("41-42", "0-42"),
            VALID.replace("41-42", "41-42 FILTERED"),
            VALID.replace("Version: 3", "Version: 1"),
            VALID.replace("41-42", "41-43"),
        ] {
            assert!(parse_all(&input).is_err());
        }
    }

    #[test]
    fn validates_the_complete_frame_and_object_sources() {
        for input in [
            VALID.replace("%END TEST", ""),
            VALID.replace("%END TEST", "%END OTHER"),
            VALID.replace("%END TEST", "%END TEST\r\n% another comment"),
            VALID.replace("%END TEST", "%END TEST\r\nDEL 43"),
            VALID.replace("%END TEST", "%ERROR:101: mirror denied"),
            VALID.replace("test 41-42", "other 41-42"),
            VALID.replace("source: TEST\r\n", "source: OTHER\r\n"),
            VALID.replace("source: TEST\r\n", ""),
            VALID.replace("aut-num: AS64497", "aut-num: AS64497\r\nsource: OTHER"),
            VALID.replace("ADD 41\r\n\r\n", "ADD 41\r\n"),
            VALID.replace("aut-num: AS64497\r\n\r\n", "aut-num: AS64497\r\n"),
        ] {
            assert!(parse_all(&input).is_err(), "{input}");
        }
    }

    #[test]
    fn enforces_byte_limits_and_utf8() {
        let result: Result<Vec<_>, _> =
            Reader::new(VALID.as_bytes(), "TEST", VALID.len() as u64 - 1)
                .unwrap()
                .collect();
        assert_eq!(
            result.unwrap_err(),
            "NRTMv3 input exceeds configured byte limit"
        );
        let oversized = VALID.replace(
            "remarks: a folded attribute",
            &format!("remarks: {}", "a".repeat(MAX_OBJECT_BYTES)),
        );
        assert!(parse_all(&oversized).unwrap_err().contains("line"));
        let oversized_object = VALID.replace(
            "remarks: a folded attribute",
            &format!("remarks: {}", "a".repeat(MAX_OBJECT_BYTES - 30)),
        );
        assert!(parse_all(&oversized_object).unwrap_err().contains("object"));

        let mut invalid = VALID.as_bytes().to_vec();
        let remark = VALID.find("remarks:").unwrap();
        invalid[remark + 9] = 0xff;
        let result: Result<Vec<_>, _> =
            Reader::new(invalid.as_slice(), "TEST", invalid.len() as u64)
                .unwrap()
                .collect();
        assert!(result.unwrap_err().contains("UTF-8"));
    }
}
