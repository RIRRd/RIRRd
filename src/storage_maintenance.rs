//! Offline accounting and bounded, lossless repacking into a separate file.
use super::*;
use sha2::{Digest, Sha256};

pub(super) fn check_cancelled(cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<()> {
    ensure!(
        !cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)),
        "database repack cancelled"
    );
    Ok(())
}

const BATCH_BYTES: usize = 1024 * 1024;
const BATCH_ROWS: usize = 4096;

#[derive(Debug, serde::Serialize)]
pub struct RepackReport {
    objects: u64,
    tables: usize,
    logical_sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(path: &Path) -> Result<Store> {
        let store = Store::open(path)?;
        store.replace_source(
            "TEST",
            [Ok(
                crate::rpsl::parse("aut-num: AS64496\nsource: TEST\n").unwrap()
            )],
        )?;
        Ok(store)
    }

    #[test]
    fn repack_uses_one_snapshot_during_concurrent_source_replacement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = Store::open(&dir.path().join("source.redb"))?;
        let replace = |generation| -> Result<()> {
            source.replace_source(
                "TEST",
                (0..300).map(|index| {
                    crate::rpsl::parse(&format!(
                        "aut-num: AS{}\nas-name: GENERATION{generation}\nsource: TEST\n",
                        64496 + index
                    ))
                    .map_err(anyhow::Error::msg)
                }),
            )?;
            Ok(())
        };
        replace(0)?;
        let mut valid_snapshots = vec![logical_hash(&source.db.begin_read()?)?];
        let path = dir.path().join("packed.redb");
        std::thread::scope(|scope| -> Result<()> {
            let packed = scope.spawn(|| source.repack(&path, 1024 * 1024));
            for generation in 1..=10 {
                replace(generation)?;
                valid_snapshots.push(logical_hash(&source.db.begin_read()?)?);
            }
            packed.join().unwrap()?;
            Ok(())
        })?;
        let packed = Store::open_existing(&path)?;
        assert_eq!(packed.verify()?, 300);
        assert!(valid_snapshots.contains(&logical_hash(&packed.db.begin_read()?)?));
        assert_eq!(source.verify()?, 300);
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let cancelled = dir.path().join("cancelled.redb");
        assert!(
            source
                .repack_with_cancel(&cancelled, 1024 * 1024, Some(&cancel))
                .is_err()
        );
        assert!(!cancelled.exists());
        assert!(source.verify_with_cancel(Some(&cancel)).is_err());
        Ok(())
    }

    #[test]
    fn accounting_and_repack_keep_original_rows_and_refuse_overwrite() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = seeded(&dir.path().join("source.redb"))?;
        let usage = source.storage_usage()?;
        assert!(usage.stored_bytes > 0);
        assert_eq!(
            usage
                .tables
                .iter()
                .find(|row| row.name == OBJECTS.name())
                .unwrap()
                .entries,
            1
        );
        let expected = logical_hash(&source.db.begin_read()?)?;
        let path = dir.path().join("packed.redb");
        let result = source.repack(&path, 1024 * 1024)?;
        assert_eq!(result.objects, 1);
        assert_eq!(result.logical_sha256, expected.1);
        let packed = Store::open_existing(&path)?;
        assert_eq!(logical_hash(&packed.db.begin_read()?)?, expected);
        assert_eq!(logical_hash(&source.db.begin_read()?)?, expected);
        assert!(source.repack(&path, 1024 * 1024).is_err());
        assert_eq!(packed.verify()?, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn repack_rejects_unknown_tables_savepoints_and_corrupt_indexes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = seeded(&dir.path().join("source.redb"))?;
        let path = dir.path().join("packed.redb");
        let foreign: TableDefinition<&str, u64> = TableDefinition::new("future_extension");
        let write = source.db.begin_write()?;
        write.open_table(foreign)?.insert("preserve", 42)?;
        write.commit()?;
        assert!(
            source
                .repack(&path, 1024 * 1024)
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
        assert!(!path.exists());
        let write = source.db.begin_write()?;
        write.delete_table(foreign)?;
        write.commit()?;
        let write = source.db.begin_write()?;
        let savepoint = write.persistent_savepoint()?;
        write.commit()?;
        assert!(
            source
                .repack(&path, 1024 * 1024)
                .unwrap_err()
                .to_string()
                .contains("savepoints")
        );
        assert!(!path.exists());
        let write = source.db.begin_write()?;
        write.delete_persistent_savepoint(savepoint)?;
        write
            .open_multimap_table(EXACT)?
            .remove_all(("TEST", "AS64496"))?;
        write.commit()?;
        assert!(source.repack(&path, 1024 * 1024).is_err());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
        Ok(())
    }
    #[test]
    fn repack_preserves_large_rows_and_multiple_classes_for_one_key() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = Store::open(&dir.path().join("source.redb"))?;
        let policy = format!(
            "filter-set: FLTR-BIG\nfilter: {}\nsource: TEST\n",
            "AS64496 OR ".repeat(100_000) + "AS64497"
        );
        let texts = [
            "person: Example Person\nnic-hdl: SAME-TEST\nsource: TEST\n",
            "role: Example Role\nnic-hdl: SAME-TEST\nsource: TEST\n",
            &policy,
        ];
        source.replace_source(
            "TEST",
            texts
                .into_iter()
                .map(|text| crate::rpsl::parse_snapshot(text).map_err(anyhow::Error::msg)),
        )?;
        let path = dir.path().join("packed.redb");
        source.repack(&path, 1024 * 1024)?;
        let packed = Store::open_existing(&path)?;
        let sources = [String::from("TEST")];
        assert_eq!(packed.exact("SAME-TEST", &sources, 3)?.len(), 2);
        assert_eq!(
            packed
                .first("filter-set", "FLTR-BIG", &sources)?
                .unwrap()
                .text,
            policy
        );
        assert_eq!(packed.verify()?, 3);
        Ok(())
    }
}

