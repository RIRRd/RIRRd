//! Bounded expansion of AS-SET and ROUTE-SET data for IRRd WHOIS queries.

use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    fmt,
};

use anyhow::{Context, Result, anyhow, ensure};

use crate::{
    rpsl::{Object, lookup::SetMember},
    storage::Store,
};

pub(crate) const MAX_DEPTH: usize = 64;
const MAX_SET_LOOKUPS: usize = 1024;
const MAX_INDIRECT_ROWS: usize = 128;
const MAX_ORIGIN_LOOKUPS: usize = 4096;
const MAX_VISITED_VALUES: usize = 16_384;
const MAX_WORK_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESULTS: usize = 4096;
const MAX_RESULT_BYTES: usize = 256 * 1024;

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
    let Some(root) = find_set(store, sources, &name, false)? else {
        return Ok(Vec::new());
    };
    let mut expansion = Expansion::new(store, sources, excluded);
    if recursive {
        let mode = RecursiveMode::for_members(&root);
        expansion.expand(root, mode, None)?;
    } else {
        for member in direct_members(store, &root)? {
            let member = member.to_string();
            if member != name {
                expansion.insert(member)?;
            }
        }
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
) -> Result<Option<Vec<String>>> {
    let Some(root) = find_set(store, &[root_source.to_owned()], name, false)? else {
        return Ok(None);
    };
    let key = root.key.clone();
    let mode = RecursiveMode::for_members(&root);
    let mut expansion = Expansion::new(store, sources, excluded);
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
    let name = name.to_uppercase();
    let Some(root) = find_set(store, sources, &name, true)? else {
        return Ok(Vec::new());
    };
    let mut expansion = Expansion::new(store, sources, excluded);
    expansion.expand(root, RecursiveMode::OriginPrefixes(family), None)?;
    Ok(expansion.output.into_iter().collect())
}

fn find_set(store: &Store, sources: &[String], key: &str, as_only: bool) -> Result<Option<Object>> {
    let result = store.first("as-set", key, sources)?;
    if result.is_some() || as_only {
        return Ok(result);
    }
    store.first("route-set", key, sources)
}

fn direct_members(store: &Store, object: &Object) -> Result<Vec<SetMember>> {
    let lookup = object.lookup_attributes().map_err(|error| anyhow!(error))?;
    let mut members = lookup.members;
    members.extend(lookup.mp_members);
    if lookup.mbrs_by_ref.is_empty() {
        return Ok(members);
    }
    let any_maintainer = lookup.mbrs_by_ref.iter().any(|mnt| mnt == "ANY");
    let maintainers: HashSet<_> = lookup.mbrs_by_ref.iter().collect();
    let rows = store.inverse(
        "member-of",
        &object.key,
        std::slice::from_ref(&object.source),
        MAX_INDIRECT_ROWS + 1,
    )?;
    ensure!(
        rows.len() <= MAX_INDIRECT_ROWS,
        LimitExceeded("set exceeds indirect membership limit")
    );
    for row in rows {
        let applicable = match object.class.as_str() {
            "as-set" => row.class == "aut-num",
            "route-set" => matches!(row.class.as_str(), "route" | "route6"),
            _ => false,
        };
        if !applicable {
            continue;
        }
        if !any_maintainer {
            let lookup = row.lookup_attributes().map_err(|error| anyhow!(error))?;
            if !lookup.mnt_by.iter().any(|mnt| maintainers.contains(mnt)) {
                continue;
            }
        }
        let member = if row.class == "aut-num" {
            let number = row
                .key
                .strip_prefix("AS")
                .context("invalid aut-num identity in set membership")?
                .parse()
                .context("invalid ASN in set membership")?;
            SetMember::Asn(number, None)
        } else {
            let (prefix, _) = row
                .key
                .rsplit_once("AS")
                .context("invalid route identity in set membership")?;
            SetMember::Prefix(prefix.to_ascii_lowercase(), None)
        };
        members.push(member);
    }
    Ok(members)
}

