use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub database: PathBuf,
    pub sources: Vec<String>,
    pub listen: SocketAddr,
    pub http_listen: Option<SocketAddr>,
    pub max_connections: usize,
    pub max_connections_per_ip: Option<usize>,
    #[serde(default)]
    pub connection_limit_exempt_ips: HashSet<IpAddr>,
    #[serde(default)]
    pub nrtm3_serve: HashMap<String, HashSet<IpAddr>>,
    pub io_timeout_seconds: u64,
    pub max_snapshot_bytes: u64,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Self = toml::from_str(&contents).context("invalid configuration")?;
        ensure!(
            !config.database.as_os_str().is_empty(),
            "database path is empty"
        );
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
        if config.database.is_relative() {
            config.database = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&config.database);
        }
        Ok(config)
    }
}