// Give supported B-tree inserts a dispersed order within a bounded batch.
// Ascending inserts otherwise repeatedly split the growing edge into half-full
// leaves. This is solely a packing order, never a security/randomness primitive.
fn packing_order(rows: &mut [(Vec<u8>, Vec<u8>)]) {
    let mut state = 0x5249_5252_4450_4143_u64;
    for index in (1..rows.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        rows.swap(index, (state % (index as u64 + 1)) as usize);
    }
}

fn hash_bytes(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn flush<K: redb::Key + 'static, V: redb::Value + 'static>(
    rows: &mut Vec<(Vec<u8>, Vec<u8>)>,
    table: &mut Table<'_, K, V>,
) -> Result<()> {
    packing_order(rows);
    for (key, value) in rows.drain(..) {
        table.insert(K::from_bytes(&key), V::from_bytes(&value))?;
    }
    Ok(())
}

fn copy_table<K: redb::Key + 'static, V: redb::Value + 'static>(
    source: &ReadTransaction,
    destination: &WriteTransaction,
    definition: TableDefinition<'_, K, V>,
    hash: &mut Sha256,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<usize> {
    let original = match source.open_table(definition) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    eprintln!("Repacking table {}", definition.name());
    hash.update([1]);
    hash_bytes(hash, definition.name().as_bytes());
    hash.update(original.len()?.to_le_bytes());
    let mut table = destination.open_table(definition)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for entry in original.iter()? {
        check_cancelled(cancel)?;
        let (key, value) = entry?;
        let key = K::as_bytes(&key.value()).as_ref().to_vec();
        let value = V::as_bytes(&value.value()).as_ref().to_vec();
        hash_bytes(hash, &key);
        hash_bytes(hash, &value);
        bytes += key.len() + value.len();
        rows.push((key, value));
        if bytes >= BATCH_BYTES || rows.len() >= BATCH_ROWS {
            flush(&mut rows, &mut table)?;
            bytes = 0;
        }
    }
    flush(&mut rows, &mut table)?;
    Ok(1)
}

fn copy_multimap<K: redb::Key + 'static, V: redb::Key + 'static>(
    source: &ReadTransaction,
    destination: &WriteTransaction,
    definition: MultimapTableDefinition<'_, K, V>,
    hash: &mut Sha256,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<usize> {
    let original = source.open_multimap_table(definition)?;
    eprintln!("Repacking multimap {}", definition.name());
    hash.update([2]);
    hash_bytes(hash, definition.name().as_bytes());
    hash.update(original.len()?.to_le_bytes());
    let mut table = destination.open_multimap_table(definition)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    // Bound batches even inside a single high-cardinality multimap group.
    for entry in original.iter()? {
        check_cancelled(cancel)?;
        let (key, values) = entry?;
        for value in values {
            check_cancelled(cancel)?;
            let value = value?;
            let key = K::as_bytes(&key.value()).as_ref().to_vec();
            let value = V::as_bytes(&value.value()).as_ref().to_vec();
            hash_bytes(hash, &key);
            hash_bytes(hash, &value);
            bytes += key.len() + value.len();
            rows.push((key, value));
            if bytes >= BATCH_BYTES || rows.len() >= BATCH_ROWS {
                flush_multimap(&mut rows, &mut table)?;
                bytes = 0;
            }
        }
    }
    flush_multimap(&mut rows, &mut table)?;
    Ok(1)
}

fn flush_multimap<K: redb::Key + 'static, V: redb::Key + 'static>(
    rows: &mut Vec<(Vec<u8>, Vec<u8>)>,
    table: &mut MultimapTable<'_, K, V>,
) -> Result<()> {
    packing_order(rows);
    for (key, value) in rows.drain(..) {
        table.insert(K::from_bytes(&key), V::from_bytes(&value))?;
    }
    Ok(())
}

fn hash_table<K: redb::Key + 'static, V: redb::Value + 'static>(
    read: &ReadTransaction,
    definition: TableDefinition<'_, K, V>,
    hash: &mut Sha256,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<usize> {
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    hash.update([1]);
    hash_bytes(hash, definition.name().as_bytes());
    hash.update(table.len()?.to_le_bytes());
    for entry in table.iter()? {
        check_cancelled(cancel)?;
        let (key, value) = entry?;
        hash_bytes(hash, K::as_bytes(&key.value()).as_ref());
        hash_bytes(hash, V::as_bytes(&value.value()).as_ref());
    }
    Ok(1)
}

fn hash_multimap<K: redb::Key + 'static, V: redb::Key + 'static>(
    read: &ReadTransaction,
    definition: MultimapTableDefinition<'_, K, V>,
    hash: &mut Sha256,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<usize> {
    let table = read.open_multimap_table(definition)?;
    hash.update([2]);
    hash_bytes(hash, definition.name().as_bytes());
    hash.update(table.len()?.to_le_bytes());
    for entry in table.iter()? {
        check_cancelled(cancel)?;
        let (key, values) = entry?;
        for value in values {
            check_cancelled(cancel)?;
            hash_bytes(hash, K::as_bytes(&key.value()).as_ref());
            hash_bytes(hash, V::as_bytes(&value?.value()).as_ref());
        }
    }
    Ok(1)
}

#[cfg(test)]
fn logical_hash(read: &ReadTransaction) -> Result<(usize, String)> {
    logical_hash_with_cancel(read, None)
}

fn logical_hash_with_cancel(
    read: &ReadTransaction,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(usize, String)> {
    // Hash canonical, length-delimited encodings, table kind/name and row count.
    let mut hash = Sha256::new();
    hash.update(b"RIRRd logical database v1");
    let mut tables = 0;
    macro_rules! table { ($($definition:expr),*) => {
        $(tables += hash_table(read, $definition, &mut hash, cancel)?;)*
    }; }
    table!(
        OBJECTS,
        PREFIXES,
        INETNUM_COVER,
        AS_BLOCK_COVER,
        INVERSE,
        CLASS_COUNTS,
        SERIALS,
        MIRROR_FILTERS,
        AUTH_NRTM3_SERIALS,
        NRTM4_STATE,
        JOURNAL,
        AUTH_AUDIT,
        PROTECTED_NAMES
    );
    tables += hash_multimap(read, EXACT, &mut hash, cancel)?;
    tables += hash_multimap(read, ORIGINS, &mut hash, cancel)?;
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((tables, digest))
}

#[derive(serde::Serialize)]
pub struct TableUsage {
    name: String,
    kind: &'static str,
    entries: u64,
    stored_bytes: u64,
    metadata_bytes: u64,
    fragmented_bytes: u64,
    leaf_pages: u64,
    branch_pages: u64,
}

#[derive(serde::Serialize)]
pub struct StorageUsage {
    page_size: usize,
    allocated_pages: u64,
    stored_bytes: u64,
    metadata_bytes: u64,
    fragmented_bytes: u64,
    tables: Vec<TableUsage>,
}

fn table_usage(
    name: String,
    kind: &'static str,
    entries: u64,
    stats: redb::TableStats,
) -> TableUsage {
    TableUsage {
        name,
        kind,
        entries,
        stored_bytes: stats.stored_bytes(),
        metadata_bytes: stats.metadata_bytes(),
        fragmented_bytes: stats.fragmented_bytes(),
        leaf_pages: stats.leaf_pages(),
        branch_pages: stats.branch_pages(),
    }
}

impl Store {
    pub fn repack(&self, output: &Path, cache_bytes: usize) -> Result<RepackReport> {
        self.repack_with_cancel(output, cache_bytes, None)
    }

    pub fn repack_with_cancel(
        &self,
        output: &Path,
        cache_bytes: usize,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<RepackReport> {
        check_cancelled(cancel)?;
        crate::backup::require_absent(output)?;
        let read = self.db.begin_read()?;
        let normal = [
            OBJECTS.name(),
            PREFIXES.name(),
            INETNUM_COVER.name(),
            AS_BLOCK_COVER.name(),
            INVERSE.name(),
            CLASS_COUNTS.name(),
            SERIALS.name(),
            MIRROR_FILTERS.name(),
            AUTH_NRTM3_SERIALS.name(),
            NRTM4_STATE.name(),
            JOURNAL.name(),
            AUTH_AUDIT.name(),
            PROTECTED_NAMES.name(),
        ];
        ensure!(
            read.list_tables()?
                .all(|table| normal.contains(&table.name())),
            "repack refuses unknown application tables"
        );
        ensure!(
            read.list_multimap_tables()?
                .all(|table| [EXACT.name(), ORIGINS.name()].contains(&table.name())),
            "repack refuses unknown multimap tables"
        );
        let check = self.db.begin_write()?;
        ensure!(
            check.list_persistent_savepoints()?.next().is_none(),
            "repack refuses persistent savepoints"
        );
        check.abort()?;
        let objects = read.open_table(OBJECTS)?.len()?;
        // Hash the stable source snapshot in canonical order while copying it,
        // before dispersing each batch. The independently read final artifact
        // must match this digest and pass the complete semantic verifier.
        let mut hash = Sha256::new();
        hash.update(b"RIRRd logical database v1");
        let mut tables = 0;
        let expected;
        let stage = crate::backup::stage_for(output)?;
        {
            let db = Database::builder()
                .set_cache_size(cache_bytes)
                .create_file(stage.as_file().try_clone()?)?;
            let write = db.begin_write()?;
            macro_rules! copy { ($($definition:expr),*) => { $(tables += copy_table(&read, &write, $definition, &mut hash, cancel)?;)* }; }
            copy!(
                OBJECTS,
                PREFIXES,
                INETNUM_COVER,
                AS_BLOCK_COVER,
                INVERSE,
                CLASS_COUNTS,
                SERIALS,
                MIRROR_FILTERS,
                AUTH_NRTM3_SERIALS,
                NRTM4_STATE,
                JOURNAL,
                AUTH_AUDIT,
                PROTECTED_NAMES
            );
            tables += copy_multimap(&read, &write, EXACT, &mut hash, cancel)?;
            tables += copy_multimap(&read, &write, ORIGINS, &mut hash, cancel)?;
            ensure!(
                tables == read.list_tables()?.count() + read.list_multimap_tables()?.count(),
                "repack cannot account for every source table"
            );
            expected = (
                tables,
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            );
            write.commit()?;
            let mut db = db;
            eprintln!("Compacting repacked database");
            db.compact()?;
        }
        // Reopen the closed database through the private staging handle. Windows
        // deliberately denies a second path-based read handle before publication.
        {
            eprintln!("Verifying closed and reopened database");
            let rebuilt = crate::backup::lock_existing_file(stage.as_file(), cache_bytes)?;
            ensure!(
                rebuilt.verify_with_cancel(cancel)? == objects,
                "repacked database failed reopen verification"
            );
            eprintln!("Comparing final logical database hash");
            ensure!(
                logical_hash_with_cancel(&rebuilt.db.begin_read()?, cancel)? == expected,
                "repacked database changed after reopen"
            );
        }
        check_cancelled(cancel)?;
        crate::backup::publish(stage, output)?;
        Ok(RepackReport {
            objects,
            tables: expected.0,
            logical_sha256: expected.1,
        })
    }

    pub fn storage_usage(&self) -> Result<StorageUsage> {
        let read = self.read_transaction()?;
        let mut tables = Vec::new();
        for handle in read.list_tables()? {
            let name = handle.name().to_owned();
            let table = read.open_untyped_table(handle)?;
            tables.push(table_usage(name, "table", table.len()?, table.stats()?));
        }
        for handle in read.list_multimap_tables()? {
            let name = handle.name().to_owned();
            let table = read.open_untyped_multimap_table(handle)?;
            tables.push(table_usage(name, "multimap", table.len()?, table.stats()?));
        }
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        let write = self.db.begin_write()?;
        let stats = write.stats()?;
        write.abort()?;
        Ok(StorageUsage {
            page_size: stats.page_size(),
            allocated_pages: stats.allocated_pages(),
            stored_bytes: stats.stored_bytes(),
            metadata_bytes: stats.metadata_bytes(),
            fragmented_bytes: stats.fragmented_bytes(),
            tables,
        })
    }
}
