//! Offline backup of the complete redb registry, including application metadata.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::storage::Store;

const MAGIC: &[u8; 8] = b"RIRRDBAK";
const VERSION: u32 = 1;
const HEADER_BYTES: u64 = 8 + 4 + 8;
const CHECKSUM_BYTES: u64 = 32;

/// Create a versioned backup and return the number of raw database bytes copied.
pub fn create(database: &Path, output: &Path, cache_bytes: usize) -> Result<u64> {
    require_absent(output)?;
    let mut raw = OpenOptions::new()
        .read(true)
        .write(true)
        .open(database)
        .context("opening existing registry for backup")?;
    let metadata = raw.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "backup source must be a nonempty regular database file"
    );

    // A second open cannot read the locked bytes on Windows. Duplicate the
    // original handle and let redb acquire and retain its usual exclusive lock.
    let store =
        lock_existing_file(&raw, cache_bytes).context("locking existing registry for backup")?;
    store.verify().context("verifying registry before backup")?;
    raw.seek(SeekFrom::Start(0))?;
    let length = raw.metadata()?.len();
    let mut stage = stage_for(output)?;
    write_header(stage.as_file_mut(), length)?;
    let checksum = copy_payload(&mut raw, stage.as_file_mut(), length)?;
    require_eof(&mut raw).context("registry length changed while backing up")?;
    stage.as_file_mut().write_all(&checksum)?;
    publish(stage, output)?;
    drop(store);
    Ok(length)
}

/// Validate and restore a backup into a new file, returning its raw payload size.
pub fn restore(input: &Path, database: &Path, cache_bytes: usize) -> Result<u64> {
    require_absent(database)?;
    let mut archive = File::open(input).context("opening registry backup")?;
    let metadata = archive.metadata()?;
    ensure!(metadata.is_file(), "backup input must be a regular file");
    let length = read_header(&mut archive)?;
    let expected_length = HEADER_BYTES
        .checked_add(length)
        .and_then(|bytes| bytes.checked_add(CHECKSUM_BYTES))
        .context("backup length exceeds format limits")?;
    ensure!(
        metadata.len() == expected_length,
        "backup length does not match its header"
    );

    let mut stage = stage_for(database)?;
    let checksum = copy_payload(&mut archive, stage.as_file_mut(), length)?;
    let mut expected = [0; CHECKSUM_BYTES as usize];
    archive
        .read_exact(&mut expected)
        .context("reading backup checksum")?;
    require_eof(&mut archive).context("backup contains trailing bytes")?;
    ensure!(checksum == expected, "backup checksum mismatch");
    {
        let restored = lock_existing_file(stage.as_file(), cache_bytes)
            .context("opening staged restored database")?;
        restored
            .verify()
            .context("verifying staged restored registry")?;
    }
    publish(stage, database)?;
    Ok(length)
}

fn write_header(mut output: impl Write, length: u64) -> io::Result<()> {
    output.write_all(MAGIC)?;
    output.write_all(&VERSION.to_be_bytes())?;
    output.write_all(&length.to_be_bytes())
}

fn read_header(mut input: impl Read) -> Result<u64> {
    let mut magic = [0; 8];
    let mut version = [0; 4];
    let mut length = [0; 8];
    input
        .read_exact(&mut magic)
        .context("reading backup magic")?;
    ensure!(&magic == MAGIC, "invalid registry backup magic");
    input
        .read_exact(&mut version)
        .context("reading backup version")?;
    ensure!(
        u32::from_be_bytes(version) == VERSION,
        "unsupported registry backup version"
    );
    input
        .read_exact(&mut length)
        .context("reading backup length")?;
    let length = u64::from_be_bytes(length);
    ensure!(length > 0, "empty registry backup payload");
    Ok(length)
}

fn copy_payload(
    mut input: impl Read,
    mut output: impl Write,
    mut remaining: u64,
) -> Result<[u8; 32]> {
    let mut buffer = [0; 64 * 1024];
    let mut hash = Sha256::new();
    while remaining != 0 {
        let size = remaining.min(buffer.len() as u64) as usize;
        let chunk = &mut buffer[..size];
        input
            .read_exact(chunk)
            .context("reading registry backup payload")?;
        output
            .write_all(chunk)
            .context("writing registry backup payload")?;
        hash.update(chunk);
        remaining -= size as u64;
    }
    Ok(hash.finalize().into())
}

