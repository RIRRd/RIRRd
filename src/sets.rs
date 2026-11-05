//! Bounded expansion of AS-SET and ROUTE-SET data for IRRd WHOIS queries.

use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, ensure};

use crate::{
    rpsl::lookup::{LookupAttributes, SetMember},
    storage::Store,
};

pub(crate) const MAX_DEPTH: usize = 64;
const MAX_SET_LOOKUPS: usize = 65_536;
const MAX_ORIGIN_LOOKUPS: usize = 131_072;
const MAX_VISITED_VALUES: usize = 2_000_000;
const MAX_WORK_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_RESULTS: usize = 262_144;
pub(crate) const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
// Large transit networks can contain millions of distinct prefixes. Keep their
// bounded output separate from AS/member traversal and object lookup limits.
pub(crate) const MAX_ROUTE_RESULTS: usize = 4_000_000;
pub(crate) const MAX_ROUTE_BYTES: usize = 96 * 1024 * 1024;

struct ExpansionLimits {
    sets: usize,
    origins: usize,
    visited_values: usize,
    work_bytes: usize,
    results: usize,
    result_bytes: usize,
}

impl Default for ExpansionLimits {
    fn default() -> Self {
        Self {
            sets: MAX_SET_LOOKUPS,
            origins: MAX_ORIGIN_LOOKUPS,
            visited_values: MAX_VISITED_VALUES,
            work_bytes: MAX_WORK_BYTES,
            results: MAX_RESULTS,
            result_bytes: MAX_RESULT_BYTES,
        }
    }
}

/// Shared by all prefix/member fields in one GraphQL request. Charge while
/// collecting, so aliases cannot multiply each field's materialization cap.
pub(crate) struct ResultBudget {
    values: AtomicUsize,
    bytes: AtomicUsize,
    route_values: AtomicUsize,
    route_bytes: AtomicUsize,
    route_used: AtomicUsize,
    heavy: std::sync::Mutex<Option<Arc<crate::storage::HeavyQueryLease>>>,
}

impl Default for ResultBudget {
    fn default() -> Self {
        Self {
            route_values: AtomicUsize::new(MAX_ROUTE_RESULTS),
            route_bytes: AtomicUsize::new(MAX_ROUTE_BYTES),
            ..Self::new(MAX_RESULTS, MAX_RESULT_BYTES)
        }
    }
}

