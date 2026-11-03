//! Admission policy for new and modified authoritative route objects.

use std::{collections::HashSet, fs::File};

use anyhow::{Context, Result, ensure};

use crate::{
    config::Config,
    rpki::{RoaTable, Validation},
    scope::Scope,
};

#[derive(Default)]
pub struct RoutePolicy {
    roas: Option<RoaTable>,
    roa_exempt: HashSet<String>,
    scope: Option<Scope>,
    scope_exempt: HashSet<String>,
}

impl RoutePolicy {
    pub fn load(config: &Config) -> Result<Self> {
        let roas = config
            .authoritative_roa_file
            .as_ref()
            .map(|path| -> Result<RoaTable> {
                let file = File::open(path).context("opening authoritative ROA policy file")?;
                let table = RoaTable::read(file, config.max_snapshot_bytes.min(256 * 1024 * 1024))?;
                ensure!(!table.is_empty(), "authoritative ROA policy file is empty");
                Ok(table)
            })
            .transpose()?;
        let (scope, scope_exempt) = match &config.authoritative_scope {
            Some(config) => (
                Some(Scope::compile(&config.prefixes, &config.asns)?),
                config.exempt_sources.clone(),
            ),
            None => (None, HashSet::new()),
        };
        Ok(Self {
            roas,
            roa_exempt: config.authoritative_roa_exempt_sources.clone(),
            scope,
            scope_exempt,
        })
    }

    pub fn validate(&self, source: &str, prefix: &str, origin: u32) -> Result<()> {
        if let Some(scope) = &self.scope
            && !self.scope_exempt.contains(source)
        {
            ensure!(
                !scope.rejects_route(prefix, origin)?,
                "route is outside the authoritative scope policy"
            );
        }
        if let Some(roas) = &self.roas
            && !self.roa_exempt.contains(source)
        {
            ensure!(
                roas.validate(prefix, origin)? != Validation::Invalid,
                "route origin is invalid under the authoritative ROA policy"
            );
        }
        Ok(())
    }
}
