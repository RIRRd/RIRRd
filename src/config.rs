use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const DEFAULT_DATABASE_CACHE_MIB: usize = 64;

fn default_database_cache_mib() -> usize {
    DEFAULT_DATABASE_CACHE_MIB
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub node: NodeConfig,
    pub database: PathBuf,
    #[serde(default = "default_database_cache_mib")]
    pub database_cache_mib: usize,
    pub worker_threads: Option<usize>,
    pub max_blocking_threads: Option<usize>,
    pub sources: Vec<String>,
    /// Optional HTTP query subset for source-specific distribution policies.
    pub http_sources: Option<Vec<String>>,
    /// Optional raw HTTP WHOIS service scope, independently of GraphQL exports.
    pub http_whois_sources: Option<Vec<String>>,
    #[serde(default)]
    pub authoritative_sources: HashSet<String>,
    #[serde(default)]
    pub legacy_md5_sources: HashSet<String>,
    pub authoritative_scope: Option<ScopeConfig>,
    pub listen: SocketAddr,
    pub http_listen: Option<SocketAddr>,
    /// Public reverse-proxy admission; the native listener remains loopback-only.
    #[serde(default)]
    pub http_public_queries: bool,
    #[serde(default)]
    pub http_authoritative_updates: bool,
    pub max_connections: usize,
    pub max_connections_per_ip: Option<usize>,
    #[serde(default)]
    pub connection_limit_exempt_ips: HashSet<IpAddr>,
    #[serde(default)]
    pub nrtm3_serve: HashMap<String, HashSet<IpAddr>>,
    #[serde(default)]
    pub nrtm3_sync: HashMap<String, MirrorConfig>,
    pub io_timeout_seconds: u64,
    pub max_snapshot_bytes: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    #[serde(default = "default_node_name")]
    pub name: String,
    pub location: Option<String>,
    pub sponsor: Option<String>,
}
fn default_node_name() -> String {
    "Local RIRRd".into()
}
impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: default_node_name(),
            location: None,
            sponsor: None,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorConfig {
    pub endpoint: SocketAddr,
    pub interval_seconds: u64,
    pub base_serial: Option<u64>,
    /// Retain these classes while consuming every upstream serial.
    #[serde(default)]
    pub object_class_filter: HashSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeConfig {
    #[serde(default)]
    pub prefixes: Vec<String>,
    #[serde(default)]
    pub asns: Vec<String>,
    #[serde(default)]
    pub exempt_sources: HashSet<String>,
}

impl Config {
    pub fn database_cache_bytes(&self) -> usize {
        self.database_cache_mib * 1024 * 1024
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Self = toml::from_str(&contents).context("invalid configuration")?;
        if let Some(sources) = &config.http_whois_sources {
            ensure!(
                !sources.is_empty() && sources.iter().all(|s| config.sources.contains(s)),
                "HTTP WHOIS sources must be configured sources"
            );
        }
        for label in std::iter::once(&config.node.name)
            .chain(config.node.location.iter())
            .chain(config.node.sponsor.iter())
        {
            ensure!(
                !label.trim().is_empty()
                    && label.len() <= 128
                    && !label.chars().any(char::is_control),
                "node labels must contain 1–128 bytes without control characters"
            );
        }
        ensure!(
            !config.database.as_os_str().is_empty(),
            "database path is empty"
        );
        ensure!(
            (1..=65536).contains(&config.database_cache_mib),
            "database_cache_mib must be between 1 and 65536"
        );
        if let Some(threads) = config.worker_threads {
            ensure!(
                (1..=64).contains(&threads),
                "worker_threads must be between 1 and 64"
            );
        }
        if let Some(threads) = config.max_blocking_threads {
            ensure!(
                (1..=256).contains(&threads),
                "max_blocking_threads must be between 1 and 256"
            );
        }
        if let Some(http) = config.http_listen {
            ensure!(
                http.ip().to_canonical().is_loopback(),
                "http_listen must use a loopback address"
            );
            ensure!(
                http.port() == 0 || http != config.listen,
                "WHOIS and HTTP must use distinct addresses"
            );
        }
        ensure!(
            (1..=256).contains(&config.sources.len()),
            "between 1 and 256 sources are required"
        );
        let mut seen = HashSet::new();
        for source in &config.sources {
            ensure!(
                source.len() <= 32
                    && crate::rpsl::generic_name(source)
                    && !source.bytes().any(|byte| byte.is_ascii_lowercase()),
                "invalid configured source {source:?}"
            );
            ensure!(seen.insert(source), "duplicate configured source {source}");
        }
        if let Some(sources) = &config.http_sources {
            let unique: HashSet<_> = sources.iter().collect();
            ensure!(
                !sources.is_empty()
                    && unique.len() == sources.len()
                    && sources.iter().all(|source| seen.contains(source)),
                "http_sources must be a nonempty unique subset of configured sources"
            );
        }
        ensure!(
            config
                .authoritative_sources
                .iter()
                .all(|source| seen.contains(source)),
            "authoritative_sources must be configured in sources"
        );
        ensure!(
            !config.http_public_queries
                || (config.http_listen.is_some() && !config.http_authoritative_updates),
            "http_public_queries requires a loopback HTTP listener and disabled HTTP updates"
        );
        ensure!(
            !config.http_authoritative_updates
                || (config.http_listen.is_some() && !config.authoritative_sources.is_empty()),
            "HTTP updates require http_listen and at least one authoritative source"
        );
        ensure!(
            config
                .legacy_md5_sources
                .is_subset(&config.authoritative_sources),
            "legacy_md5_sources must be authoritative"
        );
        if let Some(scope) = &config.authoritative_scope {
            ensure!(
                scope
                    .exempt_sources
                    .is_subset(&config.authoritative_sources),
                "scope exemptions must be authoritative sources"
            );
        }
        for (source, peers) in &mut config.nrtm3_serve {
            ensure!(
                seen.contains(source),
                "unknown NRTMv3 export source {source}"
            );
            ensure!(
                (1..=256).contains(&peers.len()),
                "NRTMv3 export requires 1–256 peers per source"
            );
            *peers = std::mem::take(peers)
                .into_iter()
                .map(|address| address.to_canonical())
                .collect();
        }
        ensure!(
            config.nrtm3_sync.len() <= 16,
            "at most 16 scheduled mirrors are supported"
        );
        for (source, mirror) in &config.nrtm3_sync {
            ensure!(
                seen.contains(source) && !config.authoritative_sources.contains(source),
                "scheduled NRTMv3 source must be configured and non-authoritative"
            );
            ensure!(
                !mirror.endpoint.ip().is_unspecified() && mirror.endpoint.port() != 0,
                "NRTMv3 mirror endpoint must specify a peer IP and port"
            );
            ensure!(
                (1..=86_400).contains(&mirror.interval_seconds),
                "NRTMv3 mirror interval must be between 1 and 86400 seconds"
            );
            ensure!(
                mirror.object_class_filter.len() <= 22
                    && mirror
                        .object_class_filter
                        .iter()
                        .all(|class| crate::rpsl::supported_class(class)),
                "invalid NRTMv3 object class filter"
            );
            ensure!(
                mirror.object_class_filter.is_empty() || !config.nrtm3_serve.contains_key(source),
                "a filtered NRTMv3 mirror cannot serve a complete upstream journal"
            );
        }
        ensure!(
            (1..=4096).contains(&config.max_connections),
            "max_connections must be between 1 and 4096"
        );
        if let Some(limit) = config.max_connections_per_ip {
            ensure!(
                (1..=config.max_connections).contains(&limit),
                "max_connections_per_ip must be between 1 and max_connections"
            );
        }
        ensure!(
            config.connection_limit_exempt_ips.len() <= 256,
            "connection_limit_exempt_ips supports at most 256 addresses"
        );
        config.connection_limit_exempt_ips = config
            .connection_limit_exempt_ips
            .into_iter()
            .map(|address| address.to_canonical())
            .collect();
        ensure!(
            (1..=300).contains(&config.io_timeout_seconds),
            "io_timeout_seconds must be between 1 and 300"
        );
        ensure!(
            (1..=64 * 1024 * 1024 * 1024).contains(&config.max_snapshot_bytes),
            "max_snapshot_bytes must be between 1 byte and 64 GiB"
        );
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        if config.database.is_relative() {
            config.database = directory.join(&config.database);
        }
        Ok(config)
    }
}