impl ResultBudget {
    pub(crate) fn new(values: usize, bytes: usize) -> Self {
        Self {
            values: AtomicUsize::new(values),
            bytes: AtomicUsize::new(bytes),
            route_values: AtomicUsize::new(values),
            route_bytes: AtomicUsize::new(bytes),
            route_used: AtomicUsize::new(0),
            heavy: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn charge(&self, bytes: usize) -> Result<()> {
        self.consume(&self.values, 1)?;
        self.consume(&self.bytes, bytes)?;
        self.charge_route(bytes)
    }

    pub(crate) fn charge_route(&self, bytes: usize) -> Result<()> {
        self.consume(&self.route_values, 1)?;
        self.consume(&self.route_bytes, bytes)?;
        if self.route_used.fetch_add(1, Ordering::Relaxed) >= MAX_RESULTS {
            let mut lease = self.heavy.lock().expect("request result lease");
            if lease.is_none() {
                *lease = Some(crate::storage::acquire_heavy_query()?);
            }
        }
        Ok(())
    }

    fn consume(&self, counter: &AtomicUsize, amount: usize) -> Result<()> {
        fn consume(counter: &AtomicUsize, amount: usize) -> Result<()> {
            let mut previous = counter.load(Ordering::Relaxed);
            loop {
                let next = previous
                    .checked_sub(amount)
                    .ok_or(LimitExceeded("request exceeds prefix/member result budget"))?;
                match counter.compare_exchange_weak(
                    previous,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok(()),
                    Err(current) => previous = current,
                }
            }
        }
        consume(counter, amount)
    }
}

struct SetDefinition {
    source: String,
    key: String,
    class: &'static str,
    attributes: LookupAttributes,
}

/// A valid set query exceeded an explicit work or response budget. This is a
/// client-visible limit, not a database failure; callers must not return a
/// partial expansion or misreport it as an internal server error.
#[derive(Debug)]
pub(crate) struct LimitExceeded(&'static str);

impl fmt::Display for LimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for LimitExceeded {}

pub fn members(
    store: &Store,
    sources: &[String],
    name: &str,
    recursive: bool,
    excluded: &[String],
) -> Result<Vec<String>> {
    let name = name.to_uppercase();
    let mut expansion = Expansion::new(store, sources, excluded);
    let Some(root) = find_set(store, sources, &name, false, &mut expansion.set_text_bytes)? else {
        return Ok(Vec::new());
    };
    if recursive {
        let mode = RecursiveMode::for_members(&root);
        expansion.expand(root, mode, None)?;
    } else {
        let mut remaining_lookups = expansion.indirect_lookups;
        visit_direct_members(store, root, &mut remaining_lookups, |member| {
            let member = member.to_string();
            expansion.charge(member.len())?;
            if member != name {
                expansion.insert(member)?;
            }
            Ok(())
        })?;
    }
    Ok(expansion.output.into_iter().collect())
}

/// Resolve a set in one specific registry, following nested references using
/// the independently selected source order.
pub(crate) fn recursive_members_at_root(
    store: &Store,
    sources: &[String],
    root_source: &str,
    name: &str,
    depth: Option<usize>,
    excluded: &[String],
    budget: Option<Arc<ResultBudget>>,
) -> Result<Option<Vec<String>>> {
    let mut expansion = Expansion::new(store, sources, excluded);
    expansion.result_budget = budget;
    let Some(root) = find_set(
        store,
        &[root_source.to_owned()],
        name,
        false,
        &mut expansion.set_text_bytes,
    )?
    else {
        return Ok(None);
    };
    let key = root.key.clone();
    let mode = RecursiveMode::for_members(&root);
    expansion.expand(root, mode, depth)?;
    expansion.output.remove(&key);
    Ok(Some(expansion.output.into_iter().collect()))
}

/// Expand origins of AS-set members. Family is None for both, false for IPv4,
/// and true for IPv6.
pub fn routes_for_as_set(
    store: &Store,
    sources: &[String],
    name: &str,
    family: Option<bool>,
    excluded: &[String],
) -> Result<Vec<String>> {
    routes_for_as_set_with_budget(store, sources, name, family, excluded, None)
}

pub(crate) fn routes_for_as_set_with_budget(
    store: &Store,
    sources: &[String],
    name: &str,
    family: Option<bool>,
    excluded: &[String],
    budget: Option<Arc<ResultBudget>>,
) -> Result<Vec<String>> {
    let name = name.to_uppercase();
    let mut expansion = Expansion::new(store, sources, excluded);
    expansion.result_budget = budget;
    expansion.enable_routes();
    let Some(root) = find_set(store, sources, &name, true, &mut expansion.set_text_bytes)? else {
        return Ok(Vec::new());
    };
    expansion.expand(root, RecursiveMode::OriginPrefixes(family), None)?;
    expansion.finish_routes()
}

fn find_set(
    store: &Store,
    sources: &[String],
    key: &str,
    as_only: bool,
    remaining_bytes: &mut usize,
) -> Result<Option<SetDefinition>> {
    for class in ["as-set", "route-set"] {
        if class == "route-set" && as_only {
            break;
        }
        if let Some((source, attributes)) =
            store.first_set_attributes(class, key, sources, remaining_bytes)?
        {
            return Ok(Some(SetDefinition {
                source,
                key: key.to_owned(),
                class,
                attributes,
            }));
        }
    }
    Ok(None)
}

fn visit_direct_members(
    store: &Store,
    object: SetDefinition,
    remaining_lookups: &mut usize,
    mut visit: impl FnMut(SetMember) -> Result<()>,
) -> Result<()> {
    let lookup = object.attributes;
    for member in lookup.members.into_iter().chain(lookup.mp_members) {
        visit(member)?;
    }
    store.visit_indirect_set_members(
        &object.source,
        &object.key,
        object.class == "as-set",
        &lookup.mbrs_by_ref,
        remaining_lookups,
        visit,
    )
}

#[derive(Clone, Copy)]
enum RecursiveMode {
    AsMembers,
    RouteMembers,
    OriginPrefixes(Option<bool>),
}

impl RecursiveMode {
    fn for_members(root: &SetDefinition) -> Self {
        if root.class == "route-set" {
            Self::RouteMembers
        } else {
            Self::AsMembers
        }
    }
}

struct Expansion<'a> {
    store: &'a Store,
    sources: &'a [String],
    excluded: HashSet<String>,
    seen_sets: HashSet<String>,
    seen_origins: HashSet<u32>,
    output: BTreeSet<String>,
    route_output: Option<HashSet<Box<str>>>,
    heavy_lease: Option<Arc<crate::storage::HeavyQueryLease>>,
    output_bytes: usize,
    visited_values: usize,
    work_bytes: usize,
    set_text_bytes: usize,
    indirect_lookups: usize,
    result_budget: Option<Arc<ResultBudget>>,
    limits: ExpansionLimits,
}

impl<'a> Expansion<'a> {
    fn new(store: &'a Store, sources: &'a [String], excluded: &[String]) -> Self {
        Self {
            store,
            sources,
            excluded: excluded.iter().map(|name| name.to_uppercase()).collect(),
            seen_sets: HashSet::new(),
            seen_origins: HashSet::new(),
            output: BTreeSet::new(),
            route_output: None,
            heavy_lease: None,
            output_bytes: 0,
            visited_values: 0,
            work_bytes: 0,
            set_text_bytes: MAX_WORK_BYTES,
            indirect_lookups: MAX_VISITED_VALUES,
            result_budget: None,
            limits: ExpansionLimits::default(),
        }
    }

    fn enable_routes(&mut self) {
        self.route_output = Some(HashSet::new());
        self.limits.results = MAX_ROUTE_RESULTS;
        self.limits.result_bytes = MAX_ROUTE_BYTES;
        self.limits.visited_values = 16_000_000;
        self.limits.work_bytes = 256 * 1024 * 1024;
    }

    fn finish_routes(self) -> Result<Vec<String>> {
        let mut values: Vec<String> = self
            .route_output
            .expect("route collector")
            .into_iter()
            .map(|value| {
                crate::storage::check_query_deadline()?;
                Ok(String::from(value))
            })
            .collect::<Result<_>>()?;
        crate::storage::check_query_deadline()?;
        values.sort_unstable();
        crate::storage::check_query_deadline()?;
        Ok(values)
    }

    fn expand(
        &mut self,
        root: SetDefinition,
        mode: RecursiveMode,
        limit: Option<usize>,
    ) -> Result<()> {
        let mut pending = VecDeque::new();
        self.seen_sets.insert(root.key.clone());
        self.expand_members(root, mode, 0, limit == Some(1), &mut pending)?;
        while let Some((key, depth)) = pending.pop_front() {
            ensure!(
                depth <= MAX_DEPTH && self.seen_sets.len() <= self.limits.sets,
                LimitExceeded("set exceeds traversal limit")
            );
            let as_only = !matches!(mode, RecursiveMode::RouteMembers);
            let cutoff = limit == Some(depth + 1);
            if let Some(object) = find_set(
                self.store,
                self.sources,
                &key,
                as_only,
                &mut self.set_text_bytes,
            )? {
                self.expand_members(object, mode, depth, cutoff, &mut pending)?;
            } else if cutoff {
                self.insert(key)?;
            }
        }
        Ok(())
    }

    fn expand_members(
        &mut self,
        object: SetDefinition,
        mode: RecursiveMode,
        depth: usize,
        cutoff: bool,
        pending: &mut VecDeque<(String, usize)>,
    ) -> Result<()> {
        let mut remaining_lookups = self.indirect_lookups;
        let result = visit_direct_members(self.store, object, &mut remaining_lookups, |member| {
            let wire = member.to_string();
            self.charge(wire.len())?;
            if cutoff {
                self.insert(wire.clone())?;
            }
            match member {
                SetMember::Set(name, None) if !cutoff && !self.excluded.contains(&name) => {
                    // Deduplicate on discovery, so repeated references cannot
                    // fill the pending queue before their first expansion.
                    if self.seen_sets.insert(name.clone()) {
                        ensure!(
                            self.seen_sets.len() <= self.limits.sets,
                            LimitExceeded("set exceeds traversal limit")
                        );
                        pending.push_back((name, depth + 1));
                    }
                }
                SetMember::Asn(number, None) => match mode {
                    RecursiveMode::AsMembers => self.insert(wire)?,
                    RecursiveMode::RouteMembers => self.expand_origin(number, None)?,
                    RecursiveMode::OriginPrefixes(family) => self.expand_origin(number, family)?,
                },
                SetMember::Prefix(_, _) if matches!(mode, RecursiveMode::RouteMembers) => {
                    self.insert(wire)?;
                }
                _ => {}
            }
            Ok(())
        });
        self.indirect_lookups = remaining_lookups;
        result
    }

    fn expand_origin(&mut self, number: u32, family: Option<bool>) -> Result<()> {
        if !self.seen_origins.insert(number) {
            return Ok(());
        }
        ensure!(
            self.seen_origins.len() <= self.limits.origins,
            LimitExceeded("set exceeds origin lookup limit")
        );
        let origin = format!("AS{number}");
        for (ipv6, class) in [(false, "route"), (true, "route6")] {
            if family.is_some_and(|value| value != ipv6) {
                continue;
            }
            self.store
                .visit_origin_prefixes(class, &origin, self.sources, |prefix| {
                    self.charge(prefix.len())?;
                    self.insert(prefix.to_ascii_lowercase())
                })?;
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        crate::storage::check_query_deadline()?;
        self.visited_values += 1;
        self.work_bytes += bytes;
        ensure!(
            self.visited_values <= self.limits.visited_values
                && self.work_bytes <= self.limits.work_bytes,
            LimitExceeded("set exceeds expansion work limit")
        );
        Ok(())
    }

    fn insert(&mut self, value: String) -> Result<()> {
        if let Some(output) = &mut self.route_output {
            if output.contains(value.as_str()) {
                return Ok(());
            }
            if output.len() == MAX_RESULTS && self.heavy_lease.is_none() {
                self.heavy_lease = Some(crate::storage::acquire_heavy_query()?);
            }
            ensure!(
                output.len() < self.limits.results
                    && self.output_bytes + value.len() < self.limits.result_bytes,
                LimitExceeded("set exceeds result limit")
            );
            if let Some(budget) = &self.result_budget {
                budget.charge_route(value.len() + 1)?;
            }
            self.output_bytes += value.len() + 1;
            output.insert(value.into_boxed_str());
            return Ok(());
        }
        if !self.output.contains(&value) {
            if let Some(budget) = &self.result_budget {
                budget.charge(value.len() + 1)?;
            }
            self.output_bytes += value.len() + 1;
            ensure!(
                self.output.len() < self.limits.results
                    && self.output_bytes <= self.limits.result_bytes,
                LimitExceeded("set exceeds result limit")
            );
            self.output.insert(value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpsl::parse_snapshot;

    /// Offline operator diagnostic, never used by a public listener. Supply a
    /// private config for an inactive database; the explicit probe profile has
    /// finite bounds and an outer process timeout/memory cgroup is recommended.
    #[test]
    #[ignore = "requires an explicitly selected inactive operator database"]
    fn measure_offline_large_set() -> Result<()> {
        let path = std::env::var_os("RIRRD_QUERY_REVIEW_CONFIG")
            .context("set RIRRD_QUERY_REVIEW_CONFIG")?;
        let config = crate::config::Config::load(std::path::Path::new(&path))?;
        let store =
            Store::open_existing_with_cache(&config.database, config.database_cache_bytes())?;
        let name =
            std::env::var("RIRRD_QUERY_REVIEW_NAME").context("set RIRRD_QUERY_REVIEW_NAME")?;
        let family = match std::env::var("RIRRD_QUERY_REVIEW_FAMILY").as_deref() {
            Ok("4") => Some(false),
            Ok("6") => Some(true),
            Ok("all") => None,
            _ => anyhow::bail!("set family to 4, 6 or all"),
        };
        let mut sources = config.sources.clone();
        if let Ok(first) = std::env::var("RIRRD_QUERY_REVIEW_FIRST_SOURCE") {
            ensure!(sources.contains(&first), "unknown preferred source");
            sources.retain(|source| source != &first);
            sources.insert(0, first);
        }
        let started = std::time::Instant::now();
        let mut expansion = Expansion::new(&store, &sources, &[]);
        expansion.enable_routes();
        if std::env::var("RIRRD_QUERY_REVIEW_PROFILE").as_deref() == Ok("bounded-probe") {
            expansion.limits = ExpansionLimits {
                sets: 262_144,
                origins: 1_000_000,
                visited_values: 32_000_000,
                work_bytes: 512 * 1024 * 1024,
                results: 4_000_000,
                result_bytes: 128 * 1024 * 1024,
            };
            expansion.set_text_bytes = 512 * 1024 * 1024;
            expansion.indirect_lookups = 32_000_000;
        }
        let root = find_set(&store, &sources, &name, true, &mut expansion.set_text_bytes)?
            .context("set missing")?;
        let root_source = root.source.clone();
        let result = expansion.expand(root, RecursiveMode::OriginPrefixes(family), None);
        println!(
            "{}",
            serde_json::json!({"name":name,"root_source":root_source,"family":family,
            "completed":result.is_ok(),"error":result.err().map(|error|error.to_string()),
            "results":expansion.route_output.as_ref().map_or(expansion.output.len(), HashSet::len),"result_bytes":expansion.output_bytes,
            "visited_sets":expansion.seen_sets.len(),"visited_origins":expansion.seen_origins.len(),
            "visited_values":expansion.visited_values,"work_bytes":expansion.work_bytes,
            "seconds":started.elapsed().as_secs_f64()})
        );
        Ok(())
    }
    use anyhow::{Context, anyhow};

    fn fixture(store: &Store, source: &str, objects: &[&str]) -> Result<()> {
        store.replace_source(
            source,
            objects.iter().map(|text| {
                parse_snapshot(&format!("{text}\nsource: {source}\n"))
                    .map_err(|error| anyhow!(error))
            }),
        )?;
        Ok(())
    }

    #[test]
    fn route_collector_deduplicates_sorts_and_keeps_explicit_bounds() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("registry.redb"))?;
        let mut expansion = Expansion::new(&store, &[], &[]);
        expansion.enable_routes();
        expansion.limits.results = 2;
        expansion.insert("2001:db8::/32".into())?;
        expansion.insert("192.0.2.0/24".into())?;
        expansion.insert("2001:db8::/32".into())?;
        assert_eq!(
            expansion.finish_routes()?,
            ["192.0.2.0/24", "2001:db8::/32"]
        );
        let mut oversized = Expansion::new(&store, &[], &[]);
        oversized.enable_routes();
        oversized.limits.result_bytes = 8;
        assert!(oversized.insert("192.0.2.0/24".into()).is_err());
        assert!(oversized.route_output.as_ref().unwrap().is_empty());
        Ok(())
    }

    #[test]
    fn resolves_ordered_sources_and_indirect_membership_with_maintainers() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("registry.redb"))?;
        fixture(
            &store,
            "FIRST",
            &[
                "route-set: AS-ROOT\nmembers: 192.0.2.0/24",
                "as-set: AS-CHILD\nmembers: AS64497,AS-ROOT",
                "aut-num: AS64500\nmember-of: AS-ROOT\nmnt-by: MNT-ALLOWED",
            ],
        )?;
        fixture(
            &store,
            "SECOND",
            &[
                "as-set: AS-ROOT\nmembers: AS64496,AS-CHILD,AS-MISSING\nmbrs-by-ref: MNT-ALLOWED",
                "as-set: AS-CHILD\nmembers: AS64498",
                "aut-num: AS64499\nmember-of: AS-ROOT\nmnt-by: MNT-ALLOWED",
                "aut-num: AS64501\nmember-of: AS-ROOT\nmnt-by: MNT-DENIED",
            ],
        )?;
        let sources = ["FIRST".to_owned(), "SECOND".to_owned()];
        assert_eq!(
            members(&store, &sources, "as-root", false, &["AS-CHILD".into()])?,
            ["AS-CHILD", "AS-MISSING", "AS64496", "AS64499"]
        );
        assert_eq!(
            members(&store, &sources, "AS-ROOT", true, &[])?,
            ["AS64496", "AS64497", "AS64499"]
        );
        assert_eq!(
            members(&store, &sources, "AS-ROOT", true, &["as-child".into()])?,
            ["AS64496", "AS64499"]
        );
        assert!(members(&store, &sources, "AS-MISSING", true, &[])?.is_empty());
        Ok(())
    }

    #[test]
    fn resolves_prefixes_by_family_and_keeps_literal_range_operators() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("registry.redb"))?;
        fixture(
            &store,
            "TEST",
            &[
                "as-set: AS-ROOT\nmembers: AS64496,AS-ROOT",
                "route: 192.0.2.0/24\norigin: AS64496",
                "route6: 2001:db8::/32\norigin: AS64496\nmember-of: RS-ROOT",
                "route-set: RS-ROOT\nmembers: AS-ROOT,192.0.2.0/24^+,RS-EMPTY^-\nmbrs-by-ref: ANY",
                "route-set: RS-EMPTY\nmembers: 203.0.113.0/24",
                "route-set: RS-MALFORMED\nmbrs-by-ref: ANY,INVALID!",
                "route: 198.51.100.0/24\norigin: AS64497\nmember-of: RS-MALFORMED",
            ],
        )?;
        let sources = ["TEST".to_owned()];
        assert_eq!(
            routes_for_as_set(&store, &sources, "AS-ROOT", None, &[])?,
            ["192.0.2.0/24", "2001:db8::/32"]
        );
        assert_eq!(
            routes_for_as_set(&store, &sources, "AS-ROOT", Some(true), &[])?,
            ["2001:db8::/32"]
        );
        assert_eq!(
            members(&store, &sources, "RS-ROOT", false, &[])?,
            ["192.0.2.0/24^+", "2001:db8::/32", "AS-ROOT", "RS-EMPTY^-"]
        );
        assert_eq!(
            members(&store, &sources, "RS-ROOT", true, &[])?,
            ["192.0.2.0/24", "192.0.2.0/24^+", "2001:db8::/32"]
        );
        assert!(members(&store, &sources, "RS-MALFORMED", false, &[])?.is_empty());
        Ok(())
    }

