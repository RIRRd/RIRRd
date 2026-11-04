//! Offline accounting and bounded, lossless repacking into a separate file.
use super::*;
use sha2::{Digest, Sha256};

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
) -> Result<()> {
    let original = match source.open_table(definition) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut table = destination.open_table(definition)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for entry in original.iter()? {
        let (key, value) = entry?;
        let key = K::as_bytes(&key.value()).as_ref().to_vec();
        let value = V::as_bytes(&value.value()).as_ref().to_vec();
        bytes += key.len() + value.len();
        rows.push((key, value));
        if bytes >= BATCH_BYTES || rows.len() >= BATCH_ROWS {
            flush(&mut rows, &mut table)?;
            bytes = 0;
        }
    }
    flush(&mut rows, &mut table)?;
    Ok(())
}

fn copy_multimap<K: redb::Key + 'static, V: redb::Key + 'static>(
    source: &ReadTransaction,
    destination: &WriteTransaction,
    definition: MultimapTableDefinition<'_, K, V>,
) -> Result<()> {
    let original = source.open_multimap_table(definition)?;
    let mut table = destination.open_multimap_table(definition)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    // Bound batches even inside a single high-cardinality multimap group.
    for entry in original.iter()? {
        let (key, values) = entry?;
        for value in values {
            let value = value?;
            let key = K::as_bytes(&key.value()).as_ref().to_vec();
            let value = V::as_bytes(&value.value()).as_ref().to_vec();
            bytes += key.len() + value.len();
            rows.push((key, value));
            if bytes >= BATCH_BYTES || rows.len() >= BATCH_ROWS {
                flush_multimap(&mut rows, &mut table)?;
                bytes = 0;
            }
        }
    }
    flush_multimap(&mut rows, &mut table)?;
    Ok(())
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
) -> Result<usize> {
    let table = read.open_multimap_table(definition)?;
    hash.update([2]);
    hash_bytes(hash, definition.name().as_bytes());
    hash.update(table.len()?.to_le_bytes());
    for entry in table.iter()? {
        let (key, values) = entry?;
        for value in values {
            hash_bytes(hash, K::as_bytes(&key.value()).as_ref());
            hash_bytes(hash, V::as_bytes(&value?.value()).as_ref());
        }
    }
    Ok(1)
}

fn logical_hash(read: &ReadTransaction) -> Result<(usize, String)> {
    // Hash canonical, length-delimited encodings, table kind/name and row count.
    let mut hash = Sha256::new();
    hash.update(b"RIRRd logical database v1");
    let mut tables = 0;
    macro_rules! table { ($($definition:expr),*) => {
        $(tables += hash_table(read, $definition, &mut hash)?;)*
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
    tables += hash_multimap(read, EXACT, &mut hash)?;
    tables += hash_multimap(read, ORIGINS, &mut hash)?;
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
        let objects = self.verify().context("verifying registry before repack")?;
        let expected = logical_hash(&read)?;
        ensure!(
            expected.0 == read.list_tables()?.count() + read.list_multimap_tables()?.count(),
            "repack cannot account for every source table"
        );
        let stage = crate::backup::stage_for(output)?;
        {
            let db = Database::builder()
                .set_cache_size(cache_bytes)
                .create_file(stage.as_file().try_clone()?)?;
            let write = db.begin_write()?;
            macro_rules! copy { ($($definition:expr),*) => { $(copy_table(&read, &write, $definition)?;)* }; }
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
            copy_multimap(&read, &write, EXACT)?;
            copy_multimap(&read, &write, ORIGINS)?;
            write.commit()?;
            let mut rebuilt = Store::from_database(db);
            ensure!(rebuilt.verify()? == objects, "repack changed object count");
            ensure!(
                logical_hash(&rebuilt.db.begin_read()?)? == expected,
                "repack changed logical database contents"
            );
            rebuilt.db.compact()?;
        }
        // Reopen the closed database through the private staging handle. Windows
        // deliberately denies a second path-based read handle before publication.
        {
            let rebuilt = crate::backup::lock_existing_file(stage.as_file(), cache_bytes)?;
            ensure!(
                rebuilt.verify()? == objects,
                "repacked database failed reopen verification"
            );
            ensure!(
                logical_hash(&rebuilt.db.begin_read()?)? == expected,
                "repacked database changed after reopen"
            );
        }
        crate::backup::publish(stage, output)?;
        Ok(RepackReport {
            objects,
            tables: expected.0,
            logical_sha256: expected.1,
        })
    }

    pub fn storage_usage(&self) -> Result<StorageUsage> {
        let read = self.db.begin_read()?;
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
