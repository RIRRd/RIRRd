//! Durable, indexed storage for locally imported source snapshots.

use std::{collections::HashSet, io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use redb::{
    Database, Durability, MultimapTableDefinition, MultimapTableHandle, ReadOnlyTable,
    ReadableMultimapTable, ReadableTable, ReadableTableMetadata, TableDefinition,
};

use crate::rpsl::Object;

type Identity = (&'static str, &'static str, &'static str);
type ExactKey = (&'static str, &'static str);

// Identity is (source, class, key). The exact index maps (source, key) to classes.
const OBJECTS: TableDefinition<Identity, &str> = TableDefinition::new("objects_v1");
const EXACT: MultimapTableDefinition<ExactKey, &str> = MultimapTableDefinition::new("exact_v1");
// The origin index maps (source, class, origin) to distinct canonical prefixes.
const ORIGINS: MultimapTableDefinition<Identity, &str> = MultimapTableDefinition::new("origins_v1");

pub struct Store {
    db: Database,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).with_context(|| format!("open {}", path.display()))?;
        let mut write = db.begin_write()?;
        write.set_durability(Durability::Immediate);
        {
            let records = write.open_table(OBJECTS)?;
            write.open_multimap_table(EXACT)?;
            let needs_backfill = !write
                .list_multimap_tables()?
                .any(|table| table.name() == ORIGINS.name());
            let mut origins = write.open_multimap_table(ORIGINS)?;
            if needs_backfill {
                for entry in records.iter()? {
                    let (identity, _) = entry?;
                    let (source, class, key) = identity.value();
                    if let Some((prefix, origin)) = route_index_parts(class, key)? {
                        origins.insert((source, class, origin), prefix)?;
                    }
                }
            }
        }
        write.commit()?;
        Ok(Self { db })
    }

    /// Open without creating the file or initializing its application tables.
    pub fn open_existing(path: &Path) -> Result<Self> {
        let db = Database::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(Self { db })
    }

    /// Publish a source's replacement snapshot and its index in one durable commit.
    /// Readers continue to see the previous committed state while records are processed.
    /// An empty iterator clears the named source.
    pub fn replace_source(
        &self,
        source: &str,
        objects: impl IntoIterator<Item = Result<Object>>,
    ) -> Result<u64> {
        let source = source.to_ascii_uppercase();
        ensure!(
            !source.is_empty() && !source.contains('\0'),
            "invalid source name"
        );
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let mut count = 0;
        {
            let mut records = write.open_table(OBJECTS)?;
            let mut exact = write.open_multimap_table(EXACT)?;
            let mut origins = write.open_multimap_table(ORIGINS)?;

            // Appending the smallest character gives the exclusive end of this source's range.
            let next_source = format!("{source}\0");
            let old_records = records.extract_from_if(
                (source.as_str(), "", "")..(next_source.as_str(), "", ""),
                |_, _| true,
            )?;
            for entry in old_records {
                let (identity, _) = entry?;
                let (source, class, key) = identity.value();
                ensure!(
                    exact.remove((source, key), class)?,
                    "object is missing from the exact lookup index"
                );
                if let Some((prefix, origin)) = route_index_parts(class, key)? {
                    ensure!(
                        origins.remove((source, class, origin), prefix)?,
                        "object is missing from the route origin index"
                    );
                }
            }

            for object in objects {
                let Object {
                    source: object_source,
                    class,
                    key,
                    text,
                } = object?;
                ensure!(
                    object_source == source,
                    "object source does not match {source}"
                );
                ensure!(
                    !class.is_empty() && !key.is_empty(),
                    "empty object identity"
                );
                let identity = (source.as_str(), class.as_str(), key.as_str());
                ensure!(
                    records.insert(identity, text.as_str())?.is_none(),
                    "duplicate {class} object {key} in {source}"
                );
                ensure!(
                    !exact.insert((source.as_str(), key.as_str()), class.as_str())?,
                    "unexpected duplicate entry in the exact lookup index"
                );
                if let Some((prefix, origin)) = route_index_parts(&class, &key)? {
                    ensure!(
                        !origins.insert((source.as_str(), class.as_str(), origin), prefix)?,
                        "unexpected duplicate entry in the route origin index"
                    );
                }
                count += 1;
            }
        }
        write.commit()?;
        Ok(count)
    }

    /// Find up to `limit` objects by their key in the given source priority order.
    pub fn exact(&self, key: &str, sources: &[String], limit: usize) -> Result<Vec<Object>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        let key = key.to_uppercase();
        let read = self.db.begin_read()?;
        let exact = read.open_multimap_table(EXACT)?;
        let records = read.open_table(OBJECTS)?;
        let mut objects = Vec::new();
        for source in sources {
            for entry in exact
                .get((source.as_str(), key.as_str()))?
                .take(limit - objects.len())
            {
                let entry = entry?;
                let object = read_object(&records, source, entry.value(), &key)?
                    .context("exact lookup index points to a missing object")?;
                objects.push(object);
            }
            if objects.len() == limit {
                break;
            }
        }
        Ok(objects)
    }

    /// Find the first object of the specified class and canonical key in source order.
    pub fn first(&self, class: &str, key: &str, sources: &[String]) -> Result<Option<Object>> {
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        for source in sources {
            if let Some(object) = read_object(&records, source, class, key)? {
                return Ok(Some(object));
            }
        }
        Ok(None)
    }

    /// Find routes at one canonical prefix in source priority order.
    /// IPv6 prefixes use uppercase hexadecimal, as in stored route identities.
    pub fn routes_at_prefix(
        &self,
        class: &str,
        prefix: &str,
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        let start = format!("{prefix}AS");
        let end = format!("{prefix}AT");
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let mut routes = Vec::new();
        for source in sources {
            let lower = (source.as_str(), class, start.as_str());
            let upper = (source.as_str(), class, end.as_str());
            for entry in records.range(lower..upper)?.take(limit - routes.len()) {
                let (identity, text) = entry?;
                let (source, class, key) = identity.value();
                routes.push(Object {
                    source: source.to_owned(),
                    class: class.to_owned(),
                    key: key.to_owned(),
                    text: text.value().to_owned(),
                });
            }
            if routes.len() == limit {
                break;
            }
        }
        Ok(routes)
    }

    /// Find distinct prefixes in source priority order, sorted within each source.
    pub fn origin_prefixes(
        &self,
        class: &str,
        origin: &str,
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<String>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        let read = self.db.begin_read()?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let mut seen = HashSet::new();
        let mut prefixes = Vec::new();
        for source in sources {
            // Each source has unique prefixes, so at most the current number of
            // results can repeat. Inspecting `limit` entries suffices to fill the cap.
            for prefix in origins.get((source.as_str(), class, origin))?.take(limit) {
                let prefix = prefix?;
                if seen.insert(prefix.value().to_owned()) {
                    prefixes.push(prefix.value().to_ascii_lowercase());
                    if prefixes.len() == limit {
                        return Ok(prefixes);
                    }
                }
            }
        }
        Ok(prefixes)
    }

    /// Write raw RPSL from one committed source snapshot, including any sensitive attributes.
    pub fn export_source(&self, source: &str, mut output: impl Write) -> Result<u64> {
        let source = source.to_ascii_uppercase();
        let next_source = format!("{source}\0");
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let mut count = 0;
        for entry in records.range((source.as_str(), "", "")..(next_source.as_str(), "", ""))? {
            let (_, text) = entry?;
            let text = text.value();
            output.write_all(text.as_bytes())?;
            if !text.ends_with('\n') {
                output.write_all(b"\n")?;
            }
            output.write_all(b"\n")?;
            count += 1;
        }
        ensure!(count > 0, "source {source} has no objects to export");
        Ok(count)
    }

    /// Check that the exact and route origin indexes cover the stored identities.
    pub fn verify(&self) -> Result<u64> {
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let exact = read.open_multimap_table(EXACT)?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let mut indexed = 0;
        let mut routes = 0;
        for entry in exact.iter()? {
            let (identity, classes) = entry?;
            let (source, key) = identity.value();
            for class in classes {
                let class = class?;
                ensure!(
                    records.get((source, class.value(), key))?.is_some(),
                    "exact lookup index points to a missing object"
                );
                indexed += 1;
                if route_index_parts(class.value(), key)?.is_some() {
                    routes += 1;
                }
            }
        }
        // Each index entry identifies a distinct object, so matching counts prove coverage.
        ensure!(
            indexed == records.len()?,
            "some objects are missing from the exact lookup index"
        );
        let mut indexed_routes = 0;
        for entry in origins.iter()? {
            let (identity, prefixes) = entry?;
            let (source, class, origin) = identity.value();
            for prefix in prefixes {
                let prefix = prefix?;
                let key = format!("{}{origin}", prefix.value());
                ensure!(
                    route_index_parts(class, &key)? == Some((prefix.value(), origin))
                        && records.get((source, class, key.as_str()))?.is_some(),
                    "route origin index points to a missing route"
                );
                indexed_routes += 1;
            }
        }
        ensure!(
            indexed_routes == routes,
            "some routes are missing from the route origin index"
        );
        Ok(indexed)
    }
}