    #[test]
    fn oversized_result_fails_without_partial_members() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("registry.redb"))?;
        let members_list = (0..=MAX_RESULTS)
            .map(|number| format!("AS{number}"))
            .collect::<Vec<_>>()
            .join(",");
        let object = format!("as-set: AS-BIG\nmembers: {members_list}");
        fixture(&store, "TEST", &[&object])?;
        let error = members(&store, &["TEST".into()], "AS-BIG", false, &[]).unwrap_err();
        assert!(error.is::<crate::storage::QueryMaterializationLimit>());
        Ok(())
    }

    #[test]
    fn set_and_indirect_expansion_borrows_large_records_and_enforces_index_work() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(&directory.path().join("registry.redb"))?;
        let large_remarks = "x".repeat(crate::storage::MAX_QUERY_OBJECT_BYTES + 1);
        fixture(
            &store,
            "TEST",
            &[
                &format!(
                    "as-set: AS-BORROWED\nmembers: AS64496\nmbrs-by-ref: ALLOWED-MNT\nremarks: {large_remarks}"
                ),
                &format!(
                    "aut-num: AS64497\nmember-of: AS-BORROWED\nmnt-by: ALLOWED-MNT\nremarks: {large_remarks}"
                ),
                "aut-num: AS64498\nmember-of: AS-BORROWED\nmnt-by: WRONG-MNT",
                &format!("route: 192.0.2.0/24\norigin: AS64497\nremarks: {large_remarks}"),
            ],
        )?;
        let sources = ["TEST".to_owned()];
        assert!(
            store
                .first("as-set", "AS-BORROWED", &sources)
                .unwrap_err()
                .is::<crate::storage::QueryMaterializationLimit>()
        );
        assert_eq!(
            members(&store, &sources, "AS-BORROWED", true, &[])?,
            ["AS64496", "AS64497"]
        );
        assert_eq!(
            routes_for_as_set(&store, &sources, "AS-BORROWED", None, &[])?,
            ["192.0.2.0/24"]
        );
        let error = store
            .visit_indirect_set_members(
                "TEST",
                "AS-BORROWED",
                true,
                &["ALLOWED-MNT".into()],
                &mut 1,
                |_| Ok(()),
            )
            .unwrap_err();
        assert!(error.is::<crate::storage::QueryScanLimit>());
        assert_eq!(
            members(&store, &sources, "AS-BORROWED", false, &[])?,
            ["AS64496", "AS64497"]
        );
        Ok(())
    }
}
