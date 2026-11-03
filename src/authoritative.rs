//! Authorization for explicitly enabled authoritative sources.

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};

use crate::{
    authoritative_validation::{contacts, maintainers, set_creation_asn, validate},
    maintainer_auth::{AuthBudget, authenticates, authenticates_existing, validate_new},
    rpsl::Object,
};

pub const MAX_CHANGES: usize = 32;

pub enum Change {
    Upsert(Object),
    Delete(Object),
}

impl Change {
    pub fn object(&self) -> &Object {
        match self {
            Self::Upsert(object) | Self::Delete(object) => object,
        }
    }
}

pub fn authorize(
    source: &str,
    changes: &[Change],
    passwords: &[String],
    existing: impl Fn(&str, &str) -> Result<Option<Object>>,
    referenced: impl Fn(&str, &str) -> Result<bool>,
    allocation_parent: impl Fn(&Object) -> Result<Option<Object>>,
    legacy_md5: bool,
) -> Result<()> {
    ensure!(
        (1..=MAX_CHANGES).contains(&changes.len()),
        "invalid number of authoritative changes"
    );
    let mut pending = HashMap::new();
    for change in changes {
        let object = change.object();
        ensure!(
            object.source == source,
            "authoritative object source mismatch"
        );
        ensure!(
            matches!(
                object.class.as_str(),
                "person"
                    | "role"
                    | "mntner"
                    | "route"
                    | "route6"
                    | "as-set"
                    | "route-set"
                    | "aut-num"
            ),
            "unsupported authoritative object class"
        );
        ensure!(
            pending
                .insert((object.class.as_str(), object.key.as_str()), change)
                .is_none(),
            "duplicate authoritative object identity in submission"
        );
        if matches!(change, Change::Upsert(_)) {
            validate(object)?;
            if object.class == "mntner" {
                validate_new(object)?;
            }
        }
    }
    let exists_after = |class: &str, key: &str| -> Result<bool> {
        match pending.get(&(class, key)) {
            Some(Change::Upsert(_)) => Ok(true),
            Some(Change::Delete(_)) => Ok(false),
            None => Ok(existing(class, key)?.is_some()),
        }
    };
    let mut budget = AuthBudget::default();
    let mut authenticated = HashMap::new();
    for change in changes {
        let object = change.object();
        let previous = existing(&object.class, &object.key)?;
        if let Some(previous) = &previous {
            ensure!(
                any_maintainer(
                    previous,
                    &existing,
                    passwords,
                    &mut budget,
                    &mut authenticated,
                    legacy_md5,
                )?,
                "maintainer authentication failed"
            );
        }
        if matches!(change, Change::Delete(_)) {
            ensure!(previous.is_some(), "object to delete does not exist");
            ensure!(
                !referenced(&object.class, &object.key)?,
                "object still has strong references"
            );
            continue;
        }

        if object.class == "mntner" {
            ensure!(
                previous.is_some(),
                "new maintainers require operator provisioning"
            );
            ensure!(
                authenticates(object, passwords, &mut budget)?,
                "proposed maintainer authentication failed"
            );
        }
        ensure!(
            any_maintainer(
                object,
                &existing,
                passwords,
                &mut budget,
                &mut authenticated,
                legacy_md5,
            )?,
            "maintainer authentication failed"
        );
        let parent = if previous.is_some() {
            None
        } else {
            match object.class.as_str() {
                "route" | "route6" => allocation_parent(object)?,
                "aut-num" => Some(
                    allocation_parent(object)?.context("no covering AS-block in this source")?,
                ),
                "as-set" | "route-set" => {
                    let asn = set_creation_asn(object)?;
                    existing("aut-num", &format!("AS{asn}"))?
                }
                _ => None,
            }
        };
        if let Some(parent) = parent {
            ensure!(
                exists_after(&parent.class, &parent.key)?,
                "parent object would be deleted by this submission"
            );
            ensure!(
                any_maintainer(
                    &parent,
                    &existing,
                    passwords,
                    &mut budget,
                    &mut authenticated,
                    legacy_md5,
                )?,
                "parent maintainer authentication failed"
            );
        }
        for maintainer in maintainers(object)? {
            ensure!(
                exists_after("mntner", &maintainer)?,
                "referenced maintainer does not exist"
            );
        }
        for contact in contacts(object)? {
            ensure!(
                exists_after("person", &contact)? || exists_after("role", &contact)?,
                "referenced contact does not exist"
            );
        }
    }
    Ok(())
}

fn any_maintainer(
    object: &Object,
    lookup: impl Fn(&str, &str) -> Result<Option<Object>>,
    passwords: &[String],
    budget: &mut AuthBudget,
    authenticated: &mut HashMap<String, bool>,
    legacy_md5: bool,
) -> Result<bool> {
    for key in maintainers(object).context("invalid maintainer references")? {
        let Some(maintainer) = lookup("mntner", &key)? else {
            continue;
        };
        let passed = match authenticated.get(&key) {
            Some(passed) => *passed,
            None => {
                let passed = authenticates_existing(&maintainer, passwords, budget, legacy_md5)?;
                authenticated.insert(key, passed);
                passed
            }
        };
        if passed {
            return Ok(true);
        }
    }
    Ok(false)
}
