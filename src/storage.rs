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
    Database, Durability, MultimapTableDefinition, MultimapTableHandle, ReadOnlyTable,
    ReadableMultimapTable, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle,
};

use crate::rpsl::{Object, lookup_attributes};

type Identity = (&'static str, &'static str, &'static str);
type ExactKey = (&'static str, &'static str);
type PrefixKey<'a> = (&'a str, &'a str, u128, u8, u32);
type InverseKey = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

// Identity is (source, class, key). The exact index maps (source, key) to classes.
const OBJECTS: TableDefinition<Identity, &str> = TableDefinition::new("objects_v1");
const EXACT: MultimapTableDefinition<ExactKey, &str> = MultimapTableDefinition::new("exact_v1");
// The origin index maps (source, class, origin) to distinct canonical prefixes.
const ORIGINS: MultimapTableDefinition<Identity, &str> = MultimapTableDefinition::new("origins_v1");
// Per-source order by network address permits one range scan for all descendants.
const PREFIXES: TableDefinition<PrefixKey<'static>, ()> = TableDefinition::new("route_prefixes_v1");
// A row is (source, field, normalized value, class, object key).
const INVERSE: TableDefinition<InverseKey, ()> = TableDefinition::new("inverse_v1");

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
        let mut skipped_lookup_values = 0;
        {
            let mut records = write.open_table(OBJECTS)?;
            let mut exact = write.open_multimap_table(EXACT)?;
            let mut origins = write.open_multimap_table(ORIGINS)?;
            let mut prefixes = write.open_table(PREFIXES)?;
            let mut inverse = write.open_table(INVERSE)?;

            // Appending the smallest character gives the exclusive end of this source's range.
            let next_source = format!("{source}\0");
            let old_records = records.extract_from_if(
                (source.as_str(), "", "")..(next_source.as_str(), "", ""),
                |_, _| true,
            )?;
            for entry in old_records {
                let (identity, text) = entry?;
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
                    ensure!(
                        prefixes
                            .remove(prefix_index_key(source, class, prefix, origin)?)?
                            .is_some(),
                        "object is missing from the route prefix index"
                    );
                }
                let attributes =
                    lookup_attributes(class, text.value()).map_err(anyhow::Error::msg)?;
                for (field, value) in attributes.inverse_entries().collect::<HashSet<_>>() {
                    ensure!(
                        inverse
                            .remove((source, field, value.as_ref(), class, key))?
                            .is_some(),
                        "object is missing from the inverse lookup index"
                    );
                }
            }

            for object in objects {
                let object = object?;
                let attributes = object.lookup_attributes().map_err(anyhow::Error::msg)?;
                let Object {
                    source: object_source,
                    class,
                    key,
                    text,
                } = object;
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
                    ensure!(
                        prefixes
                            .insert(prefix_index_key(&source, &class, prefix, origin)?, ())?
                            .is_none(),
                        "unexpected duplicate entry in the route prefix index"
                    );
                }
                skipped_lookup_values += attributes.skipped_invalid_values;
                for (field, value) in attributes.inverse_entries() {
                    inverse.insert(
                        (
                            source.as_str(),
                            field,
                            value.as_ref(),
                            class.as_str(),
                            key.as_str(),
                        ),
                        (),
                    )?;
                }
                count += 1;
            }
        }
        write.commit()?;
        if skipped_lookup_values != 0 {
            tracing::warn!(%source, skipped_lookup_values, "retained malformed lookup attribute values in snapshot text without indexing them");
        }
        Ok(count)
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

    /// Check that the indexes cover the stored identities and attributes.
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
        Ok(indexed)
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
    fn verifies_index_coverage_and_dangling_references() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("registry.redb");
        assert!(Store::open_existing(&path).is_err());
        assert!(Store::open_for_service(&path).is_err());
        assert!(!path.exists());
        drop(Database::create(&path)?);
        assert!(Store::open_for_service(&path).is_err());
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