fn require_eof(mut input: impl Read) -> Result<()> {
    ensure!(
        input.read(&mut [0])? == 0,
        "unexpected bytes after expected length"
    );
    Ok(())
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("backup destination already exists"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("checking backup destination"),
    }
}

fn directory_for(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn stage_for(path: &Path) -> Result<NamedTempFile> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Allow publication by rename while excluding independent reads and writes.
        const FILE_SHARE_DELETE: u32 = 4;
        tempfile::Builder::new()
            .make_in(directory_for(path), |temporary| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .share_mode(FILE_SHARE_DELETE)
                    .open(temporary)
            })
            .context("creating private backup staging file")
    }
    #[cfg(not(windows))]
    NamedTempFile::new_in(directory_for(path)).context("creating private backup staging file")
}

fn lock_existing_file(file: &File, cache_bytes: usize) -> Result<Store> {
    ensure!(file.metadata()?.len() > 0, "empty database file");
    let db = redb::Database::builder()
        .set_cache_size(cache_bytes)
        .create_file(file.try_clone()?)?;
    Ok(Store::from_database(db))
}

fn publish(stage: NamedTempFile, path: &Path) -> Result<()> {
    stage
        .as_file()
        .sync_all()
        .context("syncing backup staging file")?;
    stage
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .context("publishing backup without overwriting a destination")?;
    #[cfg(unix)]
    File::open(directory_for(path))
        .and_then(|directory| directory.sync_all())
        .context("backup published, but syncing its parent directory failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        authoritative::Change,
        nrtm3::{Kind, Operation},
        nrtm4_records::Record,
        nrtm4_state::fixture_state,
        rpsl::{Object, parse},
    };

    use super::*;

    fn object(text: &str) -> Result<Object> {
        parse(text).map_err(anyhow::Error::msg)
    }

    #[test]
    fn full_backup_round_trips_objects_and_source_metadata() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let database = dir.path().join("registry.redb");
        let backup = dir.path().join("registry.backup");
        let restored = dir.path().join("restored.redb");
        let store = Store::open(&database)?;
        let baseline = object("aut-num: AS64496\nsource: TEST\n")?;
        let added = object("route: 192.0.2.0/24\norigin: AS64496\nsource: TEST\n")?;
        store.replace_source("TEST", [Ok(baseline)])?;
        store.apply_nrtm3(
            "TEST",
            (10, 10),
            Some(9),
            [Ok(Operation {
                kind: Kind::Add,
                serial: 10,
                object: added.clone(),
            })],
        )?;

        let state = fixture_state("SIGNED", "c86f0f62-447e-4bdf-8cb6-18f861bc3532", 1);
        let signed = object("aut-num: AS64497\nsource: SIGNED\n")?;
        store.apply_nrtm4(
            "SIGNED",
            None,
            &state,
            true,
            [Ok(Record::Snapshot(signed.clone()))],
        )?;

        let passwords = [String::from("fixture password")];
        let hash = bcrypt::hash(&passwords[0], 4)?;
        let maintainer = object(&format!(
            "mntner: AUTH-MNT\nauth: BCRYPT-PW {hash}\nmnt-by: AUTH-MNT\nsource: AUTH\n"
        ))?;
        let person = object(
            "person: Example Operator\naddress: Example Office\nphone: +1 555 0100\ne-mail: operator@example.net\nnic-hdl: EO1-AUTH\nmnt-by: AUTH-MNT\nsource: AUTH\n",
        )?;
        store.replace_source("AUTH", [Ok(maintainer)])?;
        store.apply_authoritative("AUTH", vec![Change::Upsert(person.clone())], &passwords)?;
        store.apply_authoritative("AUTH", vec![Change::Delete(person.clone())], &passwords)?;

        assert!(create(&database, &backup, 8 * 1024 * 1024).is_err());
        assert!(!backup.exists());
        drop(store);

        let length = create(&database, &backup, 8 * 1024 * 1024)?;
        assert_eq!(
            fs::metadata(&backup)?.len(),
            HEADER_BYTES + length + CHECKSUM_BYTES
        );
        assert_eq!(restore(&backup, &restored, 8 * 1024 * 1024)?, length);
        let store = Store::open_existing(&restored)?;
        assert_eq!(store.verify()?, 4);
        assert_eq!(
            store.first("route", &added.key, &[String::from("TEST")])?,
            Some(added.clone())
        );
        assert_eq!(store.source_serial("TEST")?, Some(10));
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, Some((10, 10)));
        let journal = store.nrtm3_journal("TEST", 10, None, 1, 4096)?.unwrap();
        assert_eq!(journal.1[0].object, added);
        assert_eq!(store.nrtm4_state("SIGNED")?, Some(state));
        assert_eq!(
            store.first("aut-num", &signed.key, &[String::from("SIGNED")])?,
            Some(signed)
        );
        assert_eq!(store.source_history("AUTH", 2)?[0].action, "delete");
        let error = store
            .apply_authoritative("AUTH", vec![Change::Upsert(person)], &passwords)
            .unwrap_err();
        assert!(error.to_string().contains("name is protected"));
        Ok(())
    }

    #[test]
    fn backup_rejects_corruption_foreign_databases_and_existing_targets() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let database = dir.path().join("registry.redb");
        let archive = dir.path().join("registry.backup");
        let damaged = dir.path().join("damaged.backup");
        let restored = dir.path().join("restored.redb");
        let existing = dir.path().join("existing");
        let foreign = dir.path().join("foreign.redb");
        let empty = dir.path().join("empty.redb");
        drop(Store::open(&database)?);
        create(&database, &archive, 8 * 1024 * 1024)?;
        fs::write(&existing, b"keep me")?;
        assert!(create(&database, &existing, 8 * 1024 * 1024).is_err());
        assert!(restore(&archive, &existing, 8 * 1024 * 1024).is_err());
        assert_eq!(fs::read(&existing)?, b"keep me");

        let valid = fs::read(&archive)?;
        let mut bad_checksum = valid.clone();
        bad_checksum[HEADER_BYTES as usize] ^= 1;
        let mut wrong_version = valid.clone();
        wrong_version[MAGIC.len()] ^= 1;
        let mut trailing = valid.clone();
        trailing.push(0);
        for (bytes, message) in [
            (bad_checksum.as_slice(), "checksum mismatch"),
            (&valid[..valid.len() - 1], "length does not match"),
            (trailing.as_slice(), "length does not match"),
            (
                wrong_version.as_slice(),
                "unsupported registry backup version",
            ),
        ] {
            fs::write(&damaged, bytes)?;
            assert!(
                restore(&damaged, &restored, 8 * 1024 * 1024)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
            assert!(!restored.exists());
        }

        drop(redb::Database::create(&foreign)?);
        assert!(create(&foreign, &restored, 8 * 1024 * 1024).is_err());
        assert!(!restored.exists());
        let payload = fs::read(&foreign)?;
        let mut foreign_archive = Vec::new();
        write_header(&mut foreign_archive, payload.len() as u64)?;
        foreign_archive.extend_from_slice(&payload);
        foreign_archive.extend_from_slice(&Sha256::digest(&payload));
        fs::write(&damaged, foreign_archive)?;
        assert!(restore(&damaged, &restored, 8 * 1024 * 1024).is_err());
        assert!(!restored.exists());

        fs::write(&empty, [])?;
        assert!(create(&empty, &restored, 8 * 1024 * 1024).is_err());
        assert_eq!(fs::metadata(&empty)?.len(), 0);
        assert!(!restored.exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_stage_excludes_other_read_handles_until_published() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let output = dir.path().join("backup");
        let mut stage = stage_for(&output)?;
        stage.write_all(b"private")?;
        assert!(File::open(stage.path()).is_err());
        publish(stage, &output)?;
        assert_eq!(fs::read(&output)?, b"private");
        Ok(())
    }
}
