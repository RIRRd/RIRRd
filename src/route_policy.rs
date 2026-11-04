//! Admission policy for new and modified authoritative route objects.

use std::collections::HashSet;

use anyhow::{Result, ensure};

use crate::{config::Config, scope::Scope};

#[derive(Default)]
pub struct RoutePolicy {
    scope: Option<Scope>,
    scope_exempt: HashSet<String>,
}

impl RoutePolicy {
    pub fn load(config: &Config) -> Result<Self> {
        let (scope, scope_exempt) = match &config.authoritative_scope {
            Some(config) => (
                Some(Scope::compile(&config.prefixes, &config.asns)?),
                config.exempt_sources.clone(),
            ),
            None => (None, HashSet::new()),
        };
        Ok(Self {
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
        Ok(())
    }
}
