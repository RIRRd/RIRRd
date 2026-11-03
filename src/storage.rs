//! Durable, indexed storage for locally imported source snapshots.

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::HashSet,
    io::Write,
    net::{Ipv4Addr, Ipv6Addr},
    ops::RangeInclusive,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use redb::{
    Database, Durability, MultimapTable, MultimapTableDefinition, MultimapTableHandle,
    ReadTransaction, ReadableMultimapTable, ReadableTable, ReadableTableMetadata, Table,
    TableDefinition, TableHandle, WriteTransaction,
};
use tokio::sync::RwLock;

use crate::{
    authoritative::{Change, authorize},
    nrtm3::{Kind, Operation},
    nrtm4_records::Record as Nrtm4Record,
    nrtm4_state::CommittedState,
    route_policy::RoutePolicy,
    rpsl::{Object, lookup_attributes},
};

type Identity<'a> = (&'a str, &'a str, &'a str);
type ExactKey = (&'static str, &'static str);
type PrefixKey<'a> = (&'a str, &'a str, u128, u8, u32);
type RangeCoverKey<'a> = (&'a str, u32, u8, u32, &'a str);
type InverseKey = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
type JournalKey = (&'static str, u64);
type JournalValue = (bool, &'static str, &'static str, &'static str);
type ObjectWriteTable<'txn> = Table<'txn, Identity<'static>, &'static str>;
type AuditValue = (u64, &'static str, &'static str, &'static str);

// Identity is (source, class, key). The exact index maps (source, key) to classes.
const OBJECTS: TableDefinition<Identity<'static>, &str> = TableDefinition::new("objects_v1");
const EXACT: MultimapTableDefinition<ExactKey, &str> = MultimapTableDefinition::new("exact_v1");
// The origin index maps (source, class, origin) to distinct canonical prefixes.
const ORIGINS: MultimapTableDefinition<Identity<'static>, &str> =
    MultimapTableDefinition::new("origins_v1");
// Per-source order by network address permits one range scan for all descendants.
const PREFIXES: TableDefinition<PrefixKey<'static>, ()> = TableDefinition::new("route_prefixes_v1");
// Keyed by source, block network, block length, allocation size minus one and object key.
const INETNUM_COVER: TableDefinition<RangeCoverKey<'static>, ()> =
    TableDefinition::new("inetnum_cover_v1");
const AS_BLOCK_COVER: TableDefinition<RangeCoverKey<'static>, ()> =
    TableDefinition::new("as_block_cover_v1");
// A row is (source, field, normalized value, class, object key).
const INVERSE: TableDefinition<InverseKey, ()> = TableDefinition::new("inverse_v1");
const CLASS_COUNTS: TableDefinition<ExactKey, u64> = TableDefinition::new("object_class_counts_v1");
const SERIALS: TableDefinition<&str, u64> = TableDefinition::new("source_serials_v1");
const MIRROR_FILTERS: TableDefinition<&str, &str> = TableDefinition::new("mirror_class_filters_v1");
const AUTH_NRTM3_SERIALS: TableDefinition<&str, u64> =
    TableDefinition::new("authoritative_nrtm3_serials_v1");
const NRTM4_STATE: TableDefinition<&str, &str> = TableDefinition::new("nrtm4_state_v1");
const MAX_NRTM4_STATE_BYTES: usize = 16 * 1024 * 1024;
// The journal value is (is_add, object class, object key, raw object text).
const JOURNAL: TableDefinition<JournalKey, JournalValue> = TableDefinition::new("nrtm3_journal_v1");
// Keyed by source and sequence. The value is (timestamp, class, key, action).
const AUTH_AUDIT: TableDefinition<JournalKey, AuditValue> =
    TableDefinition::new("authoritative_audit_v1");
const PROTECTED_NAMES: TableDefinition<Identity<'static>, ()> =
    TableDefinition::new("authoritative_protected_names_v1");
const NRTM3_RETAINED_OPERATIONS: u64 = 4096;

enum SourceReplacement {
    Mirror,
    Authoritative { serial_floor: Option<u64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixMatch {
    Exact,
    ClosestCovering,
    AllCovering,
    MoreSpecific,
}

#[derive(serde::Serialize)]
pub struct AuditEntry {
    pub sequence: u64,
    pub timestamp_unix_seconds: u64,
    pub source: String,
    pub class: String,
    pub key: String,
    pub action: String,
}

pub struct Store {
    db: Database,
    query_gate: RwLock<()>,
    legacy_md5_sources: HashSet<String>,
    route_policy: RoutePolicy,
    mirror_status:
        std::sync::Mutex<std::collections::BTreeMap<String, crate::mirror_worker::Status>>,
}

impl Store {
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_cache(
            path,
            crate::config::DEFAULT_DATABASE_CACHE_MIB * 1024 * 1024,
        )
    }

    pub fn open_with_cache(path: &Path, cache_bytes: usize) -> Result<Self> {
        let db = Database::builder()
            .set_cache_size(cache_bytes)
            .create(path)
            .with_context(|| format!("open {}", path.display()))?;
        Self::initialize(db)
    }

    /// Open an existing registry database and upgrade its indexes before serving queries.
    #[cfg(test)]
    pub fn open_for_service(path: &Path) -> Result<Self> {
        Self::open_for_service_with_cache(
            path,
            crate::config::DEFAULT_DATABASE_CACHE_MIB * 1024 * 1024,
        )
    }

    pub fn open_for_service_with_cache(path: &Path, cache_bytes: usize) -> Result<Self> {
        let db = Database::builder()
            .set_cache_size(cache_bytes)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
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
            if !tables.contains(CLASS_COUNTS.name()) {
                let mut counts = write.open_table(CLASS_COUNTS)?;
                for entry in records.iter()? {
                    let (identity, _) = entry?;
                    let (source, class, _) = identity.value();
                    let value = counts.get((source, class))?.map_or(0, |n| n.value());
                    counts.insert((source, class), value + 1)?;
                }
            }
            let backfill_prefixes = !tables.contains(PREFIXES.name());
            let backfill_inverse = !tables.contains(INVERSE.name());
            let backfill_inetnum = !tables.contains(INETNUM_COVER.name());
            let backfill_as_block = !tables.contains(AS_BLOCK_COVER.name());
            let mut origins = write.open_multimap_table(ORIGINS)?;
            let mut prefixes = write.open_table(PREFIXES)?;
            let mut inverse = write.open_table(INVERSE)?;
            let mut inetnum_cover = write.open_table(INETNUM_COVER)?;
            let mut as_block_cover = write.open_table(AS_BLOCK_COVER)?;
            write.open_table(SERIALS)?;
            write.open_table(MIRROR_FILTERS)?;
            write.open_table(AUTH_NRTM3_SERIALS)?;
            write.open_table(NRTM4_STATE)?;
            write.open_table(JOURNAL)?;
            let audit = write.open_table(AUTH_AUDIT)?;
            let mut protected = write.open_table(PROTECTED_NAMES)?;
            if !tables.contains(PROTECTED_NAMES.name()) {
                for entry in audit.iter()? {
                    let (identity, metadata) = entry?;
                    let (source, _) = identity.value();
                    let (_, class, key, action) = metadata.value();
                    if action == "delete"
                        && let Some(name) = protected_name((source, class, key))
                    {
                        protected.insert(name, ())?;
                    }
                }
            }
            if backfill_origins
                || backfill_prefixes
                || backfill_inverse
                || backfill_inetnum
                || backfill_as_block
            {
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
                    if backfill_inetnum && class == "inetnum" {
                        for entry in inetnum_cover_keys(source, key)? {
                            inetnum_cover.insert(entry, ())?;
                        }
                    }
                    if backfill_as_block && class == "as-block" {
                        for entry in as_block_cover_keys(source, key)? {
                            as_block_cover.insert(entry, ())?;
                        }
                    }
                }
            }
        }
        write.commit()?;
        Ok(Self::from_database(db))
    }

    /// Open without creating the file or initializing its application tables.
    #[cfg(test)]
    pub fn open_existing(path: &Path) -> Result<Self> {
        Self::open_existing_with_cache(
            path,
            crate::config::DEFAULT_DATABASE_CACHE_MIB * 1024 * 1024,
        )
    }

    pub fn open_existing_with_cache(path: &Path, cache_bytes: usize) -> Result<Self> {
        let db = Database::builder()
            .set_cache_size(cache_bytes)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        Ok(Self::from_database(db))
    }

    /// Wrap an already opened redb handle without initializing registry tables.
    pub(crate) fn from_database(db: Database) -> Self {
        Self {
            db,
            query_gate: RwLock::new(()),
            legacy_md5_sources: HashSet::new(),
            route_policy: RoutePolicy::default(),
            mirror_status: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// Hold a read guard across a service query that performs multiple reads.
    pub fn query_gate(&self) -> &RwLock<()> {
        &self.query_gate
    }

    pub fn set_mirror_status(&self, source: String, status: crate::mirror_worker::Status) {
        self.mirror_status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(source, status);
    }

    pub fn mirror_status(
        &self,
    ) -> std::collections::BTreeMap<String, crate::mirror_worker::Status> {
        self.mirror_status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Transactionally maintained counts; no object scan during status polling.
    pub fn class_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>> {
        let read = self.db.begin_read()?;
        let counts = read.open_table(CLASS_COUNTS)?;
        let mut result =
            std::collections::BTreeMap::<String, std::collections::BTreeMap<String, u64>>::new();
        for entry in counts.iter()? {
            let (key, count) = entry?;
            let (source, class) = key.value();
            result
                .entry(source.into())
                .or_default()
                .insert(class.into(), count.value());
        }
        Ok(result)
    }

    pub fn object_keys(
        &self,
        source: &str,
        class: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<String>, bool)> {
        ensure!(limit > 0 && limit <= 100, "object page limit must be 1–100");
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let upper = format!("{class}\0");
        let lower = after.map_or(std::ops::Bound::Included((source, class, "")), |key| {
            std::ops::Bound::Excluded((source, class, key))
        });
        let mut keys = Vec::new();
        for entry in records
            .range((
                lower,
                std::ops::Bound::Excluded((source, upper.as_str(), "")),
            ))?
            .take(limit + 1)
        {
            let (key, _) = entry?;
            keys.push(key.value().2.to_owned());
        }
        let more = keys.len() > limit;
        keys.truncate(limit);
        Ok((keys, more))
    }

    pub fn object_count(&self) -> Result<u64> {
        Ok(self.db.begin_read()?.open_table(OBJECTS)?.len()?)
    }

    /// Persist a verified snapshot cursor after a complete no-new-updates reply.
    /// This also pins the class policy for quiet sources across restarts.
    pub fn confirm_mirror_base(
        &self,
        source: &str,
        base: u64,
        classes: &HashSet<String>,
    ) -> Result<()> {
        let source = canonical_source(source)?;
        ensure!(
            !classes.is_empty()
                && classes
                    .iter()
                    .all(|class| crate::rpsl::supported_class(class)),
            "invalid mirror filter"
        );
        let mut policy = classes.iter().map(String::as_str).collect::<Vec<_>>();
        policy.sort_unstable();
        let policy = policy.join(",");
        let _gate = self.query_gate.blocking_write();
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        {
            reject_nrtm4_source(&write, &source)?;
            reject_authoritative_source(&write, &source)?;
            let mut serials = write.open_table(SERIALS)?;
            let mut filters = write.open_table(MIRROR_FILTERS)?;
            if let Some(recorded) = serials.get(source.as_str())? {
                ensure!(
                    recorded.value() == base
                        && filters
                            .get(source.as_str())?
                            .is_some_and(|value| value.value() == policy),
                    "mirror cursor or class filter changed; explicit snapshot reset required"
                );
                return Ok(());
            }
            let records = write.open_table(OBJECTS)?;
            let upper = format!("{source}\0");
            let mut count = 0;
            for entry in records.range((source.as_str(), "", "")..(upper.as_str(), "", ""))? {
                let (identity, _) = entry?;
                ensure!(
                    classes.contains(identity.value().1),
                    "snapshot contains an excluded mirror class"
                );
                count += 1;
            }
            ensure!(count > 0, "mirror bootstrap requires a nonempty snapshot");
            serials.insert(source.as_str(), base)?;
            filters.insert(source.as_str(), policy.as_str())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn with_legacy_md5_sources(mut self, sources: HashSet<String>) -> Self {
        self.legacy_md5_sources = sources;
        self
    }

    pub fn with_route_policy(mut self, policy: RoutePolicy) -> Self {
        self.route_policy = policy;
        self
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
        self.replace_source_mode(source, objects, SourceReplacement::Mirror)
    }

    /// Provision or rebase an authoritative publication, using an empty input
    /// to purge. A serial floor can recover from an older backup without
    /// reusing serials that clients may have seen.
    pub fn replace_authoritative_source(
        &self,
        source: &str,
        objects: impl IntoIterator<Item = Result<Object>>,
        serial_floor: Option<u64>,
    ) -> Result<u64> {
        self.replace_source_mode(
            source,
            objects,
            SourceReplacement::Authoritative { serial_floor },
        )
    }

    fn replace_source_mode(
        &self,
        source: &str,
        objects: impl IntoIterator<Item = Result<Object>>,
        mode: SourceReplacement,
    ) -> Result<u64> {
        let source = canonical_source(source)?;
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let mut count = 0;
        let mut skipped_lookup_values = 0;
        {
            reject_nrtm4_source(&write, &source)?;
            match mode {
                SourceReplacement::Mirror => {
                    reject_authoritative_source(&write, &source)?;
                    write.open_table(SERIALS)?.remove(source.as_str())?;
                    write.open_table(MIRROR_FILTERS)?.remove(source.as_str())?;
                }
                SourceReplacement::Authoritative { serial_floor } => {
                    ensure!(
                        write.open_table(SERIALS)?.get(source.as_str())?.is_none(),
                        "cannot rebase an authoritative source with a committed mirror serial"
                    );
                    let mut serials = write.open_table(AUTH_NRTM3_SERIALS)?;
                    let previous = serials.get(source.as_str())?.map(|serial| serial.value());
                    let serial =
                        previous
                            .into_iter()
                            .chain(serial_floor)
                            .max()
                            .map_or(Ok(0), |last| {
                                last.checked_add(1)
                                    .context("authoritative serial exhausted")
                            })?;
                    serials.insert(source.as_str(), serial)?;
                }
            }
            let mut records = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            write.open_table(JOURNAL)?.retain_in(
                (source.as_str(), 0)..=(source.as_str(), u64::MAX),
                |_, _| false,
            )?;

            clear_source_objects(&mut records, &mut indexes, &source)?;
            for object in objects {
                skipped_lookup_values +=
                    insert_snapshot_object(&mut records, &mut indexes, &source, &object?)?;
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
        self.apply_nrtm3_if(source, declared, initial_base, operations, || true)
    }

    /// Check cancellation after waiting for active queries and before opening the transaction.
    pub(crate) fn apply_nrtm3_if(
        &self,
        source: &str,
        declared: (u64, u64),
        initial_base: Option<u64>,
        operations: impl IntoIterator<Item = Result<Operation, String>>,
        should_start: impl FnOnce() -> bool,
    ) -> Result<u64> {
        self.apply_nrtm3_filtered_if(
            source,
            declared,
            initial_base,
            operations,
            None,
            should_start,
        )
    }

    /// A filtered mirror consumes a complete stream but must not republish an incomplete journal.
    pub(crate) fn apply_nrtm3_filtered_if(
        &self,
        source: &str,
        declared: (u64, u64),
        initial_base: Option<u64>,
        operations: impl IntoIterator<Item = Result<Operation, String>>,
        classes: Option<&HashSet<String>>,
        should_start: impl FnOnce() -> bool,
    ) -> Result<u64> {
        let source = canonical_source(source)?;
        let (start, end) = declared;
        ensure!(start > 0 && end >= start, "invalid NRTMv3 serial range");
        let _gate = self.query_gate.blocking_write();
        ensure!(should_start(), "NRTMv3 apply cancelled before write");
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let expire_through = end.saturating_sub(NRTM3_RETAINED_OPERATIONS);
        let mut skipped_lookup_values = 0;
        {
            reject_nrtm4_source(&write, &source)?;
            reject_authoritative_source(&write, &source)?;
            let mut serials = write.open_table(SERIALS)?;
            let mut journal = write.open_table(JOURNAL)?;
            let mut records = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            let recorded = serials.get(source.as_str())?.map(|serial| serial.value());
            let mut filters = write.open_table(MIRROR_FILTERS)?;
            let policy = classes.map(|classes| {
                let mut values = classes.iter().map(String::as_str).collect::<Vec<_>>();
                values.sort_unstable();
                values.join(",")
            });
            if recorded.is_some() {
                ensure!(
                    filters
                        .get(source.as_str())?
                        .map(|value| value.value().to_owned())
                        == policy,
                    "NRTMv3 class filter changed; explicit snapshot reset required"
                );
            } else if let Some(policy) = &policy {
                ensure!(!policy.is_empty(), "empty NRTMv3 class filter");
                filters.insert(source.as_str(), policy.as_str())?;
                // Refuse a bootstrap that already contains excluded classes.
                let upper = format!("{source}\0");
                for entry in records.range((source.as_str(), "", "")..(upper.as_str(), "", ""))? {
                    let (identity, _) = entry?;
                    ensure!(
                        classes.is_some_and(|classes| classes.contains(identity.value().1)),
                        "snapshot contains objects outside the mirror class filter"
                    );
                }
            }
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
                    last.checked_add(1)
                        .is_some_and(|next| if classes.is_some() {
                            serial >= next
                        } else {
                            serial == next
                        })
                        && serial <= end,
                    "non-contiguous NRTMv3 serial {serial} after {last}"
                );
                let identity = validate_identity(&source, &object)?;
                let retained = classes.is_none_or(|classes| classes.contains(&object.class));
                match (kind, retained) {
                    (Kind::Add, true) => {
                        let (_, skipped) =
                            upsert_object(&mut records, &mut indexes, identity, &object)?;
                        skipped_lookup_values += skipped;
                    }
                    (Kind::Delete, true) => {
                        delete_object(&mut records, &mut indexes, identity)?;
                    }
                    (_, false) => {}
                }
                if classes.is_none() && serial > expire_through {
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
                (classes.is_some() && last <= end) || last == end,
                "NRTMv3 stream ended at serial {last}, expected {end}"
            );
            journal.retain_in(
                (source.as_str(), 0)
                    ..=(
                        source.as_str(),
                        if classes.is_some() {
                            u64::MAX
                        } else {
                            expire_through
                        },
                    ),
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
        read_serial(&read, SERIALS, &source)
    }

    /// Return the authoritative publication serial, separately from the
    /// upstream serial used to resume mirrored sources.
    pub fn source_local_serial(&self, source: &str) -> Result<Option<u64>> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        read.open_table(OBJECTS)
            .context("missing or invalid registry objects table")?;
        read_serial(&read, AUTH_NRTM3_SERIALS, &source)
    }

    /// Read the committed NRTMv4 trust and session state for a source.
    pub fn nrtm4_state(&self, source: &str) -> Result<Option<CommittedState>> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        read.open_table(OBJECTS)
            .context("missing or invalid registry objects table")?;
        let state = match read.open_table(NRTM4_STATE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        state
            .get(source.as_str())?
            .map(|value| decode_nrtm4_state(&source, value.value()))
            .transpose()
    }

    /// Apply a verified snapshot and/or ordered deltas with the associated trust transition.
    /// The caller verifies every selected file, and the iterator must report any trailing error.
    pub fn apply_nrtm4(
        &self,
        source: &str,
        expected: Option<&CommittedState>,
        next: &CommittedState,
        replace: bool,
        records: impl IntoIterator<Item = Result<Nrtm4Record>>,
    ) -> Result<u64> {
        let source = canonical_source(source)?;
        ensure!(next.source == source, "NRTMv4 next state source mismatch");
        next.validate().context("validating NRTMv4 next state")?;
        let encoded = serde_json::to_string(next).context("encoding NRTMv4 next state")?;
        ensure!(
            encoded.len() <= MAX_NRTM4_STATE_BYTES,
            "NRTMv4 state exceeds byte limit"
        );

        let _gate = self.query_gate.blocking_write();
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let mut count = 0;
        let mut skipped_lookup_values = 0;
        {
            let mut state = write.open_table(NRTM4_STATE)?;
            let current = state
                .get(source.as_str())?
                .map(|value| decode_nrtm4_state(&source, value.value()))
                .transpose()?;
            ensure!(
                current.as_ref() == expected,
                "NRTMv4 state changed after planning"
            );
            let serials = write.open_table(SERIALS)?;
            ensure!(
                serials.get(source.as_str())?.is_none(),
                "cannot apply NRTMv4 to a source with a committed NRTM serial"
            );
            reject_authoritative_source(&write, &source)?;

            let mut metadata_only = false;
            if let Some(current) = current {
                if current.session_id == next.session_id {
                    ensure!(next.version >= current.version, "NRTMv4 version regression");
                    metadata_only = next.version == current.version;
                    ensure!(
                        !metadata_only || !replace,
                        "cannot replace an unchanged NRTMv4 version"
                    );
                } else {
                    ensure!(replace, "new NRTMv4 session requires a snapshot");
                }
            } else {
                ensure!(replace, "initial NRTMv4 apply requires a snapshot");
            }

            let mut objects = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            if replace {
                clear_source_objects(&mut objects, &mut indexes, &source)?;
            }
            let mut snapshot_phase = replace;
            for record in records {
                let record = record?;
                ensure!(
                    !metadata_only,
                    "unchanged NRTMv4 version cannot apply records"
                );
                match record {
                    Nrtm4Record::Snapshot(object) => {
                        ensure!(
                            snapshot_phase,
                            "NRTMv4 snapshot record follows a delta or appears in a delta apply"
                        );
                        skipped_lookup_values +=
                            insert_snapshot_object(&mut objects, &mut indexes, &source, &object)?;
                    }
                    Nrtm4Record::AddModify(object) => {
                        snapshot_phase = false;
                        let identity = validate_identity(&source, &object)?;
                        let (_, skipped) =
                            upsert_object(&mut objects, &mut indexes, identity, &object)?;
                        skipped_lookup_values += skipped;
                    }
                    Nrtm4Record::Delete { class, key } => {
                        snapshot_phase = false;
                        ensure!(
                            !class.is_empty() && !key.is_empty(),
                            "empty NRTMv4 delete identity"
                        );
                        delete_object(&mut objects, &mut indexes, (&source, &class, &key))?;
                    }
                }
                count += 1;
            }
            ensure!(
                replace || metadata_only || count > 0,
                "NRTMv4 version advance has no delta records"
            );
            state.insert(source.as_str(), encoded.as_str())?;
        }
        write.commit()?;
        warn_skipped_lookup_values(&source, skipped_lookup_values);
        Ok(count)
    }

    /// Authorize and commit a submission against the same registry write transaction.
    pub fn apply_authoritative(
        &self,
        source: &str,
        changes: Vec<Change>,
        passwords: &[String],
    ) -> Result<u64> {
        Ok(self
            .apply_authoritative_report(source, changes, passwords)?
            .len() as u64)
    }

    /// Commit a submission and return API action names in input order.
    /// Call from synchronous code or a blocking worker.
    pub fn apply_authoritative_report(
        &self,
        source: &str,
        mut changes: Vec<Change>,
        passwords: &[String],
    ) -> Result<Vec<&'static str>> {
        let source = canonical_source(source)?;
        let _gate = self.query_gate.blocking_write();
        let mut write = self.db.begin_write()?;
        write.set_durability(Durability::Immediate);
        let mut actions = Vec::new();
        {
            reject_nrtm4_source(&write, &source)?;
            let serials = write.open_table(SERIALS)?;
            ensure!(
                serials.get(source.as_str())?.is_none(),
                "cannot write to a source with a committed NRTM serial"
            );
            let mut records = write.open_table(OBJECTS)?;
            let mut indexes = WriteIndexes::open(&write)?;
            authorize(
                &source,
                &changes,
                passwords,
                |class, key| read_object(&records, &source, class, key),
                |class, key| indexes.has_strong_reference(&source, class, key),
                |object| indexes.allocation_parent(&records, object),
                self.legacy_md5_sources.contains(&source),
            )?;
            for change in &changes {
                if let Change::Upsert(object) = change
                    && let Some((prefix, origin)) = route_index_parts(&object.class, &object.key)?
                {
                    let origin = origin
                        .strip_prefix("AS")
                        .context("invalid route origin")?
                        .parse()?;
                    self.route_policy.validate(&source, prefix, origin)?;
                }
            }

            // Authentication and all admission checks precede expensive replacement
            // hashing. No placeholder credential can enter storage or a journal.
            for change in &mut changes {
                if let Change::Upsert(object) = change
                    && crate::maintainer_auth::dummy_update(object)?
                {
                    let password = passwords.first().context("missing replacement password")?;
                    *object = crate::maintainer_auth::materialize_dummy_update(object, password)?;
                }
            }

            let mut audit = write.open_table(AUTH_AUDIT)?;
            let mut journal = write.open_table(JOURNAL)?;
            let mut local_serials = write.open_table(AUTH_NRTM3_SERIALS)?;
            let mut local_serial = local_serials
                .get(source.as_str())?
                .map_or(0, |serial| serial.value());
            let mut protected = write.open_table(PROTECTED_NAMES)?;
            let pending_deletions: HashSet<_> = changes
                .iter()
                .filter_map(|change| match change {
                    Change::Delete(object) => {
                        protected_name((&object.source, &object.class, &object.key))
                    }
                    Change::Upsert(_) => None,
                })
                .collect();
            let mut sequence = audit
                .range((source.as_str(), 0)..=(source.as_str(), u64::MAX))?
                .next_back()
                .transpose()?
                .map_or(0, |(key, _)| key.value().1);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system time before Unix epoch")?
                .as_secs();
            for change in &changes {
                local_serial = local_serial
                    .checked_add(1)
                    .context("authoritative serial exhausted")?;
                let (identity, audit_action, report_action, journal_text) = match change {
                    Change::Upsert(object) => {
                        let identity = validate_identity(&source, object)?;
                        if let Some(name) = protected_name(identity)
                            && (pending_deletions.contains(&name) || protected.get(name)?.is_some())
                        {
                            ensure!(
                                records.get(identity)?.is_some(),
                                "authoritative object name is protected after deletion"
                            );
                        }
                        let (updated, skipped) =
                            upsert_object(&mut records, &mut indexes, identity, object)?;
                        ensure!(
                            skipped == 0,
                            "invalid lookup attributes in authoritative object"
                        );
                        let (audit_action, report_action) = if updated {
                            ("update", "modify")
                        } else {
                            ("create", "create")
                        };
                        (identity, audit_action, report_action, object.public_text())
                    }
                    Change::Delete(object) => {
                        let identity = validate_identity(&source, object)?;
                        let previous = read_object(&records, identity.0, identity.1, identity.2)?
                            .context("object to delete does not exist")?;
                        let journal_text = Cow::Owned(previous.public_text().into_owned());
                        ensure!(
                            delete_object(&mut records, &mut indexes, identity)?,
                            "object to delete does not exist"
                        );
                        if let Some(name) = protected_name(identity) {
                            protected.insert(name, ())?;
                        }
                        (identity, "delete", "delete", journal_text)
                    }
                };
                let (source, class, key) = identity;
                sequence = sequence
                    .checked_add(1)
                    .context("authoritative audit sequence exhausted")?;
                audit.insert((source, sequence), (timestamp, class, key, audit_action))?;
                ensure!(
                    journal
                        .insert(
                            (source, local_serial),
                            (audit_action != "delete", class, key, journal_text.as_ref()),
                        )?
                        .is_none(),
                    "duplicate authoritative NRTMv3 serial {local_serial}"
                );
                actions.push(report_action);
            }
            journal.retain_in(
                (source.as_str(), 0)
                    ..=(
                        source.as_str(),
                        local_serial.saturating_sub(NRTM3_RETAINED_OPERATIONS),
                    ),
                |_, _| false,
            )?;
            local_serials.insert(source.as_str(), local_serial)?;
        }
        write.commit()?;
        Ok(actions)
    }

    /// Read recent successful authoritative operations, most recent first.
    pub fn source_history(&self, source: &str, limit: usize) -> Result<Vec<AuditEntry>> {
        ensure!(limit <= 1000, "authoritative history limit exceeds 1000");
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        read.open_table(OBJECTS)
            .context("missing or invalid registry objects table")?;
        let audit = match read.open_table(AUTH_AUDIT) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut entries = Vec::new();
        for entry in audit
            .range((source.as_str(), 0)..=(source.as_str(), u64::MAX))?
            .rev()
            .take(limit)
        {
            let (audit_key, metadata) = entry?;
            let (source, sequence) = audit_key.value();
            let (timestamp_unix_seconds, class, key, action) = metadata.value();
            entries.push(AuditEntry {
                sequence,
                timestamp_unix_seconds,
                source: source.to_owned(),
                class: class.to_owned(),
                key: key.to_owned(),
                action: action.to_owned(),
            });
        }
        Ok(entries)
    }

    /// Return the oldest and newest retained NRTMv3 operations for one source.
    pub fn nrtm3_journal_bounds(&self, source: &str) -> Result<Option<(u64, u64)>> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        let journal = match read.open_table(JOURNAL) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut range = journal.range((source.as_str(), 0)..=(source.as_str(), u64::MAX))?;
        let Some((first, _)) = range.next().transpose()? else {
            return Ok(None);
        };
        let first = first.value().1;
        let last = range
            .next_back()
            .transpose()?
            .map_or(first, |(key, _)| key.value().1);
        ensure!(
            publication_serial(&read, &source)? == Some(last),
            "NRTMv3 journal does not end at the publication serial"
        );
        Ok(Some((first, last)))
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
        let committed =
            publication_serial(&read, &source)?.context("NRTMv3 journal unavailable for source")?;
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
    pub fn export_source(&self, source: &str, output: impl Write) -> Result<u64> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        write_source_records(&read, &source, output, false)
    }

    /// Write public RPSL and return the publication serial observed by the
    /// same redb read snapshot.
    pub fn export_public_source(
        &self,
        source: &str,
        output: impl Write,
    ) -> Result<(u64, Option<u64>)> {
        let source = canonical_source(source)?;
        let read = self.db.begin_read()?;
        let serial = publication_serial(&read, &source)?;
        let count = write_source_records(&read, &source, output, true)?;
        Ok((count, serial))
    }

    /// Check index coverage and retained journal metadata.
    pub fn verify(&self) -> Result<u64> {
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let exact = read.open_multimap_table(EXACT)?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let prefixes = read.open_table(PREFIXES)?;
        let inverse = read.open_table(INVERSE)?;
        let inetnum_cover = match read.open_table(INETNUM_COVER) {
            Ok(table) => Some(table),
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(error) => return Err(error.into()),
        };
        let as_block_cover = match read.open_table(AS_BLOCK_COVER) {
            Ok(table) => Some(table),
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(error) => return Err(error.into()),
        };
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
        let mut expected_counts = std::collections::BTreeMap::<(String, String), u64>::new();
        let mut indexed_attributes = 0;
        let mut indexed_inetnum_blocks = 0;
        let mut indexed_as_blocks = 0;
        for entry in records.iter()? {
            let (identity, text) = entry?;
            let (source, class, key) = identity.value();
            *expected_counts
                .entry((source.into(), class.into()))
                .or_default() += 1;
            if class == "inetnum"
                && let Some(inetnum_cover) = &inetnum_cover
            {
                for entry in inetnum_cover_keys(source, key)? {
                    ensure!(
                        inetnum_cover.get(entry)?.is_some(),
                        "object is missing from the inetnum cover index"
                    );
                    indexed_inetnum_blocks += 1;
                }
            }
            if class == "as-block"
                && let Some(as_block_cover) = &as_block_cover
            {
                for entry in as_block_cover_keys(source, key)? {
                    ensure!(
                        as_block_cover.get(entry)?.is_some(),
                        "object is missing from the AS-block cover index"
                    );
                    indexed_as_blocks += 1;
                }
            }
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
        if let Some(inetnum_cover) = inetnum_cover {
            ensure!(
                inetnum_cover.len()? == indexed_inetnum_blocks,
                "unexpected inetnum cover index entries"
            );
        }
        if let Some(as_block_cover) = as_block_cover {
            ensure!(
                as_block_cover.len()? == indexed_as_blocks,
                "unexpected AS-block cover index entries"
            );
        }
        match read.open_table(CLASS_COUNTS) {
            Ok(counts) => {
                let mut actual = std::collections::BTreeMap::new();
                for entry in counts.iter()? {
                    let (key, n) = entry?;
                    let (source, class) = key.value();
                    actual.insert((source.to_owned(), class.to_owned()), n.value());
                }
                ensure!(
                    actual == expected_counts,
                    "object class counts do not match stored objects"
                );
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(error) => return Err(error.into()),
        }
        verify_journal(&read)?;
        verify_mirror_filters(&read)?;
        verify_nrtm4_state(&read)?;
        Ok(indexed)
    }
}

fn verify_mirror_filters(read: &ReadTransaction) -> Result<()> {
    let filters = match read.open_table(MIRROR_FILTERS) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let records = read.open_table(OBJECTS)?;
    let journal = read.open_table(JOURNAL)?;
    for entry in filters.iter()? {
        let (source, policy) = entry?;
        let source = source.value();
        ensure!(
            canonical_source(source)? == source && read_serial(read, SERIALS, source)?.is_some(),
            "filtered mirror has no corresponding source serial"
        );
        let classes = policy.value().split(',').collect::<Vec<_>>();
        ensure!(
            !classes.is_empty()
                && classes
                    .iter()
                    .all(|class| crate::rpsl::supported_class(class))
                && classes.windows(2).all(|pair| pair[0] < pair[1]),
            "invalid persisted mirror class filter"
        );
        ensure!(
            journal
                .range((source, 0)..=(source, u64::MAX))?
                .next()
                .is_none(),
            "filtered mirror must not expose a partial journal"
        );
        let upper = format!("{source}\0");
        for entry in records.range((source, "", "")..(upper.as_str(), "", ""))? {
            let (identity, _) = entry?;
            ensure!(
                classes.contains(&identity.value().1),
                "filtered mirror contains an excluded class"
            );
        }
    }
    Ok(())
}

fn write_source_records(
    read: &ReadTransaction,
    source: &str,
    mut output: impl Write,
    public: bool,
) -> Result<u64> {
    let next_source = format!("{source}\0");
    let records = read.open_table(OBJECTS)?;
    let mut count = 0;
    for entry in records.range((source, "", "")..(next_source.as_str(), "", ""))? {
        let (identity, text) = entry?;
        let raw = text.value();
        let object;
        let text = if public {
            let (_, class, key) = identity.value();
            object = Object {
                source: source.to_owned(),
                class: class.to_owned(),
                key: key.to_owned(),
                text: raw.to_owned(),
            };
            object.public_text()
        } else {
            Cow::Borrowed(raw)
        };
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

fn read_serial(
    read: &ReadTransaction,
    definition: TableDefinition<&str, u64>,
    source: &str,
) -> Result<Option<u64>> {
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(table.get(source)?.map(|serial| serial.value()))
}

fn publication_serial(read: &ReadTransaction, source: &str) -> Result<Option<u64>> {
    let mirror = read_serial(read, SERIALS, source)?;
    let local = read_serial(read, AUTH_NRTM3_SERIALS, source)?;
    ensure!(
        mirror.is_none() || local.is_none(),
        "source has both local and mirror NRTMv3 serials"
    );
    Ok(local.or(mirror))
}

fn verify_nrtm4_state(read: &ReadTransaction) -> Result<()> {
    let states = match read.open_table(NRTM4_STATE) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let audit = read.open_table(AUTH_AUDIT)?;
    for entry in states.iter()? {
        let (source, state) = entry?;
        let source = source.value();
        decode_nrtm4_state(source, state.value())?;
        ensure!(
            publication_serial(read, source)?.is_none(),
            "NRTMv4 source also has a committed NRTM serial"
        );
        ensure!(
            audit
                .range((source, 0)..=(source, u64::MAX))?
                .next()
                .transpose()?
                .is_none(),
            "NRTMv4 source also has authoritative history"
        );
    }
    Ok(())
}

fn decode_nrtm4_state(source: &str, encoded: &str) -> Result<CommittedState> {
    ensure!(
        encoded.len() <= MAX_NRTM4_STATE_BYTES,
        "NRTMv4 state exceeds byte limit"
    );
    let state: CommittedState =
        serde_json::from_str(encoded).context("decoding committed NRTMv4 state")?;
    ensure!(
        state.source == source,
        "committed NRTMv4 state source mismatch"
    );
    state
        .validate()
        .context("validating committed NRTMv4 state")?;
    Ok(state)
}

fn reject_nrtm4_source(write: &WriteTransaction, source: &str) -> Result<()> {
    let states = write.open_table(NRTM4_STATE)?;
    ensure!(
        states.get(source)?.is_none(),
        "source has committed NRTMv4 state; explicit reset is required"
    );
    Ok(())
}

fn reject_authoritative_source(write: &WriteTransaction, source: &str) -> Result<()> {
    let local = write.open_table(AUTH_NRTM3_SERIALS)?;
    let audit = write.open_table(AUTH_AUDIT)?;
    ensure!(
        local.get(source)?.is_none()
            && audit
                .range((source, 0)..=(source, u64::MAX))?
                .next()
                .transpose()?
                .is_none(),
        "source has authoritative history or local publication state"
    );
    Ok(())
}

fn verify_journal(read: &ReadTransaction) -> Result<()> {
    let local = match read.open_table(AUTH_NRTM3_SERIALS) {
        Ok(table) => Some(table),
        Err(redb::TableError::TableDoesNotExist(_)) => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(local) = &local {
        for entry in local.iter()? {
            let (source, _) = entry?;
            let source = source.value();
            ensure!(
                canonical_source(source)? == source,
                "invalid authoritative serial source"
            );
            ensure!(
                read_serial(read, SERIALS, source)?.is_none(),
                "source has both local and mirror NRTMv3 serials"
            );
        }
    }
    let journal = match read.open_table(JOURNAL) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => {
            if let Some(local) = &local {
                ensure!(
                    local.is_empty()?,
                    "authoritative NRTMv3 journal table is missing"
                );
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
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
        let committed = publication_serial(read, source)?
            .context("NRTMv3 journal has no corresponding source serial")?;
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
    inetnum_cover: Table<'txn, RangeCoverKey<'static>, ()>,
    as_block_cover: Table<'txn, RangeCoverKey<'static>, ()>,
    class_counts: Table<'txn, ExactKey, u64>,
}

impl<'txn> WriteIndexes<'txn> {
    fn open(write: &'txn WriteTransaction) -> Result<Self> {
        Ok(Self {
            exact: write.open_multimap_table(EXACT)?,
            origins: write.open_multimap_table(ORIGINS)?,
            prefixes: write.open_table(PREFIXES)?,
            inverse: write.open_table(INVERSE)?,
            inetnum_cover: write.open_table(INETNUM_COVER)?,
            as_block_cover: write.open_table(AS_BLOCK_COVER)?,
            class_counts: write.open_table(CLASS_COUNTS)?,
        })
    }

    fn has_strong_reference(&self, source: &str, class: &str, key: &str) -> Result<bool> {
        let fields: &[&str] = match class {
            "mntner" => &["mnt-by"],
            "person" | "role" => &["admin-c", "tech-c", "zone-c"],
            _ => &[],
        };
        let end = format!("{key}\0");
        for &field in fields {
            let lower = (source, field, key, "", "");
            let upper = (source, field, end.as_str(), "", "");
            for entry in self.inverse.range(lower..upper)?.take(2) {
                let (identity, _) = entry?;
                let (_, _, _, referring_class, referring_key) = identity.value();
                if (referring_class, referring_key) != (class, key) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn allocation_parent(
        &self,
        records: &ObjectWriteTable<'_>,
        object: &Object,
    ) -> Result<Option<Object>> {
        if object.class != "aut-num" {
            return self.route_parent(records, object);
        }
        let source = object.source.as_str();
        let asn = stored_asn(&object.key)?;
        let Some(key) = narrowest_cover_key(&self.as_block_cover, source, asn, 32, "AS-block")?
        else {
            return Ok(None);
        };
        let parent = read_object(records, source, "as-block", &key)?
            .context("AS-block cover index points to a missing allocation")?;
        Ok(Some(parent))
    }

    fn route_parent(
        &self,
        records: &ObjectWriteTable<'_>,
        route: &Object,
    ) -> Result<Option<Object>> {
        let source = route.source.as_str();
        let class = route.class.as_str();
        let (prefix, _) = route_index_parts(class, &route.key)?
            .context("parent lookup requires a route object")?;
        let prefix = RoutePrefix::parse(class, prefix)?;

        if class == "route" {
            if let Some(key) = narrowest_cover_key(
                &self.inetnum_cover,
                source,
                u32::try_from(prefix.network)?,
                prefix.length,
                "inetnum",
            )? {
                let parent = read_object(records, source, "inetnum", &key)?
                    .context("inetnum cover index points to a missing allocation")?;
                return Ok(Some(parent));
            }
        } else {
            for length in (0..=prefix.length).rev() {
                let network = Ipv6Addr::from(prefix.network_at(length));
                let key = format!("{network}/{length}").to_ascii_uppercase();
                if let Some(parent) = read_object(records, source, "inet6num", &key)? {
                    return Ok(Some(parent));
                }
            }
        }

        for length in (0..prefix.length).rev() {
            let network = prefix.network_at(length);
            let first = (source, class, network, length, 0);
            let last = (source, class, network, length, u32::MAX);
            let mut candidates = self.prefixes.range(first..=last)?;
            if let Some((entry, _)) = candidates.next().transpose()? {
                ensure!(
                    candidates.next().transpose()?.is_none(),
                    "ambiguous covering route parent"
                );
                let (_, _, network, length, origin) = entry.value();
                let key = route_key(class, network, length, origin)?;
                let parent = read_object(records, source, class, &key)?
                    .context("route prefix index points to a missing parent")?;
                return Ok(Some(parent));
            }
        }
        Ok(None)
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
        if class == "inetnum" {
            for entry in inetnum_cover_keys(source, key)? {
                ensure!(
                    self.inetnum_cover.insert(entry, ())?.is_none(),
                    "duplicate inetnum cover index entry"
                );
            }
        }
        if class == "as-block" {
            for entry in as_block_cover_keys(source, key)? {
                ensure!(
                    self.as_block_cover.insert(entry, ())?.is_none(),
                    "duplicate AS-block cover index entry"
                );
            }
        }
        let attributes = object.lookup_attributes().map_err(anyhow::Error::msg)?;
        for (field, value) in attributes.inverse_entries() {
            self.inverse
                .insert((source, field, value.as_ref(), class, key), ())?;
        }
        let count = self
            .class_counts
            .get((source, class))?
            .map_or(0, |value| value.value());
        self.class_counts.insert(
            (source, class),
            count.checked_add(1).context("object count overflow")?,
        )?;
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
        if class == "inetnum" {
            for entry in inetnum_cover_keys(source, key)? {
                ensure!(
                    self.inetnum_cover.remove(entry)?.is_some(),
                    "object is missing from the inetnum cover index"
                );
            }
        }
        if class == "as-block" {
            for entry in as_block_cover_keys(source, key)? {
                ensure!(
                    self.as_block_cover.remove(entry)?.is_some(),
                    "object is missing from the AS-block cover index"
                );
            }
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
        let count = self
            .class_counts
            .get((source, class))?
            .context("missing object class count")?
            .value();
        ensure!(count > 0, "invalid zero object class count");
        if count == 1 {
            self.class_counts.remove((source, class))?;
        } else {
            self.class_counts.insert((source, class), count - 1)?;
        }
        Ok(())
    }
}

fn narrowest_cover_key(
    table: &Table<'_, RangeCoverKey<'static>, ()>,
    source: &str,
    address: u32,
    query_length: u8,
    allocation_class: &str,
) -> Result<Option<String>> {
    let mut best = None::<(u32, String)>;
    let mut tied = false;
    for length in (0..=query_length).rev() {
        let mask = u32::MAX.checked_shl(u32::from(32 - length)).unwrap_or(0);
        let network = address & mask;
        let first = (source, network, length, 0, "");
        let last = (source, network, length + 1, 0, "");
        // A query contained by an allocation fits in one of its maximal
        // aligned blocks. Each block's candidates sort by allocation size.
        for entry in table.range(first..last)?.take(2) {
            let (key, _) = entry?;
            let (_, _, _, size, object_key) = key.value();
            match best.as_ref().map(|(best_size, _)| size.cmp(best_size)) {
                None | Some(Ordering::Less) => {
                    best = Some((size, object_key.to_owned()));
                    tied = false;
                }
                Some(Ordering::Equal) => tied = true,
                Some(Ordering::Greater) => break,
            }
        }
    }
    ensure!(!tied, "ambiguous covering {allocation_class} parent");
    Ok(best.map(|(_, key)| key))
}

fn inetnum_cover_keys<'a>(
    source: &'a str,
    key: &'a str,
) -> Result<impl Iterator<Item = RangeCoverKey<'a>>> {
    let address_number = |value: &str| -> Result<u32> {
        let ip = value.split_once('/').map_or(value, |(ip, _)| ip);
        Ok(u32::from(
            ip.parse::<Ipv4Addr>()
                .context("invalid stored inetnum address")?,
        ))
    };
    let (first, last) = if let Some((first, last)) = key.split_once(" - ") {
        (address_number(first)?, address_number(last)?)
    } else if key.contains('/') {
        let prefix = RoutePrefix::parse("route", key)?;
        (
            u32::try_from(prefix.network)?,
            u32::try_from(prefix.network | prefix.host_mask(prefix.length))?,
        )
    } else {
        let address = address_number(key)?;
        (address, address)
    };
    range_cover_keys(source, key, first, last).context("invalid stored inetnum range")
}

fn stored_asn(value: &str) -> Result<u32> {
    let digits = value.strip_prefix("AS").context("invalid stored ASN key")?;
    ensure!(
        digits.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid stored ASN key"
    );
    digits.parse().context("invalid stored ASN key")
}

fn as_block_cover_keys<'a>(
    source: &'a str,
    key: &'a str,
) -> Result<impl Iterator<Item = RangeCoverKey<'a>>> {
    let (first, last) = key
        .split_once(" - ")
        .context("invalid stored AS-block key")?;
    range_cover_keys(source, key, stored_asn(first)?, stored_asn(last)?)
        .context("invalid stored AS-block range")
}

fn range_cover_keys<'a>(
    source: &'a str,
    key: &'a str,
    first: u32,
    last: u32,
) -> Result<impl Iterator<Item = RangeCoverKey<'a>>> {
    let size = last
        .checked_sub(first)
        .context("reversed allocation range")?;
    let mut next = u64::from(first);
    let end = u64::from(last);
    Ok(std::iter::from_fn(move || {
        if next > end {
            return None;
        }
        let network = next as u32;
        let host_bits = network.trailing_zeros().min((end - next + 1).ilog2());
        next += 1_u64 << host_bits;
        Some((source, network, (32 - host_bits) as u8, size, key))
    }))
}

fn upsert_object(
    records: &mut ObjectWriteTable<'_>,
    indexes: &mut WriteIndexes<'_>,
    identity: Identity<'_>,
    object: &Object,
) -> Result<(bool, usize)> {
    let previous = records.insert(identity, object.text.as_str())?;
    if let Some(text) = &previous {
        indexes.remove(identity, text.value())?;
    }
    Ok((previous.is_some(), indexes.insert(object)?))
}

fn clear_source_objects(
    records: &mut ObjectWriteTable<'_>,
    indexes: &mut WriteIndexes<'_>,
    source: &str,
) -> Result<()> {
    // Appending the smallest character gives the exclusive end of this source's range.
    let next_source = format!("{source}\0");
    let old_records = records
        .extract_from_if((source, "", "")..(next_source.as_str(), "", ""), |_, _| {
            true
        })?;
    for entry in old_records {
        let (identity, text) = entry?;
        indexes.remove(identity.value(), text.value())?;
    }
    Ok(())
}

fn insert_snapshot_object(
    records: &mut ObjectWriteTable<'_>,
    indexes: &mut WriteIndexes<'_>,
    source: &str,
    object: &Object,
) -> Result<usize> {
    let identity = validate_identity(source, object)?;
    let (_, class, key) = identity;
    ensure!(
        records.insert(identity, object.text.as_str())?.is_none(),
        "duplicate {class} object {key} in {source}"
    );
    indexes.insert(object)
}

fn delete_object(
    records: &mut ObjectWriteTable<'_>,
    indexes: &mut WriteIndexes<'_>,
    identity: Identity<'_>,
) -> Result<bool> {
    let Some(previous) = records.remove(identity)? else {
        return Ok(false);
    };
    indexes.remove(identity, previous.value())?;
    Ok(true)
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

fn protected_name((source, class, key): Identity<'_>) -> Option<Identity<'_>> {
    let category = match class {
        "person" | "role" => "contact",
        "mntner" => "mntner",
        _ => return None,
    };
    Some((source, category, key))
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
    records: &impl ReadableTable<Identity<'static>, &'static str>,
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

    fn authoritative_contact(name: &str, key: &str) -> Result<Object> {
        crate::rpsl::parse(&format!(
            "person: {name}\naddress: Example Office\nphone: +1 555 0100\ne-mail: operator@example.net\nnic-hdl: {key}\nmnt-by: TEST-MNT\nsource: TEST\n"
        )).map_err(anyhow::Error::msg)
    }

    fn seed_authoritative_maintainer(store: &Store, password: &str) -> Result<()> {
        let hash = bcrypt::hash(password, 4)?;
        let maintainer = crate::rpsl::parse(&format!(
            "mntner: TEST-MNT\nauth: BCRYPT-PW {hash}\nmnt-by: TEST-MNT\nsource: TEST\n"
        ))
        .map_err(anyhow::Error::msg)?;
        if store.source_local_serial("TEST")?.is_some() {
            store.replace_authoritative_source("TEST", [Ok(maintainer)], None)?;
        } else {
            store.replace_source("TEST", [Ok(maintainer)])?;
        }
        Ok(())
    }

    #[test]
    fn class_counts_and_key_pages_follow_commits_rollbacks_and_legacy_upgrade() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("counts.redb");
        let store = Store::open(&path)?;
        store.replace_source(
            "TEST",
            [
                Ok(object("TEST", "as-set", "AS-A")),
                Ok(object("TEST", "as-set", "AS-B")),
                Ok(object("TEST", "route", "192.0.2.0/24AS64496")),
            ],
        )?;
        assert_eq!(store.class_counts()?["TEST"]["as-set"], 2);
        assert_eq!(
            store.object_keys("TEST", "as-set", None, 1)?,
            (vec![String::from("AS-A")], true)
        );
        assert_eq!(
            store.object_keys("TEST", "as-set", Some("AS-A"), 1)?,
            (vec![String::from("AS-B")], false)
        );
        let bad = [
            Ok(object("TEST", "as-set", "AS-C")),
            Err(anyhow::anyhow!("late fixture failure")),
        ];
        assert!(store.replace_source("TEST", bad).is_err());
        assert_eq!(store.class_counts()?["TEST"]["as-set"], 2);
        assert_eq!(store.verify()?, 3);
        let write = store.db.begin_write()?;
        write.delete_table(CLASS_COUNTS)?;
        write.commit()?;
        drop(store);
        let store = Store::open_for_service(&path)?;
        assert_eq!(store.class_counts()?["TEST"]["route"], 1);
        assert_eq!(store.verify()?, 3);
        store.replace_source("TEST", std::iter::empty())?;
        assert!(store.class_counts()?.is_empty());
        assert_eq!(store.verify()?, 0);
        Ok(())
    }

    #[test]
    fn authoritative_writes_require_auth_and_preserve_identity_audit_on_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let passwords = [String::from("fixture password")];
        let sources = [String::from("TEST")];
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let original = authoritative_contact("Example Operator", "EO1-TEST")?;
        let updated = authoritative_contact("Second Operator", "EO1-TEST")?;

        assert!(
            store
                .apply_authoritative("TEST", vec![Change::Upsert(original.clone())], &[])
                .is_err()
        );
        assert!(store.exact("EO1-TEST", &sources, 1)?.is_empty());
        assert_eq!(
            store.apply_authoritative("test", vec![Change::Upsert(original)], &passwords)?,
            1
        );
        assert_eq!(
            store.apply_authoritative("TEST", vec![Change::Upsert(updated.clone())], &passwords)?,
            1
        );
        assert_eq!(
            store.exact("EO1-TEST", &sources, 1)?,
            std::slice::from_ref(&updated)
        );
        assert!(
            store
                .inverse("person", "Example Operator", &sources, 1)?
                .is_empty()
        );
        assert_eq!(
            store.inverse("person", "Second Operator", &sources, 1)?,
            std::slice::from_ref(&updated)
        );
        assert_eq!(
            store.apply_authoritative(
                "TEST",
                vec![Change::Delete(authoritative_contact(
                    "Untrusted Delete Text",
                    "EO1-TEST"
                )?)],
                &passwords,
            )?,
            1
        );
        drop(store);

        let store = Store::open_existing(&path)?;
        assert!(store.exact("EO1-TEST", &sources, 1)?.is_empty());
        assert_eq!(store.verify()?, 1);
        assert_eq!(store.source_serial("TEST")?, None);
        assert_eq!(store.source_local_serial("TEST")?, Some(3));
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, Some((1, 3)));
        let (_, journal) = store.nrtm3_journal("TEST", 1, None, 3, 65536)?.unwrap();
        assert_eq!(
            journal.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
            [Kind::Add, Kind::Add, Kind::Delete]
        );
        assert_eq!(journal[2].object.text, updated.text);
        let audit = store.source_history("TEST", 10)?;
        assert_eq!(audit.len(), 3);
        for (offset, (entry, action)) in
            audit.iter().zip(["delete", "update", "create"]).enumerate()
        {
            assert_eq!(entry.sequence, 3 - offset as u64);
            assert!(entry.timestamp_unix_seconds > 0);
            assert_eq!(
                (
                    entry.source.as_str(),
                    entry.class.as_str(),
                    entry.key.as_str(),
                    entry.action.as_str()
                ),
                ("TEST", "person", "EO1-TEST", action)
            );
        }
        assert_eq!(store.source_history("TEST", 2)?.len(), 2);
        assert!(store.source_history("OTHER", 10)?.is_empty());
        assert!(store.source_history("TEST", 1001).is_err());
        Ok(())
    }

    #[test]
    fn authoritative_write_rolls_back_audit_and_objects_on_late_index_failure() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let passwords = [String::from("fixture password")];
        let sources = [String::from("TEST")];
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let first = authoritative_contact("First Operator", "FO1-TEST")?;
        let second = authoritative_contact("Second Operator", "SO1-TEST")?;

        let write = store.db.begin_write()?;
        write
            .open_multimap_table(EXACT)?
            .insert(("TEST", "SO1-TEST"), "person")?;
        write.commit()?;
        let error = store
            .apply_authoritative(
                "TEST",
                vec![Change::Upsert(first.clone()), Change::Upsert(second)],
                &passwords,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate entry in the exact lookup index")
        );
        assert!(store.exact("FO1-TEST", &sources, 1)?.is_empty());
        assert!(store.db.begin_read()?.open_table(AUTH_AUDIT)?.is_empty()?);
        assert_eq!(store.source_local_serial("TEST")?, None);
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, None);

        // The NRTM guard runs within the same write transaction as the update.
        store.apply_nrtm3(
            "TEST",
            (1, 1),
            Some(0),
            [Ok(Operation {
                kind: Kind::Delete,
                serial: 1,
                object: object("TEST", "aut-num", "AS64496"),
            })],
        )?;
        let error = store
            .apply_authoritative("TEST", vec![Change::Upsert(first)], &passwords)
            .unwrap_err();
        assert!(error.to_string().contains("committed NRTM serial"));
        assert!(store.db.begin_read()?.open_table(AUTH_AUDIT)?.is_empty()?);
        Ok(())
    }

    #[test]
    fn authoritative_reference_probe_excludes_self_and_other_sources() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        seed_authoritative_maintainer(&store, "fixture password")?;
        let mut elsewhere = object("OTHER", "role", "RO1-OTHER");
        elsewhere
            .text
            .push_str("mnt-by: TEST-MNT\nadmin-c: EO1-TEST\n");
        store.replace_source("OTHER", [Ok(elsewhere)])?;
        {
            let write = store.db.begin_write()?;
            let indexes = WriteIndexes::open(&write)?;
            assert!(!indexes.has_strong_reference("TEST", "mntner", "TEST-MNT")?);
            assert!(!indexes.has_strong_reference("TEST", "person", "EO1-TEST")?);
        }
        let mut role = object("TEST", "role", "RO1-TEST");
        role.text.push_str("mnt-by: TEST-MNT\ntech-c: EO1-TEST\n");
        store.replace_source("TEST", [Ok(role)])?;
        let write = store.db.begin_write()?;
        let indexes = WriteIndexes::open(&write)?;
        assert!(indexes.has_strong_reference("TEST", "mntner", "TEST-MNT")?);
        assert!(indexes.has_strong_reference("TEST", "person", "EO1-TEST")?);
        Ok(())
    }

    #[test]
    fn authoritative_protection_covers_contact_aliases_and_survives_reopen_and_purge() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let passwords = [String::from("fixture password")];
        let sources = [String::from("TEST")];
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let person = authoritative_contact("Example Operator", "EO1-TEST")?;
        let role = crate::rpsl::parse(&person.text.replacen("person:", "role:", 1))
            .map_err(anyhow::Error::msg)?;
        assert_eq!(
            store.apply_authoritative_report(
                "TEST",
                vec![Change::Upsert(person.clone())],
                &passwords
            )?,
            ["create"]
        );
        assert_eq!(
            store.apply_authoritative_report(
                "TEST",
                vec![Change::Upsert(person.clone())],
                &passwords
            )?,
            ["modify"]
        );
        let error = store
            .apply_authoritative_report(
                "TEST",
                vec![Change::Upsert(role.clone()), Change::Delete(person.clone())],
                &passwords,
            )
            .unwrap_err();
        assert!(error.to_string().contains("name is protected"));
        assert_eq!(
            store.first("person", "EO1-TEST", &sources)?.as_ref(),
            Some(&person)
        );
        assert!(store.first("role", "EO1-TEST", &sources)?.is_none());
        assert_eq!(
            store.apply_authoritative_report(
                "TEST",
                vec![Change::Delete(person.clone())],
                &passwords
            )?,
            ["delete"]
        );
        for object in [person.clone(), role.clone()] {
            let error = store
                .apply_authoritative_report("TEST", vec![Change::Upsert(object)], &passwords)
                .unwrap_err();
            assert!(error.to_string().contains("name is protected"));
        }
        let write = store.db.begin_write()?;
        assert!(write.delete_table(PROTECTED_NAMES)?);
        write.commit()?;
        drop(store);

        let store = Store::open_for_service(&path)?;
        store.replace_authoritative_source("TEST", [], None)?;
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let error = store
            .apply_authoritative_report("TEST", vec![Change::Upsert(role)], &passwords)
            .unwrap_err();
        assert!(error.to_string().contains("name is protected"));
        assert_eq!(store.source_history("TEST", 10)?.len(), 3);

        // An operator can restore the old record from a trusted snapshot and update it.
        let maintainer = store
            .first("mntner", "TEST-MNT", &sources)?
            .context("test maintainer missing")?;
        store.replace_authoritative_source("TEST", [Ok(maintainer), Ok(person.clone())], None)?;
        assert_eq!(
            store.apply_authoritative_report("TEST", vec![Change::Upsert(person)], &passwords)?,
            ["modify"]
        );
        Ok(())
    }

    #[test]
    fn authoritative_rebases_preserve_serial_floors_and_invalidate_old_history() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let passwords = ["fixture password".to_owned()];
        let sources = ["TEST".to_owned()];
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let maintainer = store.first("mntner", "TEST-MNT", &sources)?.unwrap();
        store.replace_authoritative_source("TEST", [Ok(maintainer.clone())], None)?;
        assert_eq!(store.source_local_serial("TEST")?, Some(0));
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, None);
        let mut public = Vec::new();
        assert_eq!(
            store.export_public_source("TEST", &mut public)?,
            (1, Some(0))
        );
        assert!(!String::from_utf8(public)?.contains("BCRYPT-PW"));
        assert!(store.replace_source("TEST", []).is_err());
        assert!(store.apply_nrtm3("TEST", (1, 1), Some(0), []).is_err());

        let person = authoritative_contact("Example Operator", "EO1-TEST")?;
        store.apply_authoritative("TEST", vec![Change::Upsert(person.clone())], &passwords)?;
        assert!(
            store
                .replace_authoritative_source(
                    "TEST",
                    [Err(anyhow::anyhow!("interrupted snapshot"))],
                    Some(10),
                )
                .is_err()
        );
        assert_eq!(store.source_local_serial("TEST")?, Some(1));
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, Some((1, 1)));
        assert!(
            store
                .replace_authoritative_source("TEST", [], Some(u64::MAX))
                .is_err()
        );
        assert_eq!(store.source_local_serial("TEST")?, Some(1));

        store.replace_authoritative_source("TEST", [], Some(10))?;
        assert_eq!(store.verify()?, 0);
        assert!(store.export_public_source("TEST", std::io::sink()).is_err());
        assert_eq!(store.source_local_serial("TEST")?, Some(11));
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, None);
        assert!(store.nrtm3_journal("TEST", 2, None, 16, 65536).is_err());
        assert!(store.nrtm3_journal("TEST", 12, None, 16, 65536)?.is_none());
        drop(store);

        let store = Store::open_for_service(&path)?;
        store.replace_authoritative_source("TEST", [Ok(maintainer)], None)?;
        assert_eq!(store.source_local_serial("TEST")?, Some(12));
        store.apply_authoritative("TEST", vec![Change::Upsert(person)], &passwords)?;
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, Some((13, 13)));
        assert!(store.nrtm3_journal("TEST", 12, None, 16, 65536).is_err());
        assert_eq!(store.verify()?, 2);
        assert_eq!(store.source_history("TEST", 10)?.len(), 2);
        Ok(())
    }

    #[test]
    fn authoritative_journal_retains_a_bounded_suffix_and_omits_credential_hashes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let passwords = ["fixture password".to_owned()];
        seed_authoritative_maintainer(&store, &passwords[0])?;
        let person = authoritative_contact("Example Operator", "EO1-TEST")?;
        store.apply_authoritative("TEST", vec![Change::Upsert(person.clone())], &passwords)?;
        let write = store.db.begin_write()?;
        {
            write
                .open_table(AUTH_NRTM3_SERIALS)?
                .insert("TEST", NRTM3_RETAINED_OPERATIONS)?;
            let mut journal = write.open_table(JOURNAL)?;
            for serial in 2..=NRTM3_RETAINED_OPERATIONS {
                journal.insert(
                    ("TEST", serial),
                    (true, "person", person.key.as_str(), person.text.as_str()),
                )?;
            }
        }
        write.commit()?;
        assert_eq!(store.verify()?, 2);

        let hash = bcrypt::hash(&passwords[0], 4)?;
        let updated = crate::rpsl::parse(&format!(
            "mntner: TEST-MNT\nadmin-c: EO1-TEST\nupd-to: noc@example.net\n\
             auth: BCRYPT-PW {hash}\nmnt-by: TEST-MNT\nsource: TEST\n"
        ))
        .map_err(anyhow::Error::msg)?;
        store.apply_authoritative("TEST", vec![Change::Upsert(updated)], &passwords)?;
        let newest = NRTM3_RETAINED_OPERATIONS + 1;
        assert_eq!(store.nrtm3_journal_bounds("TEST")?, Some((2, newest)));
        assert!(
            store
                .nrtm3_journal("TEST", 1, None, 5000, 16 * 1024 * 1024)
                .is_err()
        );
        let (_, records) = store
            .nrtm3_journal("TEST", newest, None, 1, 65536)?
            .unwrap();
        assert_eq!(records[0].object.class, "mntner");
        assert!(!records[0].object.text.contains("auth:"));
        assert!(!records[0].object.text.contains(&hash));
        assert_eq!(store.verify()?, 2);
        Ok(())
    }

    fn route_parent(store: &Store, route: &Object) -> Result<Option<Object>> {
        let write = store.db.begin_write()?;
        let records = write.open_table(OBJECTS)?;
        let indexes = WriteIndexes::open(&write)?;
        indexes.route_parent(&records, route)
    }

    fn allocation_parent(store: &Store, object: &Object) -> Result<Option<Object>> {
        let write = store.db.begin_write()?;
        let records = write.open_table(OBJECTS)?;
        let indexes = WriteIndexes::open(&write)?;
        indexes.allocation_parent(&records, object)
    }

    #[test]
    fn as_block_cover_backfills_and_tracks_transactional_replacement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let aut_num = object("TEST", "aut-num", "AS64496");
        let maximum = object("TEST", "aut-num", "AS4294967295");
        let broad = object("TEST", "as-block", "AS0 - AS4294967295");
        let narrower = object("TEST", "as-block", "AS64490 - AS64496");
        let exact = object("TEST", "as-block", "AS64496 - AS64496");
        let foreign = object("OTHER", "as-block", &exact.key);
        store.replace_source("OTHER", [Ok(foreign)])?;
        store.replace_source("TEST", [Ok(broad.clone()), Ok(narrower.clone())])?;
        assert_eq!(
            allocation_parent(&store, &aut_num)?.as_ref(),
            Some(&narrower)
        );
        assert_eq!(allocation_parent(&store, &maximum)?.as_ref(), Some(&broad));

        assert!(
            store
                .replace_source(
                    "TEST",
                    [
                        Ok(exact.clone()),
                        Err(anyhow::anyhow!("interrupted snapshot"))
                    ]
                )
                .is_err()
        );
        assert_eq!(
            allocation_parent(&store, &aut_num)?.as_ref(),
            Some(&narrower)
        );

        let write = store.db.begin_write()?;
        assert!(write.delete_table(AS_BLOCK_COVER)?);
        write.commit()?;
        assert_eq!(store.verify()?, 3); // Older backups need not carry the index.
        drop(store);

        let store = Store::open_for_service(&path)?;
        assert_eq!(store.verify()?, 3);
        assert_eq!(
            allocation_parent(&store, &aut_num)?.as_ref(),
            Some(&narrower)
        );
        store.replace_source("TEST", [Ok(exact.clone())])?;
        assert_eq!(allocation_parent(&store, &aut_num)?.as_ref(), Some(&exact));
        assert!(allocation_parent(&store, &maximum)?.is_none());
        assert_eq!(store.verify()?, 2);

        let write = store.db.begin_write()?;
        {
            let mut cover = write.open_table(AS_BLOCK_COVER)?;
            let key = as_block_cover_keys("TEST", &exact.key)?.next().unwrap();
            assert!(cover.remove(key)?.is_some());
        }
        write.commit()?;
        assert!(
            store
                .verify()
                .unwrap_err()
                .to_string()
                .contains("AS-block cover index")
        );
        Ok(())
    }

    #[test]
    fn as_block_cover_rejects_equally_specific_parents_without_fallback() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let aut_num = object("TEST", "aut-num", "AS64496");
        let broad = object("TEST", "as-block", "AS64000 - AS65000");
        let left = object("TEST", "as-block", "AS64490 - AS64500");
        let right = object("TEST", "as-block", "AS64495 - AS64505");
        store.replace_source("TEST", [Ok(broad), Ok(left), Ok(right)])?;
        assert!(
            allocation_parent(&store, &aut_num)
                .unwrap_err()
                .to_string()
                .contains("ambiguous covering AS-block")
        );
        Ok(())
    }

    #[test]
    fn authoritative_route_parent_prefers_smallest_same_source_allocation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let route = object("TEST", "route", "192.0.2.0/24AS64496");
        let broader = object("TEST", "inetnum", "192.0.0.0/16");
        let smaller = object("TEST", "inetnum", "192.0.1.252 - 192.0.3.4");
        let exact = object("TEST", "inetnum", "192.0.2.0 - 192.0.2.255");
        let fallback = object("TEST", "route", "192.0.2.0/23AS64497");
        let foreign = object("OTHER", "inetnum", &exact.key);
        store.replace_source("OTHER", [Ok(foreign)])?;
        store.replace_source(
            "TEST",
            [
                Ok(broader.clone()),
                Ok(smaller.clone()),
                Ok(fallback.clone()),
            ],
        )?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&smaller));
        store.replace_source(
            "TEST",
            [
                Ok(broader),
                Ok(smaller),
                Ok(exact.clone()),
                Ok(fallback.clone()),
            ],
        )?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&exact));

        let write = store.db.begin_write()?;
        assert!(write.delete_table(INETNUM_COVER)?);
        write.commit()?;
        drop(store);
        let store = Store::open_for_service(&path)?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&exact));
        assert_eq!(store.verify()?, 5);

        let partial = object("TEST", "inetnum", "192.0.2.0 - 192.0.2.254");
        let same_prefix = object("TEST", "route", "192.0.2.0/24AS64498");
        store.replace_source("TEST", [Ok(partial), Ok(fallback.clone()), Ok(same_prefix)])?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&fallback));
        assert_eq!(store.verify()?, 4);
        store.replace_source("TEST", [])?;
        assert!(route_parent(&store, &route)?.is_none());
        Ok(())
    }

    #[test]
    fn authoritative_route_parent_rejects_equal_allocation_and_route_candidates() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let route = object("TEST", "route", "192.0.2.0/25AS64496");
        let left = object("TEST", "inetnum", "192.0.1.255 - 192.0.2.128");
        let right = object("TEST", "inetnum", "192.0.2.0 - 192.0.2.129");
        store.replace_source("TEST", [Ok(left), Ok(right)])?;
        assert!(
            route_parent(&store, &route)
                .unwrap_err()
                .to_string()
                .contains("ambiguous covering inetnum")
        );

        let broad = object("TEST", "route", "192.0.2.0/23AS64497");
        let near1 = object("TEST", "route", "192.0.2.0/24AS64497");
        let near2 = object("TEST", "route", "192.0.2.0/24AS64498");
        store.replace_source("TEST", [Ok(broad), Ok(near1), Ok(near2)])?;
        assert!(
            route_parent(&store, &route)
                .unwrap_err()
                .to_string()
                .contains("ambiguous covering route")
        );
        Ok(())
    }

    #[test]
    fn authoritative_route6_parent_prefers_exact_then_smallest_inet6num() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let route = object("TEST", "route6", "2001:DB8:1::/48AS64496");
        let exact = object("TEST", "inet6num", "2001:DB8:1::/48");
        let parent = object("TEST", "inet6num", "2001:DB8::/32");
        let fallback = object("TEST", "route6", "2001:DB8::/47AS64497");
        store.replace_source("TEST", [Ok(parent.clone()), Ok(fallback.clone())])?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&parent));
        store.replace_source(
            "TEST",
            [Ok(exact.clone()), Ok(parent), Ok(fallback.clone())],
        )?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&exact));
        store.replace_source("TEST", [Ok(fallback.clone())])?;
        assert_eq!(route_parent(&store, &route)?.as_ref(), Some(&fallback));
        Ok(())
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

    const NRTM4_SESSION_A: &str = "c86f0f62-447e-4bdf-8cb6-18f861bc3532";
    const NRTM4_SESSION_B: &str = "06c2a859-f39a-48f4-9913-e111985a7476";

    #[test]
    fn nrtm4_applies_snapshot_and_deltas_with_committed_state_across_reopen() -> Result<()> {
        use crate::nrtm4_state::fixture_state;

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        let store = Store::open(&path)?;
        let sources = [String::from("TEST")];
        let initial = fixture_state("TEST", NRTM4_SESSION_A, 2);
        let next = fixture_state("TEST", NRTM4_SESSION_A, 3);
        let snapshot = object("TEST", "route", "192.0.2.0/24AS64496");
        let mut delta = object("TEST", "route", "198.51.100.0/24AS64496");
        delta.text.push_str("mnt-by: TEST-MNT\n");
        store.replace_source("TEST", [Ok(object("TEST", "aut-num", "AS64500"))])?;
        store.replace_source("OTHER", [Ok(object("OTHER", "aut-num", "AS64500"))])?;
        assert!(
            store
                .apply_nrtm4("TEST", None, &initial, false, [])
                .is_err()
        );
        assert_eq!(
            store.apply_nrtm4(
                "TEST",
                None,
                &initial,
                true,
                [
                    Ok(Nrtm4Record::Snapshot(snapshot.clone())),
                    Ok(Nrtm4Record::AddModify(delta.clone())),
                    Ok(Nrtm4Record::Delete {
                        class: snapshot.class,
                        key: snapshot.key
                    }),
                ]
            )?,
            3
        );
        assert!(store.exact("AS64500", &sources, 2)?.is_empty());
        assert_eq!(
            store.exact("AS64500", &[String::from("OTHER")], 2)?.len(),
            1
        );
        assert_eq!(
            store.origin_prefixes("route", "AS64496", &sources, 2)?,
            ["198.51.100.0/24"]
        );
        assert_eq!(
            store.inverse("mnt-by", "TEST-MNT", &sources, 2)?,
            std::slice::from_ref(&delta)
        );
        drop(store);

        let store = Store::open_existing(&path)?;
        assert_eq!(store.nrtm4_state("test")?.as_ref(), Some(&initial));
        assert_eq!(store.source_serial("TEST")?, None);
        let deletion = || {
            Ok(Nrtm4Record::Delete {
                class: delta.class.clone(),
                key: delta.key.clone(),
            })
        };
        assert_eq!(
            store.apply_nrtm4("TEST", Some(&initial), &next, false, [deletion()])?,
            1
        );
        assert!(
            store
                .apply_nrtm4("TEST", Some(&initial), &next, false, [deletion()])
                .is_err()
        );
        assert!(
            store
                .apply_nrtm4("TEST", Some(&next), &initial, false, [deletion()])
                .is_err()
        );
        assert!(
            store
                .apply_nrtm4("TEST", Some(&next), &next, false, [deletion()])
                .is_err()
        );
        assert_eq!(store.apply_nrtm4("TEST", Some(&next), &next, false, [])?, 0);
        assert_eq!(store.nrtm4_state("TEST")?.as_ref(), Some(&next));
        assert!(store.inverse("mnt-by", "TEST-MNT", &sources, 2)?.is_empty());
        assert_eq!(store.verify()?, 1);
        Ok(())
    }

    #[test]
    fn nrtm4_rolls_back_late_errors_and_requires_snapshot_for_new_session() -> Result<()> {
        use crate::nrtm4_state::fixture_state;

        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let sources = [String::from("TEST")];
        let initial = fixture_state("TEST", NRTM4_SESSION_A, 7);
        let next = fixture_state("TEST", NRTM4_SESSION_B, 1);
        let old = object("TEST", "aut-num", "AS64496");
        let new = object("TEST", "aut-num", "AS64497");
        store.apply_nrtm4(
            "TEST",
            None,
            &initial,
            true,
            [Ok(Nrtm4Record::Snapshot(old.clone()))],
        )?;
        assert!(
            store
                .apply_nrtm4(
                    "TEST",
                    Some(&initial),
                    &next,
                    false,
                    [Ok(Nrtm4Record::AddModify(new.clone()))]
                )
                .is_err()
        );
        for records in [
            [
                Ok(Nrtm4Record::Snapshot(new.clone())),
                Err(anyhow::anyhow!("invalid terminal record")),
            ],
            [
                Ok(Nrtm4Record::AddModify(new.clone())),
                Ok(Nrtm4Record::Snapshot(old.clone())),
            ],
            [
                Ok(Nrtm4Record::Snapshot(new.clone())),
                Ok(Nrtm4Record::Snapshot(new.clone())),
            ],
            [
                Ok(Nrtm4Record::Snapshot(new.clone())),
                Ok(Nrtm4Record::AddModify(object(
                    "OTHER", "aut-num", "AS64498",
                ))),
            ],
        ] {
            assert!(
                store
                    .apply_nrtm4("TEST", Some(&initial), &next, true, records)
                    .is_err()
            );
            assert_eq!(store.nrtm4_state("TEST")?.as_ref(), Some(&initial));
            assert_eq!(
                store.exact("AS64496", &sources, 2)?,
                std::slice::from_ref(&old)
            );
            assert!(store.exact("AS64497", &sources, 2)?.is_empty());
        }
        assert_eq!(
            store.apply_nrtm4(
                "TEST",
                Some(&initial),
                &next,
                true,
                [Ok(Nrtm4Record::Snapshot(new))]
            )?,
            1
        );
        assert_eq!(store.nrtm4_state("TEST")?.as_ref(), Some(&next));
        assert!(store.exact("AS64496", &sources, 2)?.is_empty());
        assert_eq!(store.verify()?, 1);
        Ok(())
    }

    #[test]
    fn nrtm4_state_excludes_other_write_modes_and_rejects_corrupt_metadata() -> Result<()> {
        use crate::nrtm4_state::fixture_state;

        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("registry.redb"))?;
        let state = fixture_state("TEST", NRTM4_SESSION_A, 1);
        let test_object = object("TEST", "aut-num", "AS64496");
        store.apply_nrtm4(
            "TEST",
            None,
            &state,
            true,
            [Ok(Nrtm4Record::Snapshot(test_object.clone()))],
        )?;
        assert!(
            store
                .replace_source("TEST", [Ok(test_object.clone())])
                .is_err()
        );
        assert!(store.replace_source("TEST", []).is_err());
        assert!(
            store
                .apply_nrtm3(
                    "TEST",
                    (1, 1),
                    Some(0),
                    [Ok(Operation {
                        kind: Kind::Add,
                        serial: 1,
                        object: test_object.clone()
                    })]
                )
                .is_err()
        );
        assert!(
            store
                .apply_authoritative("TEST", vec![Change::Upsert(test_object)], &[])
                .is_err()
        );
        assert_eq!(store.nrtm4_state("TEST")?.as_ref(), Some(&state));

        let legacy = object("LEGACY", "aut-num", "AS64496");
        store.replace_source("LEGACY", [Ok(legacy.clone())])?;
        store.apply_nrtm3(
            "LEGACY",
            (1, 1),
            Some(0),
            [Ok(Operation {
                kind: Kind::Add,
                serial: 1,
                object: legacy,
            })],
        )?;
        let legacy_state = fixture_state("LEGACY", NRTM4_SESSION_A, 1);
        let error = store
            .apply_nrtm4("LEGACY", None, &legacy_state, true, [])
            .unwrap_err();
        assert!(error.to_string().contains("committed NRTM serial"));

        let write = store.db.begin_write()?;
        write
            .open_table(AUTH_AUDIT)?
            .insert(("AUTH", 1), (1, "person", "EO1-AUTH", "delete"))?;
        write.commit()?;
        let auth_state = fixture_state("AUTH", NRTM4_SESSION_A, 1);
        let error = store
            .apply_nrtm4("AUTH", None, &auth_state, true, [])
            .unwrap_err();
        assert!(error.to_string().contains("authoritative history"));

        let write = store.db.begin_write()?;
        write.open_table(NRTM4_STATE)?.insert("TEST", "{}")?;
        write.commit()?;
        assert!(store.nrtm4_state("TEST").is_err());
        assert!(store.verify().is_err());
        assert!(store.replace_source("TEST", []).is_err());
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