fn route_index_parts<'a>(class: &str, key: &'a str) -> Result<Option<(&'a str, &'a str)>> {
    if !matches!(class, "route" | "route6") {
        return Ok(None);
    }
    let (prefix, asn) = key.rsplit_once("AS").context("invalid stored route key")?;
    ensure!(
        !prefix.is_empty() && !asn.is_empty() && asn.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid stored route key"
    );
    Ok(Some((prefix, &key[prefix.len()..])))
}

fn read_object(
    records: &ReadOnlyTable<Identity, &str>,
    source: &str,
    class: &str,
    key: &str,
) -> Result<Option<Object>> {
    Ok(records.get((source, class, key))?.map(|text| Object {
        source: source.to_owned(),
        class: class.to_owned(),
        key: key.to_owned(),
        text: text.value().to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::PathBuf,
        process::{Child, Command},
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    const CRASH_TEST_DATABASE: &str = "RUST_IRRD_CRASH_TEST_DATABASE";
    const CRASH_TEST_READY: &[u8] = b"replacement in progress\n";

    struct CrashChild(Child);

    impl Drop for CrashChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    fn object(source: &str, class: &str, key: &str) -> Object {
        Object {
            source: source.to_owned(),
            class: class.to_owned(),
            key: key.to_owned(),
            text: format!("{class}: {key}\nsource: {source}\n"),
        }
    }

    #[test]
    fn exact_preserves_class_and_source_identity_across_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("registry.redb");
        let sources = [String::from("TEST2"), String::from("TEST")];
        {
            let store = Store::open(&file)?;
            assert!(store.exact("AA1-TEST", &sources, 10)?.is_empty());
            assert_eq!(
                store.replace_source(
                    "test",
                    [
                        Ok(object("TEST", "person", "AA1-TEST")),
                        Ok(object("TEST", "role", "AA1-TEST")),
                    ],
                )?,
                2
            );
            store.replace_source("TEST2", [Ok(object("TEST2", "person", "AA1-TEST"))])?;
        }

        let store = Store::open(&file)?;
        let objects = store.exact("aa1-test", &sources, 10)?;
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[0].source, "TEST2");
        assert_eq!(objects[0].text, "person: AA1-TEST\nsource: TEST2\n");
        assert_eq!(
            (objects[1].source.as_str(), objects[1].class.as_str()),
            ("TEST", "person")
        );
        assert_eq!(
            (objects[2].source.as_str(), objects[2].class.as_str()),
            ("TEST", "role")
        );
        assert_eq!(store.exact("AA1-TEST", &sources, 1)?, &objects[..1]);
        assert_eq!(store.exact("AA1-TEST", &sources[1..], 10)?, &objects[1..]);
        assert!(store.exact("AA1-TEST", &sources, 0)?.is_empty());
        assert!(store.exact("AA1-TEST", &[], 10)?.is_empty());
        assert_eq!(
            store.first("person", "AA1-TEST", &sources)?.as_ref(),
            Some(&objects[0])
        );
        assert_eq!(
            store.first("person", "AA1-TEST", &sources[1..])?.as_ref(),
            Some(&objects[1])
        );
        assert_eq!(
            store.first("role", "AA1-TEST", &sources)?.as_ref(),
            Some(&objects[2])
        );
        assert!(store.first("role", "aa1-test", &sources)?.is_none());
        assert_eq!(store.verify()?, 3);
        Ok(())
    }

    #[test]
    fn exact_lookup_uses_unicode_uppercase() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let record = object("TEST", "domain", "CAFÉ.EXAMPLE");
        store.replace_source("TEST", [Ok(record.clone())])?;
        assert_eq!(
            store.exact("café.example", &[String::from("TEST")], 1)?,
            [record]
        );
        Ok(())
    }

    #[test]
    fn verifies_index_coverage_and_dangling_references() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        assert!(Store::open_existing(&path).is_err());
        assert!(!path.exists());
        drop(Database::create(&path)?);
        assert!(Store::open_existing(&path)?.verify().is_err());

        let store = Store::open(&path)?;
        store.replace_source("TEST", [Ok(object("TEST", "aut-num", "AS64496"))])?;

        let write = store.db.begin_write()?;
        assert!(
            write
                .open_multimap_table(EXACT)?
                .remove(("TEST", "AS64496"), "aut-num")?
        );
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("missing from")
        );

        let write = store.db.begin_write()?;
        write
            .open_multimap_table(EXACT)?
            .insert(("TEST", "AS64497"), "aut-num")?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("points to")
        );
        Ok(())
    }

    #[test]
    fn failed_replacement_rolls_back_data_and_index() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST")];
        store.replace_source("TEST", [Ok(object("TEST", "aut-num", "AS64496"))])?;
        assert!(
            store
                .replace_source(
                    "TEST",
                    [
                        Ok(object("TEST", "aut-num", "AS64497")),
                        Err(anyhow::anyhow!("snapshot stream failed")),
                    ],
                )
                .is_err()
        );
        assert!(
            store
                .replace_source(
                    "TEST",
                    [
                        Ok(object("TEST", "aut-num", "AS64497")),
                        Ok(object("TEST", "aut-num", "AS64497")),
                    ],
                )
                .is_err()
        );
        assert_eq!(store.exact("AS64496", &sources, 10)?.len(), 1);
        assert!(store.exact("AS64497", &sources, 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn export_selects_only_the_named_source_and_separates_raw_objects() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let mut first = object("TEST", "aut-num", "AS64496");
        first.text.pop();
        let second = object("TEST", "aut-num", "AS64497");
        let expected = format!("{}\n\n{}\n", first.text, second.text);
        store.replace_source("TEST", [Ok(second), Ok(first)])?;
        store.replace_source("TEST2", [Ok(object("TEST2", "aut-num", "AS64498"))])?;

        let mut output = Vec::new();
        assert_eq!(store.export_source("test", &mut output)?, 2);
        assert_eq!(output, expected.as_bytes());
        output.clear();
        assert!(store.export_source("MISSING", &mut output).is_err());
        assert!(output.is_empty());
        Ok(())
    }

    #[test]
    fn route_lookup_selects_the_exact_prefix_in_source_order() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        let first = object("TEST2", "route", "192.0.2.0/24AS64497");
        let second = object("TEST", "route", "192.0.2.0/24AS64496");
        let third = object("TEST", "route", "192.0.2.0/24AS64498");
        let ipv6 = object("TEST", "route6", "2001:DB8::/32AS64496");
        store.replace_source("TEST2", [Ok(first.clone())])?;
        store.replace_source(
            "TEST",
            [
                Ok(object("TEST", "route", "192.0.2.0/23AS64496")),
                Ok(object("TEST", "route", "192.0.2.0/25AS64496")),
                Ok(third.clone()),
                Ok(second.clone()),
                Ok(ipv6.clone()),
            ],
        )?;
        let expected = [first, second, third];
        assert_eq!(
            store.routes_at_prefix("route", "192.0.2.0/24", &sources, 8)?,
            expected
        );
        assert_eq!(
            store.routes_at_prefix("route", "192.0.2.0/24", &sources, 2)?,
            expected[..2]
        );
        assert_eq!(
            store.routes_at_prefix("route", "192.0.2.0/24", &sources[1..], 8)?,
            expected[1..]
        );
        assert_eq!(
            store.routes_at_prefix("route6", "2001:DB8::/32", &sources, 8)?,
            [ipv6]
        );
        assert!(
            store
                .routes_at_prefix("route", "192.0.2.0/24", &sources, 0)?
                .is_empty()
        );
        assert!(
            store
                .routes_at_prefix("route", "192.0.2.0/24", &[], 8)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn origin_lookup_is_bounded_deduplicated_and_transactional() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        store.replace_source(
            "TEST2",
            [
                Ok(object("TEST2", "route", "198.51.100.0/24AS64496")),
                Ok(object("TEST2", "route", "192.0.2.0/24AS64496")),
                Ok(object("TEST2", "route6", "2001:DB8::/32AS64496")),
                Ok(object("TEST2", "route", "203.0.113.0/24AS64497")),
            ],
        )?;
        store.replace_source(
            "TEST",
            [
                Ok(object("TEST", "route", "198.51.100.0/24AS64496")),
                Ok(object("TEST", "route", "192.0.2.0/24AS64496")),
                Ok(object("TEST", "route", "203.0.113.0/24AS64496")),
            ],
        )?;
        let expected = ["192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24"];
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 3)?,
            expected
        );
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 2)?,
            expected[..2]
        );
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources[..1], 3)?,
            expected[..2]
        );
        assert_eq!(
            store.origin_prefixes("route6", "AS64496", &sources, 3)?,
            ["2001:db8::/32"]
        );
        assert!(
            store
                .origin_prefixes("route", "AS64496", &sources, 0)?
                .is_empty()
        );

        assert!(
            store
                .replace_source(
                    "TEST",
                    [
                        Ok(object("TEST", "route", "10.0.0.0/8AS64496")),
                        Err(anyhow::anyhow!("input failed"))
                    ]
                )
                .is_err()
        );
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 3)?,
            expected
        );
        let replacement = std::iter::once_with(|| {
            assert_eq!(
                store
                    .origin_prefixes("route", "AS64496", &sources, 3)
                    .unwrap(),
                expected
            );
            Ok(object("TEST", "route", "10.0.0.0/8AS64496"))
        });
        store.replace_source("TEST", replacement)?;
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 3)?,
            ["192.0.2.0/24", "198.51.100.0/24", "10.0.0.0/8"]
        );
        assert_eq!(store.verify()?, 5);
        Ok(())
    }

    #[test]
    fn open_backfills_origin_index_and_verify_detects_missing_routes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let sources = [String::from("TEST")];
        let store = Store::open(&path)?;
        store.replace_source("TEST", [Ok(object("TEST", "route", "192.0.2.0/24AS64496"))])?;
        let write = store.db.begin_write()?;
        assert!(write.delete_multimap_table(ORIGINS)?);
        write.commit()?;
        drop(store);
        let store = Store::open(&path)?;
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 1)?,
            ["192.0.2.0/24"]
        );
        assert_eq!(store.verify()?, 1);

        let write = store.db.begin_write()?;
        assert!(
            write
                .open_multimap_table(ORIGINS)?
                .remove(("TEST", "route", "AS64496"), "192.0.2.0/24")?
        );
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("missing from the route origin index")
        );

        let write = store.db.begin_write()?;
        write
            .open_multimap_table(ORIGINS)?
            .insert(("TEST", "route", "AS64496"), "192.0.3.0/24")?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("points to a missing route")
        );
        Ok(())
    }

    #[test]
    fn readers_observe_old_snapshot_until_commit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST"), String::from("TEST2")];
        store.replace_source("TEST", [Ok(object("TEST", "aut-num", "AS64496"))])?;
        store.replace_source("TEST2", [Ok(object("TEST2", "aut-num", "AS64496"))])?;

        let new_snapshot = std::iter::once_with(|| {
            assert_eq!(store.exact("AS64496", &sources, 10).unwrap().len(), 2);
            assert!(store.exact("AS64497", &sources, 10).unwrap().is_empty());
            Ok(object("TEST", "aut-num", "AS64497"))
        });
        store.replace_source("TEST", new_snapshot)?;

        let old = store.exact("AS64496", &sources, 10)?;
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].source, "TEST2");
        assert_eq!(store.exact("AS64497", &sources, 10)?.len(), 1);
        Ok(())
    }

    #[test]
    fn process_crash_preserves_commit_and_rolls_back_replacement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let marker = path.with_extension("ready");
        let mut child = CrashChild(
            Command::new(env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "storage::tests::crash_replacement_child",
                ])
                .env(CRASH_TEST_DATABASE, &path)
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.0.try_wait()? {
                anyhow::bail!(
                    "crash test child exited before reaching the interruption point: {status}"
                );
            }
            if fs::read(&marker).is_ok_and(|bytes| bytes == CRASH_TEST_READY) {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for interrupted replacement"
            );
            thread::sleep(Duration::from_millis(20));
        }
        child.0.kill()?;
        ensure!(
            !child.0.wait()?.success(),
            "crash test child was not terminated"
        );

        let store = Store::open_existing(&path)?;
        let sources = [String::from("TEST")];
        let old = "192.0.2.0/24AS64496";
        let new = "198.51.100.0/24AS64496";
        assert_eq!(
            store.exact(old, &sources, 2)?,
            [object("TEST", "route", old)]
        );
        assert!(store.exact(new, &sources, 2)?.is_empty());
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 2)?,
            ["192.0.2.0/24"]
        );
        assert_eq!(store.verify()?, 1);
        Ok(())
    }

    #[test]
    #[ignore = "subprocess helper for process_crash_preserves_commit_and_rolls_back_replacement"]
    fn crash_replacement_child() -> Result<()> {
        let Some(path) = env::var_os(CRASH_TEST_DATABASE).map(PathBuf::from) else {
            return Ok(());
        };
        let store = Store::open(&path)?;
        store.replace_source("TEST", [Ok(object("TEST", "route", "192.0.2.0/24AS64496"))])?;
        let replacement = std::iter::once(Ok(object("TEST", "route", "198.51.100.0/24AS64496")))
            .chain(std::iter::once_with(|| {
                // The first object and both index entries have been inserted by now.
                let mut marker = fs::File::create(path.with_extension("ready"))?;
                marker.write_all(CRASH_TEST_READY)?;
                marker.flush()?;
                // Exit even if the parent itself is interrupted before it can clean up.
                thread::sleep(Duration::from_secs(60));
                anyhow::bail!("crash test child was not terminated")
            }));
        store.replace_source("TEST", replacement)?;
        anyhow::bail!("crash test child unexpectedly finished the replacement")
    }
}
