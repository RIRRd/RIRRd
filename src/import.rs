use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use flate2::bufread::MultiGzDecoder;

use crate::rpsl::{MAX_SNAPSHOT_OBJECT_BYTES as MAX_OBJECT_BYTES, Object, parse_snapshot};

pub fn snapshot(path: &Path, max_bytes: u64) -> Result<impl Iterator<Item = Result<Object>>> {
    let file = File::open(path).context("opening snapshot")?;
    let mut file = BufReader::new(file);
    let reader: Box<dyn Read> = if file.fill_buf()?.starts_with(&[0x1f, 0x8b]) {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut input = BufReader::new(reader.take(max_bytes + 1));
    let mut remaining = max_bytes;
    let mut record = 0;
    let mut finished = false;

    Ok(std::iter::from_fn(move || {
        if finished {
            return None;
        }
        let result = next_record(&mut input, &mut remaining).and_then(|raw| {
            let Some(raw) = raw else {
                ensure!(record > 0, "snapshot has no objects");
                return Ok(None);
            };
            record += 1;
            let object = parse_snapshot(&raw)
                .map_err(|error| anyhow!("snapshot record {record}: {error}"))?;
            Ok(Some(object))
        });
        finished = !matches!(result, Ok(Some(_)));
        result.transpose()
    }))
}

fn next_record(input: &mut impl BufRead, remaining: &mut u64) -> Result<Option<String>> {
    let mut raw = String::new();
    loop {
        let mut line = String::new();
        let read = input
            .take((MAX_OBJECT_BYTES + 1) as u64)
            .read_line(&mut line)
            .context("reading UTF-8 snapshot")?;
        ensure!(
            read as u64 <= *remaining,
            "snapshot exceeds decompressed byte limit"
        );
        *remaining -= read as u64;
        ensure!(
            read <= MAX_OBJECT_BYTES,
            "snapshot line exceeds object byte limit"
        );
        if read == 0 {
            return Ok((!raw.is_empty()).then_some(raw));
        }
        if matches!(line.as_str(), "\n" | "\r\n") {
            if !raw.is_empty() {
                return Ok(Some(raw));
            }
        } else if !raw.is_empty() || !line.starts_with(['%', '#']) {
            ensure!(
                raw.len() + line.len() <= MAX_OBJECT_BYTES,
                "snapshot object exceeds byte limit"
            );
            raw.push_str(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn snapshot_accepts_large_policy_without_relaxing_submission_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("policy.rpsl");
        let text = format!(
            "filter-set: FLTR-EXAMPLE\nfilter: {}\nsource: TEST\n",
            "AS64496 OR ".repeat(450_000) + "AS64497"
        );
        assert!(text.len() > 4 * 1024 * 1024);
        assert!(text.len() > crate::rpsl::MAX_OBJECT_BYTES);
        assert!(crate::rpsl::parse(&text).is_err());
        std::fs::write(&path, &text)?;
        let objects = snapshot(&path, text.len() as u64)?.collect::<Result<Vec<_>>>()?;
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].key, "FLTR-EXAMPLE");
        assert_eq!(objects[0].text, text);
        assert!(crate::rpsl::lookup_attributes(&objects[0].class, &objects[0].text).is_ok());
        let excessive = format!(
            "filter-set: FLTR-EXAMPLE\nfilter: {}\nsource: TEST\n",
            "A".repeat(MAX_OBJECT_BYTES)
        );
        std::fs::write(&path, &excessive)?;
        assert!(
            snapshot(&path, excessive.len() as u64)?
                .collect::<Result<Vec<_>>>()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn bounds_gzip_stream_and_detects_corrupt_trailer() -> Result<()> {
        let raw = b"aut-num: AS64496\nsource: TEST\n";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(raw)?;
        let mut gzip = encoder.finish()?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("snapshot");
        std::fs::write(&path, &gzip)?;
        assert_eq!(
            snapshot(&path, raw.len() as u64)?
                .collect::<Result<Vec<_>>>()?
                .len(),
            1
        );
        assert!(
            snapshot(&path, raw.len() as u64 - 1)?
                .collect::<Result<Vec<_>>>()
                .is_err()
        );
        let last = gzip.len() - 1;
        gzip[last] ^= 1;
        std::fs::write(&path, gzip)?;
        assert!(
            snapshot(&path, raw.len() as u64)?
                .collect::<Result<Vec<_>>>()
                .is_err()
        );
        Ok(())
    }
}
