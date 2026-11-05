//! Query-only source aliases. Physical sources remain the storage/ACL identity.
use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, ensure};

pub(crate) type SourceAliases = BTreeMap<String, Vec<String>>;

pub(crate) fn validate(real: &[String], aliases: &SourceAliases) -> Result<()> {
    ensure!(
        aliases.len() <= 256,
        "at most 256 source aliases are allowed"
    );
    for (name, members) in aliases {
        ensure!(
            (2..=32).contains(&name.len())
                && name.as_bytes()[0].is_ascii_uppercase()
                && name
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-'),
            "invalid source alias name {name:?}"
        );
        ensure!(
            !real.contains(name),
            "source alias conflicts with a real source: {name}"
        );
        ensure!(members.len() <= 256, "too many sources in alias {name}");
        ensure!(
            members.iter().all(|source| real.contains(source)),
            "source alias {name} must contain only configured real sources; aliases cannot be nested"
        );
    }
    Ok(())
}

/// Hide an entire alias when any member is outside this interface's scope.
/// Partial expansion would silently change the operator's requested semantics.
pub(crate) fn visible(real: &[String], aliases: &SourceAliases) -> SourceAliases {
    aliases
        .iter()
        .filter(|(_, members)| members.iter().all(|source| real.contains(source)))
        .map(|(name, members)| (name.clone(), members.clone()))
        .collect()
}

pub(crate) fn available(real: &[String], aliases: &SourceAliases) -> Vec<String> {
    real.iter().chain(aliases.keys()).cloned().collect()
}

pub(crate) fn known(name: &str, real: &[String], aliases: &SourceAliases) -> bool {
    real.iter().any(|source| source == name) || aliases.contains_key(name)
}

/// Expand in caller order, retaining the first occurrence of each real source.
pub(crate) fn expand(
    requested: &[String],
    real: &[String],
    aliases: &SourceAliases,
) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for name in requested {
        if real.contains(name) {
            if seen.insert(name.clone()) {
                result.push(name.clone());
            }
        } else {
            for member in aliases.get(name)? {
                if !real.contains(member) {
                    return None;
                }
                if seen.insert(member.clone()) {
                    result.push(member.clone());
                }
            }
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_preserve_priority_deduplicate_and_cannot_cross_interface_scope() -> Result<()> {
        let real = vec!["FIRST".into(), "SECOND".into()];
        let aliases = SourceAliases::from([
            (
                "GROUP".into(),
                vec!["SECOND".into(), "FIRST".into(), "SECOND".into()],
            ),
            ("EMPTY".into(), vec![]),
        ]);
        validate(&real, &aliases)?;
        assert_eq!(
            expand(&["GROUP".into(), "FIRST".into()], &real, &aliases),
            Some(vec!["SECOND".into(), "FIRST".into()])
        );
        assert_eq!(
            expand(&["FIRST".into(), "GROUP".into()], &real, &aliases),
            Some(real.clone())
        );
        assert_eq!(expand(&["EMPTY".into()], &real, &aliases), Some(vec![]));
        assert!(expand(&["UNKNOWN".into()], &real, &aliases).is_none());
        let restricted = visible(&real[..1], &aliases);
        assert!(!restricted.contains_key("GROUP"));
        assert!(expand(&["GROUP".into()], &real[..1], &restricted).is_none());
        assert!(restricted.contains_key("EMPTY"));
        for (name, members) in [
            ("FIRST", vec!["SECOND"]),
            ("NESTED", vec!["GROUP"]),
            ("lower", vec!["FIRST"]),
            ("1BAD", vec!["FIRST"]),
            ("BAD-", vec!["FIRST"]),
            ("X", vec!["FIRST"]),
            ("UNKNOWN", vec!["MISSING"]),
        ] {
            assert!(
                validate(
                    &real,
                    &SourceAliases::from([(
                        name.into(),
                        members.into_iter().map(str::to_owned).collect()
                    )])
                )
                .is_err()
            );
        }
        Ok(())
    }
}
