//! Bounded intersections over existing inverse indexes, without decoding rejected rows.
use super::*;

pub(crate) struct InverseFilter {
    pub field: &'static str,
    pub values: Vec<String>,
}

const MAX_FILTER_CANDIDATES: usize = 4096;

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
        let read = self.db.begin_read()?;
        let records = read.open_table(OBJECTS)?;
        let inverse = read.open_table(INVERSE)?;
        let mut output = Vec::new();
        let mut remaining_bytes = MAX_QUERY_OBJECT_BYTES;
        let mut seen = HashSet::new();
        let mut examined = 0;
        let mut consider = |source: &str, class: &str, key: &str| -> Result<bool> {
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
                    if inverse
                        .get((source, filter.field, value.as_str(), class, key))?
                        .is_some()
                    {
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
                let key = key.to_uppercase();
                if !unique_keys.insert(key.clone()) {
                    continue;
                }
                for source in sources {
                    for class in exact.get((source.as_str(), key.as_str()))? {
                        if consider(source, class?.value(), &key)? {
                            return Ok(output);
                        }
                    }
                }
            }
        } else {
            let driver = filters
                .iter()
                .min_by_key(|filter| filter.values.len())
                .unwrap();
            for source in sources {
                for value in &driver.values {
                    // The class restriction is part of the seek: unrelated
                    // classes do not consume the limited candidate budget.
                    let search_classes: Vec<&str> = if classes.is_empty() {
                        vec![""]
                    } else {
                        classes.iter().map(String::as_str).collect()
                    };
                    for class in search_classes {
                        let end_value = format!("{value}\0");
                        let end_class = format!("{class}\0");
                        let lower = (source.as_str(), driver.field, value.as_str(), class, "");
                        let upper = if class.is_empty() {
                            (source.as_str(), driver.field, end_value.as_str(), "", "")
                        } else {
                            (
                                source.as_str(),
                                driver.field,
                                value.as_str(),
                                end_class.as_str(),
                                "",
                            )
                        };
                        for entry in inverse.range(lower..upper)? {
                            let (identity, _) = entry?;
                            let (source, _, _, class, key) = identity.value();
                            if consider(source, class, key)? {
                                return Ok(output);
                            }
                        }
                    }
                }
            }
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
    fn bounded_intersection_refuses_partial_results() -> Result<()> {
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
        assert!(
            store
                .filtered_objects(None, &["TEST".into()], &[], &filters, 8)
                .unwrap_err()
                .to_string()
                .contains("scan budget")
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
