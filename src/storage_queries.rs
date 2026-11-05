//! Bounded intersections over existing inverse indexes, without decoding rejected rows.
use super::*;

pub(crate) struct InverseFilter {
    pub field: &'static str,
    pub values: Vec<String>,
}

const MAX_FILTER_CANDIDATES: usize = 4096;
// Includes selectivity probes, seeks and predicate point reads across every
// selected source/filter. This is shared by the entire intersection.
const MAX_FILTER_WORK: usize = 131_072;

fn charge_work(remaining: &std::cell::Cell<usize>) -> Result<()> {
    check_query_deadline()?;
    let next = remaining.get().checked_sub(1).ok_or(QueryScanLimit)?;
    remaining.set(next);
    Ok(())
}

#[derive(Debug)]
pub(crate) struct QueryScanLimit;
impl std::fmt::Display for QueryScanLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("indexed query scan budget exceeded")
    }
}
impl std::error::Error for QueryScanLimit {}

impl Store {
    /// OR within each field, AND across fields. A restrictive intersection can
    /// exhaust the scan budget; return an error rather than a partial answer.
    /// All predicates and result reads share one committed database snapshot.
    pub(crate) fn filtered_objects(
        &self,
        keys: Option<&[String]>,
        sources: &[String],
        classes: &[String],
        filters: &[InverseFilter],
        limit: usize,
    ) -> Result<Vec<Object>> {
        if limit == 0
            || sources.is_empty()
            || keys.is_some_and(|k| k.is_empty())
            || filters.iter().any(|filter| filter.values.is_empty())
        {
            return Ok(Vec::new());
        }
        ensure!(!filters.is_empty(), "missing indexed query filter");
        let read = self.read_transaction()?;
        let records = read.open_table(OBJECTS)?;
        let inverse = read.open_table(INVERSE)?;
        let origins = read.open_multimap_table(ORIGINS)?;
        let work = std::cell::Cell::new(MAX_FILTER_WORK);
        // Visit only index identities. Probes never decode object text, and
        // stop as soon as the current best cardinality has been exceeded.
        let visit = |filter: &InverseFilter,
                     visitor: &mut dyn FnMut(&str, &str, &str) -> Result<bool>|
         -> Result<()> {
            for source in sources {
                for value in &filter.values {
                    if filter.field == "origin" {
                        for class in ["route", "route6"] {
                            if !classes.is_empty() && !classes.iter().any(|c| c == class) {
                                continue;
                            }
                            charge_work(&work)?;
                            for prefix in origins.get((source.as_str(), class, value.as_str()))? {
                                charge_work(&work)?;
                                let key = format!("{}{value}", prefix?.value());
                                if visitor(source, class, &key)? {
                                    return Ok(());
                                }
                            }
                        }
                    } else {
                        let search_classes: Vec<&str> = if classes.is_empty() {
                            vec![""]
                        } else {
                            classes.iter().map(String::as_str).collect()
                        };
                        for class in search_classes {
                            charge_work(&work)?;
                            let end_value = format!("{value}\0");
                            let end_class = format!("{class}\0");
                            let lower = (source.as_str(), filter.field, value.as_str(), class, "");
                            let upper = if class.is_empty() {
                                (source.as_str(), filter.field, end_value.as_str(), "", "")
                            } else {
                                (
                                    source.as_str(),
                                    filter.field,
                                    value.as_str(),
                                    end_class.as_str(),
                                    "",
                                )
                            };
                            for entry in inverse.range(lower..upper)? {
                                charge_work(&work)?;
                                let (identity, _) = entry?;
                                let (source, _, _, class, key) = identity.value();
                                if visitor(source, class, key)? {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        };
        let mut output = Vec::new();
        let mut remaining_bytes = MAX_QUERY_OBJECT_BYTES;
        let mut seen = HashSet::new();
        let mut examined = 0;
        let mut consider = |source: &str, class: &str, key: &str| -> Result<bool> {
            charge_work(&work)?;
            examined += 1;
            if examined > MAX_FILTER_CANDIDATES {
                return Err(QueryScanLimit.into());
            }
            if !classes.is_empty() && !classes.iter().any(|c| c == class) {
                return Ok(false);
            }
            for filter in filters {
                let mut matched = false;
                for value in &filter.values {
                    charge_work(&work)?;
                    // Canonical route keys encode the origin after the IP
                    // prefix; preserve full source/class/key identity.
                    let matches = if filter.field == "origin" {
                        matches!(class, "route" | "route6")
                            && key
                                .rsplit_once("AS")
                                .is_some_and(|(_, asn)| value.strip_prefix("AS") == Some(asn))
                    } else {
                        inverse
                            .get((source, filter.field, value.as_str(), class, key))?
                            .is_some()
                    };
                    if matches {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    return Ok(false);
                }
            }
            if seen.insert((source.to_owned(), class.to_owned(), key.to_owned())) {
                output.push(
                    read_query_object(&records, source, class, key, &mut remaining_bytes)?
                        .context("inverse query index points to a missing object")?,
                );
            }
            Ok(output.len() == limit)
        };
        if let Some(keys) = keys {
            let exact = read.open_multimap_table(EXACT)?;
            let mut unique_keys = HashSet::new();
            for key in keys {
                charge_work(&work)?;
                let key = key.to_uppercase();
                if !unique_keys.insert(key.clone()) {
                    continue;
                }
                for source in sources {
                    charge_work(&work)?;
                    for class in exact.get((source.as_str(), key.as_str()))? {
                        if consider(source, class?.value(), &key)? {
                            return Ok(output);
                        }
                    }
                }
            }
        } else {
            let mut driver = &filters[0];
            let mut best = MAX_FILTER_CANDIDATES + 1;
            if filters.len() > 1 {
                for filter in filters {
                    let mut count = 0;
                    visit(filter, &mut |_, _, _| {
                        count += 1;
                        Ok(count >= best)
                    })?;
                    if count < best {
                        best = count;
                        driver = filter;
                    }
                    if count == 0 {
                        return Ok(output);
                    }
                }
            }
            visit(driver, &mut consider)?;
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(field: &'static str, values: &[&str]) -> InverseFilter {
        InverseFilter {
            field,
            values: values.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn indexed_intersection_filters_before_limit_and_deduplicates() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("query.redb"))?;
        for source in ["TEST", "OTHER"] {
            store.replace_source(
                source,
                (0..20).map(|n| {
                    crate::rpsl::parse(&format!(
                        "aut-num: AS{}\nmnt-by: MNT-A, MNT-B\nadmin-c: {}\nsource: {source}\n",
                        64496 + n,
                        if n == 19 { "MATCH-TEST" } else { "OTHER-TEST" }
                    ))
                    .map_err(anyhow::Error::msg)
                }),
            )?;
        }
        let filters = [
            filter("mnt-by", &["MNT-A", "MNT-B"]),
            filter("admin-c", &["MATCH-TEST"]),
        ];
        let rows =
            store.filtered_objects(None, &["OTHER".into(), "TEST".into()], &[], &filters, 8)?;
        assert_eq!(
            rows.iter()
                .map(|r| (r.source.as_str(), r.key.as_str()))
                .collect::<Vec<_>>(),
            [("OTHER", "AS64515"), ("TEST", "AS64515")]
        );
        assert_eq!(
            store
                .filtered_objects(None, &["TEST".into()], &[], &filters, 1)?
                .len(),
            1
        );
        assert!(
            store
                .filtered_objects(None, &["TEST".into()], &["route".into()], &filters, 8)?
                .is_empty()
        );
        assert!(
            store
                .filtered_objects(
                    Some(&["AS64496".into()]),
                    &["TEST".into()],
                    &[],
                    &filters,
                    8
                )?
                .is_empty()
        );
        assert!(
            store
                .filtered_objects(None, &["TEST".into()], &[], &[filter("mnt-by", &[])], 8)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn origin_intersections_preserve_source_class_and_check_worker_cancellation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("origins.redb"))?;
        for source in ["FIRST", "SECOND"] {
            store.replace_source(
                source,
                [
                    format!(
                        "route: 192.0.2.0/24\norigin: AS64496\nmnt-by: COMMON\nsource: {source}\n"
                    ),
                    format!(
                        "route6: 2001:db8::/32\norigin: AS64496\nmnt-by: COMMON\nsource: {source}\n"
                    ),
                    format!(
                        "route: 192.0.2.0/24\norigin: AS64497\nmnt-by: OTHER\nsource: {source}\n"
                    ),
                ]
                .into_iter()
                .map(|text| crate::rpsl::parse(&text).map_err(anyhow::Error::msg)),
            )?;
        }
        let sources = vec!["SECOND".into(), "FIRST".into()];
        let filters = [
            filter("origin", &["AS64496", "AS64496"]),
            filter("mnt-by", &["COMMON"]),
        ];
        let rows = store.filtered_objects(None, &sources, &[], &filters, 8)?;
        assert_eq!(
            rows.iter()
                .map(|row| (row.source.as_str(), row.class.as_str(), row.key.as_str()))
                .collect::<Vec<_>>(),
            [
                ("SECOND", "route", "192.0.2.0/24AS64496"),
                ("SECOND", "route6", "2001:DB8::/32AS64496"),
                ("FIRST", "route", "192.0.2.0/24AS64496"),
                ("FIRST", "route6", "2001:DB8::/32AS64496")
            ]
        );
        assert_eq!(
            store
                .filtered_objects(None, &sources, &[], &filters[..1], 8)?
                .len(),
            4
        );
        assert_eq!(
            store
                .filtered_objects(None, &sources, &["route6".into()], &filters, 8)?
                .len(),
            2
        );
        assert!(
            store
                .filtered_objects(
                    Some(&["192.0.2.0/24AS64497".into()]),
                    &sources,
                    &[],
                    &filters,
                    8
                )?
                .is_empty()
        );
        // Cancellation is checked on every probe/seek/predicate operation,
        // including after a worker has already captured its snapshot.
        let view = store.query_view()?;
        let control = QueryControl::new(Duration::from_secs(5));
        let guard = control.cancel_on_drop();
        view.run(|| {
            control.run(|| {
                let remaining = std::cell::Cell::new(3);
                charge_work(&remaining)?;
                drop(guard);
                assert!(
                    charge_work(&remaining)
                        .unwrap_err()
                        .is::<QueryDeadlineExceeded>()
                );
                assert!(
                    store
                        .filtered_objects(None, &sources, &[], &filters, 8)
                        .unwrap_err()
                        .is::<QueryDeadlineExceeded>()
                );
                Ok::<_, anyhow::Error>(())
            })
        })?;
        assert!(check_query_deadline().is_ok());
        let remaining = std::cell::Cell::new(1);
        charge_work(&remaining)?;
        assert!(charge_work(&remaining).unwrap_err().is::<QueryScanLimit>());
        Ok(())
    }

    #[test]
    fn cardinality_probe_finds_rare_match_and_broad_scan_stays_bounded() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("bounded.redb"))?;
        store.replace_source(
            "TEST",
            (0..MAX_FILTER_CANDIDATES + 1).map(|n| {
                crate::rpsl::parse(&format!(
                    "aut-num: AS{}\nmnt-by: MNT-A\nadmin-c: {}\nsource: TEST\n",
                    64496 + n,
                    if n == 0 { "MATCH-TEST" } else { "OTHER-TEST" }
                ))
                .map_err(anyhow::Error::msg)
            }),
        )?;
        let filters = [
            filter("mnt-by", &["MNT-A"]),
            filter("admin-c", &["MATCH-TEST"]),
        ];
        assert_eq!(
            store
                .filtered_objects(None, &["TEST".into()], &[], &filters, 8)?
                .len(),
            1
        );
        // A single broad predicate still fails rather than yielding partial
        // output when an internal caller asks beyond the finite scan cap.
        assert!(
            store
                .filtered_objects(
                    None,
                    &["TEST".into()],
                    &[],
                    &filters[..1],
                    MAX_FILTER_CANDIDATES + 2
                )
                .unwrap_err()
                .is::<QueryScanLimit>()
        );
        assert_eq!(
            store
                .filtered_objects(None, &["TEST".into()], &[], &filters, 1)?
                .len(),
            1
        );
        Ok(())
    }
}