#[derive(Clone, Copy)]
enum RecursiveMode {
    AsMembers,
    RouteMembers,
    OriginPrefixes(Option<bool>),
}

impl RecursiveMode {
    fn for_members(root: &Object) -> Self {
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
    output_bytes: usize,
    visited_values: usize,
    work_bytes: usize,
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
            output_bytes: 0,
            visited_values: 0,
            work_bytes: 0,
        }
    }

    fn expand(&mut self, root: Object, mode: RecursiveMode, limit: Option<usize>) -> Result<()> {
        let mut pending = VecDeque::new();
        self.seen_sets.insert(root.key.clone());
        self.expand_members(&root, mode, 0, limit == Some(1), &mut pending)?;
        while let Some((key, depth)) = pending.pop_front() {
            if !self.seen_sets.insert(key.clone()) {
                continue;
            }
            ensure!(
                depth <= MAX_DEPTH && self.seen_sets.len() <= MAX_SET_LOOKUPS,
                LimitExceeded("set exceeds traversal limit")
            );
            let as_only = !matches!(mode, RecursiveMode::RouteMembers);
            let cutoff = limit == Some(depth + 1);
            if let Some(object) = find_set(self.store, self.sources, &key, as_only)? {
                self.expand_members(&object, mode, depth, cutoff, &mut pending)?;
            } else if cutoff {
                self.insert(key)?;
            }
        }
        Ok(())
    }

    fn expand_members(
        &mut self,
        object: &Object,
        mode: RecursiveMode,
        depth: usize,
        cutoff: bool,
        pending: &mut VecDeque<(String, usize)>,
    ) -> Result<()> {
        for member in direct_members(self.store, object)? {
            let wire = member.to_string();
            self.charge(wire.len())?;
            if cutoff {
                self.insert(wire.clone())?;
            }
            match member {
                SetMember::Set(name, None) if !cutoff && !self.excluded.contains(&name) => {
                    pending.push_back((name, depth + 1));
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
        }
        Ok(())
    }

    fn expand_origin(&mut self, number: u32, family: Option<bool>) -> Result<()> {
        if !self.seen_origins.insert(number) {
            return Ok(());
        }
        ensure!(
            self.seen_origins.len() <= MAX_ORIGIN_LOOKUPS,
            LimitExceeded("set exceeds origin lookup limit")
        );
        let origin = format!("AS{number}");
        for (ipv6, class) in [(false, "route"), (true, "route6")] {
            if family.is_some_and(|value| value != ipv6) {
                continue;
            }
            let prefixes =
                self.store
                    .origin_prefixes(class, &origin, self.sources, MAX_RESULTS + 1)?;
            ensure!(
                prefixes.len() <= MAX_RESULTS,
                LimitExceeded("set exceeds prefix result limit")
            );
            for prefix in prefixes {
                self.charge(prefix.len())?;
                self.insert(prefix)?;
            }
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.visited_values += 1;
        self.work_bytes += bytes;
        ensure!(
            self.visited_values <= MAX_VISITED_VALUES && self.work_bytes <= MAX_WORK_BYTES,
            LimitExceeded("set exceeds expansion work limit")
        );
        Ok(())
    }

    fn insert(&mut self, value: String) -> Result<()> {
        if !self.output.contains(&value) {
            self.output_bytes += value.len() + 1;
            ensure!(
                self.output.len() < MAX_RESULTS && self.output_bytes <= MAX_RESULT_BYTES,
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
    use crate::rpsl::parse;

    fn fixture(store: &Store, source: &str, objects: &[&str]) -> Result<()> {
        store.replace_source(
            source,
            objects.iter().map(|text| {
                parse(&format!("{text}\nsource: {source}\n")).map_err(|error| anyhow!(error))
            }),
        )?;
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
        assert_eq!(error.to_string(), "set exceeds result limit");
        assert!(error.is::<LimitExceeded>());
        Ok(())
    }
}
