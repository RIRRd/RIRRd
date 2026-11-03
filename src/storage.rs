//! Durable, indexed storage for locally imported source snapshots.

use std::{
    collections::HashSet,
    io::Write,
    net::{Ipv4Addr, Ipv6Addr},
    ops::RangeInclusive,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use redb::{
    Database, Durability, MultimapTable, MultimapTableDefinition, MultimapTableHandle,
    ReadOnlyTable, ReadTransaction, ReadableMultimapTable, ReadableTable, ReadableTableMetadata,
    Table, TableDefinition, TableHandle, WriteTransaction,
};

use crate::{
    nrtm3::{Kind, Operation},
    rpsl::{Object, lookup_attributes},
};

type Identity<'a> = (&'a str, &'a str, &'a str);
type ExactKey = (&'static str, &'static str);
type PrefixKey<'a> = (&'a str, &'a str, u128, u8, u32);
type InverseKey = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
type JournalKey = (&'static str, u64);
type JournalValue = (bool, &'static str, &'static str, &'static str);

// Identity is (source, class, key). The exact index maps (source, key) to classes.
const OBJECTS: TableDefinition<Identity<'static>, &str> = TableDefinition::new("objects_v1");
const EXACT: MultimapTableDefinition<ExactKey, &str> = MultimapTableDefinition::new("exact_v1");
// The origin index maps (source, class, origin) to distinct canonical prefixes.
const ORIGINS: MultimapTableDefinition<Identity<'static>, &str> =
    MultimapTableDefinition::new("origins_v1");
// Per-source order by network address permits one range scan for all descendants.
const PREFIXES: TableDefinition<PrefixKey<'static>, ()> = TableDefinition::new("route_prefixes_v1");
// A row is (source, field, normalized value, class, object key).
const INVERSE: TableDefinition<InverseKey, ()> = TableDefinition::new("inverse_v1");
const SERIALS: TableDefinition<&str, u64> = TableDefinition::new("source_serials_v1");
// The journal value is (is_add, object class, object key, raw object text).
const JOURNAL: TableDefinition<JournalKey, JournalValue> = TableDefinition::new("nrtm3_journal_v1");
const NRTM3_RETAINED_OPERATIONS: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixMatch {
    Exact,
    ClosestCovering,
    AllCovering,
    MoreSpecific,
}

pub struct Store {
    db: Database,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).with_context(|| format!("open {}", path.display()))?;
        Self::initialize(db)
    }

    /// Open an existing registry database and upgrade its indexes before serving queries.
    pub fn open_for_service(path: &Path) -> Result<Self> {
        let db = Database::open(path).with_context(|| format!("open {}", path.display()))?;
        {
            let read = db.begin_read()?;
            read.open_table(OBJECTS)
                .context("missing or invalid registry objects table")?;
            read.open_multimap_table(EXACT)
                .context("missing or invalid registry exact lookup index")?;
        }
        Self::initialize(db)
    }

    fn initialize(db: Database) -> Result<Self> {
        let mut write = db.begin_write()?;
        write.set_durability(Durability::Immediate);
        {
            let records = write.open_table(OBJECTS)?;
            write.open_multimap_table(EXACT)?;
            let backfill_origins = !write
                .list_multimap_tables()?
                .any(|table| table.name() == ORIGINS.name());
            let tables: HashSet<_> = write
                .list_tables()?
                .map(|table| table.name().to_owned())
                .collect();
            let backfill_prefixes = !tables.contains(PREFIXES.name());
            let backfill_inverse = !tables.contains(INVERSE.name());
            let mut origins = write.open_multimap_table(ORIGINS)?;
            let mut prefixes = write.open_table(PREFIXES)?;
            let mut inverse = write.open_table(INVERSE)?;
            write.open_table(SERIALS)?;
            write.open_table(JOURNAL)?;
            if backfill_origins || backfill_prefixes || backfill_inverse {
                for entry in records.iter()? {
                    let (identity, text) = entry?;
                    let (source, class, key) = identity.value();
                    if let Some((prefix, origin)) = route_index_parts(class, key)? {
                        if backfill_origins {
                            origins.insert((source, class, origin), prefix)?;
                        }
                        if backfill_prefixes {
                            prefixes
                                .insert(prefix_index_key(source, class, prefix, origin)?, ())?;
                        }
                    }
                    if backfill_inverse {
                        let attributes =
                            lookup_attributes(class, text.value()).map_err(anyhow::Error::msg)?;
                        for (field, value) in attributes.inverse_entries() {
                            inverse.insert((source, field, value.as_ref(), class, key), ())?;
                        }
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

    /// Compact an offline registry after validating its application indexes and metadata.
    pub fn compact(&mut self) -> Result<bool> {
        let before = self
            .verify()
            .context("verifying registry before compaction")?;
        let compacted = self.db.compact().context("compacting registry database")?;
        let after = self
            .verify()
            .context("verifying registry after compaction")?;
        ensure!(
            before == after,
            "compaction changed the registry object count"
        );
        Ok(compacted)
    }

    /// Publish a source's replacement snapshot and its index in one durable commit.
    /// Readers continue to see the previous committed state while records are processed.
    /// An empty iterator clears the named source.
    pub fn replace_source(
        &self,
        source: &str,
        objects: impl IntoIterator<Item = Result<Object>>,
    ) -> Result<u64> {
        let source = canonical_source(source)?;
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let mut count = 0;
        let mut skipped_lookup_values = 0;
        {
            let mut records = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            write.open_table(SERIALS)?.remove(source.as_str())?;
            write.open_table(JOURNAL)?.retain_in(
                (source.as_str(), 0)..=(source.as_str(), u64::MAX),
                |_, _| false,
            )?;

            // Appending the smallest character gives the exclusive end of this source's range.
            let next_source = format!("{source}\0");
            let old_records = records.extract_from_if(
                (source.as_str(), "", "")..(next_source.as_str(), "", ""),
                |_, _| true,
            )?;
            for entry in old_records {
                let (identity, text) = entry?;
                indexes.remove(identity.value(), text.value())?;
            }

            for object in objects {
                let object = object?;
                let identity = validate_identity(&source, &object)?;
                let (_, class, key) = identity;
                ensure!(
                    records.insert(identity, object.text.as_str())?.is_none(),
                    "duplicate {class} object {key} in {source}"
                );
                skipped_lookup_values += indexes.insert(&object)?;
                count += 1;
            }
        }
        write.commit()?;
        warn_skipped_lookup_values(&source, skipped_lookup_values);
        Ok(count)
    }

    /// Atomically apply one contiguous NRTMv3 response and its last serial.
    pub fn apply_nrtm3(
        &self,
        source: &str,
        declared: (u64, u64),
        initial_base: Option<u64>,
        operations: impl IntoIterator<Item = Result<Operation, String>>,
    ) -> Result<u64> {
        let source = canonical_source(source)?;
        let (start, end) = declared;
        ensure!(start > 0 && end >= start, "invalid NRTMv3 serial range");
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let expire_through = end.saturating_sub(NRTM3_RETAINED_OPERATIONS);
        let mut skipped_lookup_values = 0;
        {
            let mut serials = write.open_table(SERIALS)?;
            let mut journal = write.open_table(JOURNAL)?;
            let mut records = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            let recorded = serials.get(source.as_str())?.map(|serial| serial.value());
            let mut last = if let Some(recorded) = recorded {
                ensure!(
                    initial_base.is_none(),
                    "a base serial is already recorded for {source}"
                );
                recorded
            } else {
                let base = initial_base
                    .context("initial NRTMv3 apply requires the imported snapshot serial")?;
                let next_source = format!("{source}\0");
                ensure!(
                    records
                        .range((source.as_str(), "", "")..(next_source.as_str(), "", ""))?
                        .next()
                        .transpose()?
                        .is_some(),
                    "initial NRTMv3 apply requires an imported snapshot for {source}"
                );
                base
            };
            ensure!(
                last.checked_add(1) == Some(start),
                "NRTMv3 range starts at {start} after serial {last}"
            );

            // The iterator reports terminal framing errors after yielding its last operation.
            for operation in operations {
                let Operation {
                    kind,
                    serial,
                    object,
                } = operation.map_err(anyhow::Error::msg)?;
                ensure!(
                    last.checked_add(1) == Some(serial) && serial <= end,
                    "non-contiguous NRTMv3 serial {serial} after {last}"
                );
                let identity = validate_identity(&source, &object)?;
                match kind {
                    Kind::Add => {
                        if let Some(previous) = records.insert(identity, object.text.as_str())? {
                            indexes.remove(identity, previous.value())?;
                        }
                        skipped_lookup_values += indexes.insert(&object)?;
                    }
                    Kind::Delete => {
                        if let Some(previous) = records.remove(identity)? {
                            indexes.remove(identity, previous.value())?;
                        }
                    }
                }
                if serial > expire_through {
                    let (_, class, key) = identity;
                    ensure!(
                        journal
                            .insert(
                                (source.as_str(), serial),
                                (kind == Kind::Add, class, key, object.text.as_str()),
                            )?
                            .is_none(),
                        "duplicate retained NRTMv3 serial {serial}"
                    );
                }
                last = serial;
            }
            ensure!(
                last == end,
                "NRTMv3 stream ended at serial {last}, expected {end}"
            );
            journal.retain_in(
                (source.as_str(), 0)..=(source.as_str(), expire_through),
                |_, _| false,
            )?;
            serials.insert(source.as_str(), end)?;
        }
        write.commit()?;
        warn_skipped_lookup_values(&source, skipped_lookup_values);
        Ok(end)
    }

    pub fn source_serial(&self, source: &str) -> Result<Option<u64>> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        read.open_table(OBJECTS)
            .context("missing or invalid registry objects table")?;
        let serials = match read.open_table(SERIALS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(serials.get(source.as_str())?.map(|serial| serial.value()))
    }

    /// Read one retained contiguous interval and its ending serial from a single snapshot.
    /// Returned operation text is raw and can contain sensitive attributes.
    pub fn nrtm3_journal(
        &self,
        source: &str,
        start: u64,
        end: Option<u64>,
        max_entries: usize,
        max_bytes: usize,
    ) -> Result<Option<(u64, Vec<Operation>)>> {
        let source = canonical_source(source)?;
        ensure!(start > 0, "invalid NRTMv3 journal range");
        let read = self.db.begin_read()?;
        let serials = read
            .open_table(SERIALS)
            .context("NRTMv3 journal unavailable")?;
        let committed = serials
            .get(source.as_str())?
            .context("NRTMv3 journal unavailable for source")?
            .value();
        if end.is_none() && committed.checked_add(1) == Some(start) {
            return Ok(None);
        }
        let end = end.unwrap_or(committed);
        ensure!(
            start <= end && end <= committed,
            "NRTMv3 journal range unavailable"
        );
        ensure!(
            start > committed.saturating_sub(NRTM3_RETAINED_OPERATIONS),
            "NRTMv3 journal range unavailable"
        );
        let count = usize::try_from(end - start + 1)?;
        ensure!(count <= max_entries, "NRTMv3 journal entry limit exceeded");
        let journal = read
            .open_table(JOURNAL)
            .context("NRTMv3 journal unavailable")?;
        let mut operations = Vec::new();
        let mut remaining_bytes = max_bytes;
        for entry in journal.range((source.as_str(), start)..=(source.as_str(), end))? {
            let (key, value) = entry?;
            let (_, serial) = key.value();
            ensure!(
                serial == start + operations.len() as u64,
                "NRTMv3 journal range unavailable"
            );
            let (is_add, class, key, text) = value.value();
            remaining_bytes = remaining_bytes
                .checked_sub(text.len())
                .context("NRTMv3 journal byte limit exceeded")?;
            operations.push(Operation {
                kind: if is_add { Kind::Add } else { Kind::Delete },
                serial,
                object: Object {
                    source: source.clone(),
                    class: class.to_owned(),
                    key: key.to_owned(),
                    text: text.to_owned(),
                },
            });
        }
        ensure!(
            operations.len() == count,
            "NRTMv3 journal range unavailable"
        );
        Ok(Some((end, operations)))
    }

    /// Find up to `limit` objects by their key in the given source priority order.
    pub fn exact(&self, key: &str, sources: &[String], limit: usize) -> Result<Vec<Object>> {
        self.exact_keys(std::iter::once(key.to_uppercase()), sources, &[], limit)
    }

    /// Find objects for all keys in one read snapshot, optionally restricted by class.
    pub fn exact_objects(
        &self,
        keys: &[String],
        sources: &[String],
        classes: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        let mut seen = HashSet::new();
        let keys = keys
            .iter()
            .map(|key| key.to_uppercase())
            .filter(|key| seen.insert(key.clone()));
        self.exact_keys(keys, sources, classes, limit)
    }

    fn exact_keys(
        &self,
        keys: impl IntoIterator<Item = String>,
        sources: &[String],
        classes: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        let read = self.db.begin_read()?;
        let exact = read.open_multimap_table(EXACT)?;
        let records = read.open_table(OBJECTS)?;
        let mut objects = Vec::new();
        for key in keys {
            for source in sources {
                for entry in exact.get((source.as_str(), key.as_str()))? {
                    let entry = entry?;
                    let class = entry.value();
                    if classes.is_empty() || classes.iter().any(|filter| filter == class) {
                        let object = read_object(&records, source, class, &key)?
                            .context("exact lookup index points to a missing object")?;
                        objects.push(object);
                        if objects.len() == limit {
                            return Ok(objects);
                        }
                    }
                }
            }
        }
        Ok(objects)
    }

    /// Find objects containing the exact lookup attribute value in source priority order.
    pub fn inverse(
        &self,
        field: &str,
        value: &str,
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        let field = field.to_ascii_lowercase();
        let value = value.to_uppercase();
        if field == "origin" {
            return self.routes_by_origin(&value, sources, limit);
        }
        let next_value = format!("{value}\0");
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let inverse = read.open_table(INVERSE)?;
        let mut objects = Vec::new();
        for source in sources {
            let source = source.as_str();
            let lower = (source, field.as_str(), value.as_str(), "", "");
            let upper = (source, field.as_str(), next_value.as_str(), "", "");
            for entry in inverse.range(lower..upper)?.take(limit - objects.len()) {
                let (identity, _) = entry?;
                let (source, _, _, class, key) = identity.value();
                let object = read_object(&records, source, class, key)?
                    .context("inverse lookup index points to a missing object")?;
                objects.push(object);
            }
            if objects.len() == limit {
                break;
            }
        }
        Ok(objects)
    }

    fn routes_by_origin(
        &self,
        origin: &str,
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let mut routes = Vec::new();
        for source in sources {
            for class in ["route", "route6"] {
                for prefix in origins
                    .get((source.as_str(), class, origin))?
                    .take(limit - routes.len())
                {
                    let prefix = prefix?;
                    let key = format!("{}{origin}", prefix.value());
                    let route = read_object(&records, source, class, &key)?
                        .context("route origin index points to a missing route")?;
                    routes.push(route);
                }
                if routes.len() == limit {
                    return Ok(routes);
                }
            }
        }
        Ok(routes)
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

    /// Find routes with the requested prefix relationship in a single read snapshot.
    pub fn routes_matching_prefix(
        &self,
        class: &str,
        prefix: &str,
        mode: PrefixMatch,
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Object>> {
        if limit == 0 || sources.is_empty() {
            return Ok(Vec::new());
        }
        if mode == PrefixMatch::Exact {
            return self.routes_at_prefix(class, prefix, sources, limit);
        }
        let prefix = RoutePrefix::parse(class, prefix)?;
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let prefixes = read.open_table(PREFIXES)?;
        let mut routes = Vec::new();
        let mut collect = |range: RangeInclusive<PrefixKey<'_>>| -> Result<usize> {
            for entry in prefixes.range(range)?.take(limit - routes.len()) {
                let (index_key, _) = entry?;
                let (source, class, network, length, origin) = index_key.value();
                let key = route_key(class, network, length, origin)?;
                let object = read_object(&records, source, class, &key)?
                    .context("route prefix index points to a missing route")?;
                routes.push(object);
            }
            Ok(routes.len())
        };
        let covering = |source, length| {
            let network = prefix.network_at(length);
            (source, class, network, length, 0)..=(source, class, network, length, u32::MAX)
        };
        match mode {
            PrefixMatch::Exact => unreachable!(),
            PrefixMatch::ClosestCovering => {
                for length in (0..prefix.length).rev() {
                    let mut count = 0;
                    for source in sources {
                        count = collect(covering(source.as_str(), length))?;
                        if count == limit {
                            break;
                        }
                    }
                    if count > 0 {
                        break;
                    }
                }
            }
            PrefixMatch::AllCovering => {
                'sources: for source in sources {
                    for length in (0..=prefix.length).rev() {
                        if collect(covering(source.as_str(), length))? == limit {
                            break 'sources;
                        }
                    }
                }
            }
            PrefixMatch::MoreSpecific => {
                if prefix.length == prefix.width {
                    return Ok(routes);
                }
                let last = prefix.network | prefix.host_mask(prefix.length);
                for source in sources {
                    let source = source.as_str();
                    // A longer prefix starting inside a CIDR block is contained in it.
                    // A shorter prefix can only share the block's first address.
                    let lower = (source, class, prefix.network, prefix.length + 1, 0);
                    let upper = (source, class, last, prefix.width, u32::MAX);
                    if collect(lower..=upper)? == limit {
                        break;
                    }
                }
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

    /// Check index coverage and retained journal metadata.
    pub fn verify(&self) -> Result<u64> {
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let exact = read.open_multimap_table(EXACT)?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let prefixes = read.open_table(PREFIXES)?;
        let inverse = read.open_table(INVERSE)?;
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
        for entry in prefixes.iter()? {
            let (identity, _) = entry?;
            let (source, class, network, length, origin) = identity.value();
            let key = route_key(class, network, length, origin)?;
            ensure!(
                records.get((source, class, key.as_str()))?.is_some(),
                "route prefix index points to a missing route"
            );
        }
        ensure!(
            prefixes.len()? == routes,
            "some routes are missing from the route prefix index"
        );
        let mut indexed_attributes = 0;
        for entry in records.iter()? {
            let (identity, text) = entry?;
            let (source, class, key) = identity.value();
            let attributes = lookup_attributes(class, text.value()).map_err(anyhow::Error::msg)?;
            for (field, value) in attributes.inverse_entries().collect::<HashSet<_>>() {
                ensure!(
                    inverse
                        .get((source, field, value.as_ref(), class, key))?
                        .is_some(),
                    "object is missing from the inverse lookup index"
                );
                indexed_attributes += 1;
            }
        }
        ensure!(
            inverse.len()? == indexed_attributes,
            "unexpected inverse lookup index entries"
        );
        verify_journal(&read)?;
        Ok(indexed)
    }
}

fn verify_journal(read: &ReadTransaction) -> Result<()> {
    let journal = match read.open_table(JOURNAL) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let serials = read.open_table(SERIALS)?;
    let mut previous: Option<(String, u64, u64)> = None;
    for entry in journal.iter()? {
        let (key, _) = entry?;
        let (source, serial) = key.value();
        if let Some((previous_source, last, _)) = &mut previous
            && previous_source == source
        {
            ensure!(
                last.checked_add(1) == Some(serial),
                "NRTMv3 journal has a serial gap for {source}"
            );
            *last = serial;
            continue;
        }
        if let Some((previous_source, last, committed)) = previous {
            ensure!(
                last == committed,
                "NRTMv3 journal does not end at the committed serial for {previous_source}"
            );
        }
        let committed = serials
            .get(source)?
            .context("NRTMv3 journal has no corresponding source serial")?
            .value();
        ensure!(
            serial > committed.saturating_sub(NRTM3_RETAINED_OPERATIONS),
            "NRTMv3 journal exceeds retention for {source}"
        );
        previous = Some((source.to_owned(), serial, committed));
    }
    if let Some((source, last, committed)) = previous {
        ensure!(
            last == committed,
            "NRTMv3 journal does not end at the committed serial for {source}"
        );
    }
    Ok(())
}

struct WriteIndexes<'txn> {
    exact: MultimapTable<'txn, ExactKey, &'static str>,
    origins: MultimapTable<'txn, Identity<'static>, &'static str>,
    prefixes: Table<'txn, PrefixKey<'static>, ()>,
    inverse: Table<'txn, InverseKey, ()>,
}

impl<'txn> WriteIndexes<'txn> {
    fn open(write: &'txn WriteTransaction) -> Result<Self> {
        Ok(Self {
            exact: write.open_multimap_table(EXACT)?,
            origins: write.open_multimap_table(ORIGINS)?,
            prefixes: write.open_table(PREFIXES)?,
            inverse: write.open_table(INVERSE)?,
        })
    }

    fn insert(&mut self, object: &Object) -> Result<usize> {
        let source = object.source.as_str();
        let class = object.class.as_str();
        let key = object.key.as_str();
        ensure!(
            !self.exact.insert((source, key), class)?,
            "unexpected duplicate entry in the exact lookup index"
        );
        if let Some((prefix, origin)) = route_index_parts(class, key)? {
            ensure!(
                !self.origins.insert((source, class, origin), prefix)?,
                "unexpected duplicate entry in the route origin index"
            );
            ensure!(
                self.prefixes
                    .insert(prefix_index_key(source, class, prefix, origin)?, ())?
                    .is_none(),
                "unexpected duplicate entry in the route prefix index"
            );
        }
        let attributes = object.lookup_attributes().map_err(anyhow::Error::msg)?;
        for (field, value) in attributes.inverse_entries() {
            self.inverse
                .insert((source, field, value.as_ref(), class, key), ())?;
        }
        Ok(attributes.skipped_invalid_values)
    }

    fn remove(&mut self, (source, class, key): Identity<'_>, text: &str) -> Result<()> {
        ensure!(
            self.exact.remove((source, key), class)?,
            "object is missing from the exact lookup index"
        );
        if let Some((prefix, origin)) = route_index_parts(class, key)? {
            ensure!(
                self.origins.remove((source, class, origin), prefix)?,
                "object is missing from the route origin index"
            );
            ensure!(
                self.prefixes
                    .remove(prefix_index_key(source, class, prefix, origin)?)?
                    .is_some(),
                "object is missing from the route prefix index"
            );
        }
        let attributes = lookup_attributes(class, text).map_err(anyhow::Error::msg)?;
        for (field, value) in attributes.inverse_entries().collect::<HashSet<_>>() {
            ensure!(
                self.inverse
                    .remove((source, field, value.as_ref(), class, key))?
                    .is_some(),
                "object is missing from the inverse lookup index"
            );
        }
        Ok(())
    }
}

fn canonical_source(source: &str) -> Result<String> {
    ensure!(
        !source.is_empty() && !source.contains('\0'),
        "invalid source name"
    );
    Ok(source.to_ascii_uppercase())
}

fn validate_identity<'a>(source: &str, object: &'a Object) -> Result<Identity<'a>> {
    ensure!(
        object.source == source,
        "object source does not match {source}"
    );
    ensure!(
        !object.class.is_empty() && !object.key.is_empty(),
        "empty object identity"
    );
    Ok((&object.source, &object.class, &object.key))
}

fn warn_skipped_lookup_values(source: &str, skipped_lookup_values: usize) {
    if skipped_lookup_values != 0 {
        tracing::warn!(%source, skipped_lookup_values, "retained malformed lookup attribute values in stored object text without indexing them");
    }
}

struct RoutePrefix {
    network: u128,
    length: u8,
    width: u8,
}

impl RoutePrefix {
    fn parse(class: &str, prefix: &str) -> Result<Self> {
        let (address, length) = prefix.split_once('/').context("invalid route prefix")?;
        let length: u8 = length.parse().context("invalid route prefix length")?;
        let (network, width) = match class {
            "route" => (u128::from(u32::from(address.parse::<Ipv4Addr>()?)), 32),
            "route6" => (u128::from(address.parse::<Ipv6Addr>()?), 128),
            _ => anyhow::bail!("invalid route class"),
        };
        ensure!(length <= width, "invalid route prefix length");
        let route = Self {
            network,
            length,
            width,
        };
        ensure!(
            route.network_at(length) == network,
            "route prefix contains host bits"
        );
        Ok(route)
    }

    fn host_mask(&self, length: u8) -> u128 {
        u128::MAX
            .checked_shr(u32::from(128 - (self.width - length)))
            .unwrap_or(0)
    }

    fn network_at(&self, length: u8) -> u128 {
        self.network & !self.host_mask(length)
    }
}

fn prefix_index_key<'a>(
    source: &'a str,
    class: &'a str,
    prefix: &str,
    origin: &str,
) -> Result<PrefixKey<'a>> {
    let prefix = RoutePrefix::parse(class, prefix)?;
    let origin = origin
        .strip_prefix("AS")
        .context("invalid route origin")?
        .parse()?;
    Ok((source, class, prefix.network, prefix.length, origin))
}

fn route_key(class: &str, network: u128, length: u8, origin: u32) -> Result<String> {
    match class {
        "route" => Ok(format!(
            "{}/{length}AS{origin}",
            Ipv4Addr::from(u32::try_from(network)?)
        )),
        "route6" => {
            Ok(format!("{}/{length}AS{origin}", Ipv6Addr::from(network)).to_ascii_uppercase())
        }
        _ => anyhow::bail!("invalid route class in prefix index"),
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
    records: &ReadOnlyTable<Identity<'static>, &str>,
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
    fn batch_exact_filters_before_limit_and_deduplicates_keys() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        let low = object("TEST", "role", "AA1-TEST");
        let later = object("TEST", "role", "AA2-TEST");
        store.replace_source("TEST", [Ok(later.clone()), Ok(low.clone())])?;
        store.replace_source("TEST2", [Ok(object("TEST2", "person", "AA1-TEST"))])?;
        let keys = [
            String::from("aa1-test"),
            String::from("AA1-TEST"),
            String::from("AA2-TEST"),
        ];
        let classes = [String::from("role")];
        assert_eq!(
            store.exact_objects(&keys, &sources, &classes, 2)?,
            [low, later]
        );
        Ok(())
    }

    #[test]
    fn inverse_queries_preserve_valid_values_and_reuse_origin_index() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        let mut high = object("TEST2", "route6", "2001:DB8::/32AS64496");
        high.text
            .push_str("mnt-by: Example-MNT, bad value!, EXAMPLE-MNT\n");
        let mut low = object("TEST", "route", "192.0.2.0/24AS64496");
        low.text.push_str("mnt-by: Other-MNT,\n+ EXAMPLE-MNT\n");
        let mut unrelated = object("TEST", "aut-num", "AS64496");
        unrelated.text.push_str("mnt-by: EXAMPLE-MNT2\n");
        store.replace_source("TEST2", [Ok(high.clone())])?;
        store.replace_source("TEST", [Ok(low.clone()), Ok(unrelated)])?;

        let expected = [high, low];
        assert_eq!(
            store.inverse("MNT-BY", "example-mnt", &sources, 10)?,
            expected
        );
        assert_eq!(
            store.inverse("mnt-by", "example-mnt", &sources, 1)?,
            expected[..1]
        );
        assert_eq!(
            store.inverse("mnt-by", "example-mnt", &sources[1..], 10)?,
            expected[1..]
        );
        assert!(
            store
                .inverse("mnt-by", "bad value!", &sources, 10)?
                .is_empty()
        );
        assert_eq!(store.inverse("origin", "as64496", &sources, 10)?, expected);
        assert_eq!(store.verify()?, 3);
        Ok(())
    }

    #[test]
    fn inverse_index_backfills_and_replaces_atomically() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let sources = [String::from("TEST")];
        let mut old = object("TEST", "aut-num", "AS64496");
        old.text.push_str("mnt-by: TEST-MNT, TEST-MNT\n");
        let mut new = object("TEST", "aut-num", "AS64497");
        new.text.push_str("mnt-by: TEST-MNT\n");
        let store = Store::open(&path)?;
        store.replace_source("TEST", [Ok(old.clone())])?;
        let write = store.db.begin_write()?;
        assert!(write.delete_table(INVERSE)?);
        write.commit()?;
        drop(store);

        let store = Store::open_for_service(&path)?;
        let query = || store.inverse("mnt-by", "TEST-MNT", &sources, 2);
        assert_eq!(query()?, std::slice::from_ref(&old));
        assert!(
            store
                .replace_source(
                    "TEST",
                    [Ok(new.clone()), Err(anyhow::anyhow!("import failed"))]
                )
                .is_err()
        );
        assert_eq!(query()?, std::slice::from_ref(&old));
        let replacement = std::iter::once_with(|| {
            assert_eq!(query().unwrap(), [old]);
            Ok(new.clone())
        });
        store.replace_source("TEST", replacement)?;
        assert_eq!(query()?, [new]);
        assert_eq!(store.verify()?, 1);

        let write = store.db.begin_write()?;
        write
            .open_table(INVERSE)?
            .insert(("TEST", "mnt-by", "OTHER-MNT", "aut-num", "AS64497"), ())?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("unexpected inverse lookup index entries")
        );
        Ok(())
    }

    #[test]
    fn nrtm3_apply_updates_indexes_and_retains_source_serial_across_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let sources = [String::from("TEST")];
        let replaced_key = "192.0.2.0/24AS64496";
        let deleted_key = "192.0.3.0/24AS64496";
        let mut previous = object("TEST", "route", replaced_key);
        previous.text.push_str("mnt-by: OLD-MNT\n");
        let mut updated = object("TEST", "route", replaced_key);
        updated.text.push_str("mnt-by: NEW-MNT\n");
        let deleted = object("TEST", "route", deleted_key);
        let add = Operation {
            kind: Kind::Add,
            serial: 42,
            object: updated.clone(),
        };
        let delete = Operation {
            kind: Kind::Delete,
            serial: 43,
            object: deleted.clone(),
        };
        assert!(store.apply_nrtm3("TEST", (42, 43), Some(41), []).is_err());
        store.replace_source("TEST", [Ok(previous), Ok(deleted)])?;
        store.replace_source("OTHER", [Ok(object("OTHER", "route", deleted_key))])?;
        assert!(store.apply_nrtm3("TEST", (42, 43), None, []).is_err());
        assert_eq!(
            store.apply_nrtm3("TEST", (42, 43), Some(41), [Ok(add), Ok(delete)])?,
            43
        );
        drop(store);

        let store = Store::open_existing(&path)?;
        assert_eq!(store.source_serial("test")?, Some(43));
        assert_eq!(store.source_serial("OTHER")?, None);
        assert_eq!(
            store.exact(replaced_key, &sources, 2)?,
            std::slice::from_ref(&updated)
        );
        assert!(store.exact(deleted_key, &sources, 2)?.is_empty());
        assert_eq!(
            store.exact(deleted_key, &[String::from("OTHER")], 2)?.len(),
            1
        );
        assert!(store.inverse("mnt-by", "OLD-MNT", &sources, 2)?.is_empty());
        assert_eq!(
            store.inverse("mnt-by", "NEW-MNT", &sources, 2)?,
            std::slice::from_ref(&updated)
        );
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 2)?,
            ["192.0.2.0/24"]
        );
        assert_eq!(
            store.routes_matching_prefix(
                "route",
                "192.0.2.0/23",
                PrefixMatch::MoreSpecific,
                &sources,
                2
            )?,
            [updated]
        );
        assert_eq!(store.verify()?, 2);
        assert!(store.apply_nrtm3("TEST", (42, 43), None, []).is_err());
        assert!(store.apply_nrtm3("TEST", (44, 44), Some(43), []).is_err());
        Ok(())
    }

    #[test]
    fn nrtm3_bad_batches_roll_back_and_snapshot_replacement_resets_serial() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST")];
        let old = object("TEST", "aut-num", "AS64496");
        store.replace_source("TEST", [Ok(old.clone())])?;
        let absent = object("TEST", "aut-num", "AS64500");
        assert_eq!(
            store.apply_nrtm3(
                "TEST",
                (10, 10),
                Some(9),
                [Ok(Operation {
                    kind: Kind::Delete,
                    serial: 10,
                    object: absent
                })]
            )?,
            10
        );

        let delete = || {
            Ok(Operation {
                kind: Kind::Delete,
                serial: 11,
                object: old.clone(),
            })
        };
        let add = |source, serial| {
            Ok(Operation {
                kind: Kind::Add,
                serial,
                object: object(source, "aut-num", "AS64497"),
            })
        };
        for bad in [
            [delete(), Err(String::from("missing END"))],
            [delete(), add("OTHER", 12)],
            [delete(), add("TEST", 11)],
            [delete(), add("TEST", 13)],
        ] {
            assert!(store.apply_nrtm3("TEST", (11, 12), None, bad).is_err());
            assert_eq!(store.source_serial("TEST")?, Some(10));
            assert_eq!(
                store.exact("AS64496", &sources, 2)?,
                std::slice::from_ref(&old)
            );
            assert!(store.exact("AS64497", &sources, 2)?.is_empty());
        }
        // A terminal parse error after the declared final operation must still roll back.
        assert!(
            store
                .apply_nrtm3(
                    "TEST",
                    (11, 11),
                    None,
                    [delete(), Err(String::from("trailing data"))]
                )
                .is_err()
        );
        assert!(
            store
                .apply_nrtm3("TEST", (11, 12), None, [delete()])
                .is_err()
        );
        assert!(
            store
                .replace_source("TEST", [Err(anyhow::anyhow!("snapshot failed"))])
                .is_err()
        );
        assert_eq!(store.source_serial("TEST")?, Some(10));
        store.replace_source("TEST", [Ok(old)])?;
        assert_eq!(store.source_serial("TEST")?, None);
        assert_eq!(
            store.apply_nrtm3("TEST", (11, 11), Some(10), [add("TEST", 11)])?,
            11
        );
        store.replace_source("TEST", [])?;
        assert_eq!(store.source_serial("TEST")?, None);
        assert_eq!(store.verify()?, 0);
        Ok(())
    }

    #[test]
    fn nrtm3_journal_reopens_with_caps_and_rolls_back_with_source_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let old = object("TEST", "aut-num", "AS64496");
        let new = object("TEST", "aut-num", "AS64497");
        store.replace_source("TEST", [Ok(old.clone())])?;
        store.apply_nrtm3(
            "TEST",
            (2, 3),
            Some(1),
            [
                Ok(Operation {
                    kind: Kind::Add,
                    serial: 2,
                    object: new.clone(),
                }),
                Ok(Operation {
                    kind: Kind::Delete,
                    serial: 3,
                    object: old.clone(),
                }),
            ],
        )?;
        let rollback = [
            Ok(Operation {
                kind: Kind::Delete,
                serial: 4,
                object: new.clone(),
            }),
            Err(String::from("invalid END")),
        ];
        assert!(store.apply_nrtm3("TEST", (4, 4), None, rollback).is_err());
        assert!(
            store
                .replace_source("TEST", [Err(anyhow::anyhow!("failed snapshot"))])
                .is_err()
        );
        drop(store);

        let store = Store::open_existing(&path)?;
        let bytes = old.text.len() + new.text.len();
        let expected = [
            Operation {
                kind: Kind::Add,
                serial: 2,
                object: new,
            },
            Operation {
                kind: Kind::Delete,
                serial: 3,
                object: old.clone(),
            },
        ];
        let (end, operations) = store.nrtm3_journal("test", 2, None, 2, bytes)?.unwrap();
        assert_eq!(end, 3);
        assert_eq!(operations, expected);
        assert_eq!(store.verify()?, 1);
        assert_eq!(
            store
                .nrtm3_journal("TEST", 2, Some(2), 2, bytes)?
                .unwrap()
                .1,
            expected[..1]
        );
        assert_eq!(store.nrtm3_journal("TEST", 4, None, 0, 0)?, None);
        assert!(store.nrtm3_journal("TEST", 4, Some(4), 2, bytes).is_err());
        assert!(store.nrtm3_journal("TEST", 2, Some(4), 2, bytes).is_err());
        assert!(store.nrtm3_journal("TEST", 5, None, 2, bytes).is_err());
        assert!(store.nrtm3_journal("TEST", 1, None, 3, bytes).is_err());
        assert!(
            store
                .nrtm3_journal("TEST", 2, None, 1, bytes)
                .unwrap_err()
                .to_string()
                .contains("entry limit")
        );
        assert!(
            store
                .nrtm3_journal("TEST", 2, None, 2, bytes - 1)
                .unwrap_err()
                .to_string()
                .contains("byte limit")
        );

        // Reusing an earlier serial after replacement would collide with any stale history.
        store.replace_source("TEST", [Ok(old.clone())])?;
        store.apply_nrtm3(
            "TEST",
            (2, 2),
            Some(1),
            [Ok(Operation {
                kind: Kind::Delete,
                serial: 2,
                object: old,
            })],
        )?;
        assert_eq!(
            store.nrtm3_journal("TEST", 2, None, 2, bytes)?.unwrap().1[0].kind,
            Kind::Delete
        );
        store.replace_source("TEST", [])?;
        assert!(store.nrtm3_journal("TEST", 2, None, 2, bytes).is_err());
        let read = store.db.begin_read()?;
        assert!(read.open_table(JOURNAL)?.is_empty()?);
        Ok(())
    }

    #[test]
    fn nrtm3_journal_expires_only_old_records_from_the_named_source() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let delete = |source, serial| {
            Ok(Operation {
                kind: Kind::Delete,
                serial,
                object: object(source, "aut-num", "AS64497"),
            })
        };
        for source in ["TEST", "TEST2"] {
            store.replace_source(source, [Ok(object(source, "aut-num", "AS64496"))])?;
            store.apply_nrtm3(
                source,
                (1, 2),
                Some(0),
                [delete(source, 1), delete(source, 2)],
            )?;
        }
        let end = NRTM3_RETAINED_OPERATIONS + 4;
        store.apply_nrtm3(
            "TEST",
            (3, end),
            None,
            (3..=end).map(|serial| delete("TEST", serial)),
        )?;
        assert!(
            store
                .nrtm3_journal("TEST", 4, None, 4097, usize::MAX)
                .is_err()
        );
        let (_, retained) = store
            .nrtm3_journal("TEST", 5, None, 4096, usize::MAX)?
            .unwrap();
        assert_eq!(retained.len(), 4096);
        assert_eq!(retained.first().unwrap().serial, 5);
        assert_eq!(retained.last().unwrap().serial, end);
        assert_eq!(
            store
                .nrtm3_journal("TEST2", 1, None, 2, usize::MAX)?
                .unwrap()
                .1
                .len(),
            2
        );

        assert_eq!(store.verify()?, 2);
        let write = store.db.begin_write()?;
        write.open_table(JOURNAL)?.remove(("TEST", 6))?;
        write.commit()?;
        assert!(
            store
                .nrtm3_journal("TEST", 5, Some(7), 3, usize::MAX)
                .is_err()
        );
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("serial gap")
        );

        let write = store.db.begin_write()?;
        let operation = &retained[1].object;
        write.open_table(JOURNAL)?.insert(
            ("TEST", 6),
            (
                false,
                operation.class.as_str(),
                operation.key.as_str(),
                operation.text.as_str(),
            ),
        )?;
        write.open_table(SERIALS)?.insert("TEST", end + 1)?;
        // Remove the oldest row to keep this synthetic history within the retention window.
        write.open_table(JOURNAL)?.remove(("TEST", 5))?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("does not end at the committed serial")
        );

        let write = store.db.begin_write()?;
        write.open_table(SERIALS)?.remove("TEST")?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("no corresponding source serial")
        );
        Ok(())
    }

    #[test]
    fn nrtm3_journal_without_backfill_reports_old_serial_as_unavailable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let old = object("TEST", "aut-num", "AS64496");
        store.replace_source("TEST", [Ok(old.clone())])?;
        store.apply_nrtm3(
            "TEST",
            (9, 9),
            Some(8),
            [Ok(Operation {
                kind: Kind::Delete,
                serial: 9,
                object: old.clone(),
            })],
        )?;
        let write = store.db.begin_write()?;
        assert!(write.delete_table(JOURNAL)?);
        write.commit()?;
        drop(store);
        let store = Store::open_for_service(&path)?;
        assert_eq!(store.source_serial("TEST")?, Some(9));
        assert_eq!(store.verify()?, 0);
        assert!(store.nrtm3_journal("TEST", 9, None, 1, usize::MAX).is_err());
        assert_eq!(store.nrtm3_journal("TEST", 10, None, 1, usize::MAX)?, None);
        store.apply_nrtm3(
            "TEST",
            (10, 10),
            None,
            [Ok(Operation {
                kind: Kind::Add,
                serial: 10,
                object: old,
            })],
        )?;
        assert_eq!(
            store
                .nrtm3_journal("TEST", 10, None, 1, usize::MAX)?
                .unwrap()
                .0,
            10
        );
        Ok(())
    }

    #[test]
    fn compaction_preserves_sources_and_journal_after_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let route = object("TEST", "route", "192.0.2.0/24AS64496");
        let other = object("TEST2", "aut-num", "AS64497");
        let sources = [String::from("TEST"), String::from("TEST2")];
        let store = Store::open(&path)?;
        let remarks = "x".repeat(8192);
        store.replace_source(
            "TEST",
            (1..=128).map(|asn| {
                let mut record = object("TEST", "aut-num", &format!("AS{asn}"));
                record.text.push_str(&format!("remarks: {remarks}\n"));
                Ok(record)
            }),
        )?;
        store.replace_source("TEST2", [Ok(other.clone())])?;
        store.replace_source("TEST", [Ok(object("TEST", "aut-num", "AS64496"))])?;
        store.apply_nrtm3(
            "TEST",
            (10, 10),
            Some(9),
            [Ok(Operation {
                kind: Kind::Add,
                serial: 10,
                object: route.clone(),
            })],
        )?;
        drop(store);

        let mut store = Store::open_existing(&path)?;
        store.compact()?;
        drop(store);
        let store = Store::open_existing(&path)?;
        assert_eq!(store.verify()?, 3);
        assert_eq!(store.exact("AS64497", &sources, 2)?, [other]);
        assert_eq!(
            store.routes_at_prefix("route", "192.0.2.0/24", &sources, 2)?,
            std::slice::from_ref(&route)
        );
        assert_eq!(store.source_serial("TEST")?, Some(10));
        let (end, journal) = store
            .nrtm3_journal("TEST", 10, None, 1, route.text.len())?
            .unwrap();
        assert_eq!(end, 10);
        assert_eq!(
            journal,
            [Operation {
                kind: Kind::Add,
                serial: 10,
                object: route
            }]
        );
        Ok(())
    }

    #[test]
    fn verifies_index_coverage_and_dangling_references() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        assert!(Store::open_existing(&path).is_err());
        assert!(Store::open_for_service(&path).is_err());
        assert!(!path.exists());
        drop(Database::create(&path)?);
        assert!(Store::open_for_service(&path).is_err());
        assert!(Store::open_existing(&path)?.verify().is_err());
        assert!(
            Store::open_existing(&path)?
                .compact()
                .unwrap_err()
                .to_string()
                .contains("verifying registry before compaction")
        );

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
    fn route_covering_queries_select_the_nearest_prefix_across_sources() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        let high_parent = object("TEST2", "route", "192.0.0.0/16AS64498");
        let high_root = object("TEST2", "route", "0.0.0.0/0AS0");
        let exact = object("TEST", "route", "192.0.2.0/24AS64496");
        let close = object("TEST", "route", "192.0.2.0/23AS64496");
        let close_other_origin = object("TEST", "route", "192.0.2.0/23AS4294967295");
        store.replace_source("TEST2", [Ok(high_parent.clone()), Ok(high_root.clone())])?;
        store.replace_source(
            "TEST",
            [
                Ok(close_other_origin.clone()),
                Ok(exact.clone()),
                Ok(close.clone()),
            ],
        )?;

        let nearest = PrefixMatch::ClosestCovering;
        let all = PrefixMatch::AllCovering;
        assert_eq!(
            store.routes_matching_prefix("route", "192.0.2.0/24", nearest, &sources, 8)?,
            [close.clone(), close_other_origin.clone()]
        );
        assert_eq!(
            store.routes_matching_prefix("route", "192.0.2.0/24", nearest, &sources[..1], 8)?,
            std::slice::from_ref(&high_parent)
        );
        assert_eq!(
            store.routes_matching_prefix("route", "192.0.2.0/24", all, &sources, 8)?,
            [
                high_parent,
                high_root.clone(),
                exact,
                close,
                close_other_origin
            ]
        );
        assert_eq!(
            store.routes_matching_prefix("route", "0.0.0.0/0", all, &sources, 1)?,
            [high_root]
        );
        assert!(
            store
                .routes_matching_prefix("route", "0.0.0.0/0", nearest, &sources, 8)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn route_descendants_exclude_exact_sibling_and_ancestor_prefixes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST2"), String::from("TEST")];
        let high = object("TEST2", "route6", "2001:DB8:0:FFFF::/64AS64496");
        let first = object("TEST", "route6", "2001:DB8::/33AS64496");
        let last = object(
            "TEST",
            "route6",
            "2001:DB8:FFFF:FFFF:FFFF:FFFF:FFFF:FFFF/128AS4294967295",
        );
        store.replace_source("TEST2", [Ok(high.clone())])?;
        store.replace_source(
            "TEST",
            [
                Ok(object("TEST", "route6", "::/0AS64496")),
                Ok(object("TEST", "route6", "2001:DB8::/31AS64496")),
                Ok(object("TEST", "route6", "2001:DB8::/32AS64496")),
                Ok(object("TEST", "route6", "2001:DB9::/32AS64496")),
                Ok(object("TEST", "route", "0.0.0.0/0AS64496")),
                Ok(first.clone()),
                Ok(last.clone()),
            ],
        )?;
        let more = PrefixMatch::MoreSpecific;
        let expected = [high, first, last];
        assert_eq!(
            store.routes_matching_prefix("route6", "2001:DB8::/32", more, &sources, 8)?,
            expected
        );
        assert_eq!(
            store.routes_matching_prefix("route6", "2001:DB8::/32", more, &sources, 2)?,
            expected[..2]
        );
        assert_eq!(
            store.routes_matching_prefix("route6", "2001:DB8::/32", more, &sources[1..], 8)?,
            expected[1..]
        );
        assert_eq!(
            store
                .routes_matching_prefix("route6", "::/0", more, &sources, 20)?
                .len(),
            6
        );
        assert!(
            store
                .routes_matching_prefix("route6", "2001:DB8::/128", more, &sources, 8)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn prefix_index_backfills_and_tracks_committed_replacement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let sources = [String::from("TEST")];
        let old = object("TEST", "route", "192.0.2.0/24AS64496");
        let new = object("TEST", "route", "192.0.3.0/24AS64496");
        let store = Store::open(&path)?;
        store.replace_source("TEST", [Ok(old.clone())])?;
        let write = store.db.begin_write()?;
        assert!(write.delete_table(PREFIXES)?);
        write.commit()?;
        drop(store);
        let store = Store::open(&path)?;
        let query = || {
            store.routes_matching_prefix(
                "route",
                "192.0.2.0/23",
                PrefixMatch::MoreSpecific,
                &sources,
                8,
            )
        };
        assert_eq!(query()?, std::slice::from_ref(&old));
        assert!(
            store
                .replace_source(
                    "TEST",
                    [Ok(new.clone()), Err(anyhow::anyhow!("import failed"))]
                )
                .is_err()
        );
        assert_eq!(query()?, std::slice::from_ref(&old));
        let replacement = std::iter::once_with(|| {
            assert_eq!(query().unwrap(), [old]);
            Ok(new.clone())
        });
        store.replace_source("TEST", replacement)?;
        assert_eq!(query()?, [new]);
        assert_eq!(store.verify()?, 1);

        let write = store.db.begin_write()?;
        write.open_table(PREFIXES)?.remove(prefix_index_key(
            "TEST",
            "route",
            "192.0.3.0/24",
            "AS64496",
        )?)?;
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("missing from the route prefix index")
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
