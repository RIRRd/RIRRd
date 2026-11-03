#![forbid(unsafe_code)]

mod authoritative;
mod authoritative_validation;
mod backup;
mod config;
mod dashboard;
mod graphql;
mod import;
mod legacy_md5;
mod maintainer_auth;
mod mirror_worker;
mod nrtm3;
mod nrtm4;
mod nrtm4_apply;
mod nrtm4_local;
mod nrtm4_records;
mod nrtm4_remote;
mod nrtm4_state;
mod nrtm_client;
mod route_policy;
mod rpki;
mod rpsl;
mod scope;
mod sets;
mod storage;
mod submission_http;
mod whois;

use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, BufWriter, IsTerminal, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use tokio::{net::TcpListener, sync::watch, task::JoinSet};
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

use crate::{config::Config, route_policy::RoutePolicy, storage::Store};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long, global = true, default_value = "irrd.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Server,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    Database {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    Rpki {
        #[command(subcommand)]
        command: RpkiCommand,
    },
    /// Apply an offline, maintainer-authenticated RPSL submission.
    Update {
        #[arg(long)]
        source: String,
        #[arg(long)]
        file: PathBuf,
        /// Treat every record in the file as a deletion.
        #[arg(long)]
        delete: bool,
    },
}

#[derive(Subcommand)]
enum RpkiCommand {
    /// Classify one route using a validator-produced ROA JSON file.
    Validate {
        #[arg(long)]
        roas: PathBuf,
        #[arg(long)]
        prefix: String,
        /// Route origin as a decimal ASN.
        #[arg(long)]
        origin: u32,
    },
}

#[derive(Subcommand)]
enum SourceCommand {
    /// Apply a signed NRTMv4 snapshot or delta chain from local files.
    ApplyNrtm4 {
        #[arg(long)]
        source: String,
        #[arg(long)]
        notification: PathBuf,
        /// Independently trusted P-256 public key, required for the first apply.
        #[arg(long)]
        public_key: Option<PathBuf>,
        /// Supply each selected file in snapshot-then-delta order.
        #[arg(long = "file")]
        files: Vec<PathBuf>,
    },
    /// Fetch and apply a signed NRTMv4 update from a trusted HTTPS endpoint.
    SyncNrtm4 {
        #[arg(long)]
        source: String,
        #[arg(long)]
        notification_url: String,
        /// Independently trusted P-256 public key, required for the first sync.
        #[arg(long)]
        public_key: Option<PathBuf>,
        /// Overall download budget in seconds.
        #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,
    },
    /// Read committed authoritative changes, newest first.
    History {
        #[arg(long)]
        source: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Verify a signed NRTMv4 notification and optionally a referenced local file.
    VerifyNrtm4 {
        #[arg(long)]
        source: String,
        #[arg(long)]
        notification: PathBuf,
        #[arg(long)]
        public_key: PathBuf,
        #[arg(long, conflicts_with = "delta")]
        snapshot: Option<PathBuf>,
        #[arg(long, requires = "delta_version")]
        delta: Option<PathBuf>,
        #[arg(long, requires = "delta")]
        delta_version: Option<u64>,
    },
    /// Report the last committed incremental serial for one source.
    Status {
        #[arg(long)]
        source: String,
    },
    /// Apply a local NRTMv3 transcript to an offline registry.
    ApplyNrtm3 {
        #[arg(long)]
        source: String,
        #[arg(long)]
        file: PathBuf,
        /// Serial of the imported snapshot, required on the first apply.
        #[arg(long)]
        base_serial: Option<u64>,
    },
    /// Fetch and apply an unfiltered NRTMv3 update from a trusted endpoint.
    SyncNrtm3 {
        #[arg(long)]
        source: String,
        #[arg(long)]
        endpoint: SocketAddr,
        /// Serial of the imported snapshot, required on the first sync.
        #[arg(long)]
        base_serial: Option<u64>,
        /// Network-only deadline, separate from local indexing time.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=300))]
        timeout_seconds: Option<u64>,
    },
    /// Remove one source's stored objects and indexes.
    Purge {
        #[arg(long)]
        source: String,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ExportCommand {
    Snapshot {
        #[arg(long)]
        source: String,
        #[arg(long)]
        file: PathBuf,
        /// Acknowledge that the raw snapshot may contain maintainer credentials.
        #[arg(long, required_unless_present = "public", conflicts_with = "public")]
        include_sensitive: bool,
        /// Export redacted objects and report the matching NRTMv3 base serial.
        #[arg(long)]
        public: bool,
    },
}

#[derive(Subcommand)]
enum DatabaseCommand {
    Verify,
    /// Read the committed object count; this does not verify index integrity.
    Count,
    /// Write a private, versioned backup of the offline database.
    Backup {
        #[arg(long)]
        file: PathBuf,
    },
    /// Restore a verified backup into a new database path.
    Restore {
        #[arg(long)]
        file: PathBuf,
    },
    /// Reclaim unused space in the offline registry.
    Compact {
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Check,
}

#[derive(Subcommand)]
enum ImportCommand {
    Snapshot {
        #[arg(long)]
        source: String,
        #[arg(long)]
        file: PathBuf,
        /// Acknowledge replacement of an authoritative source by a trusted operator.
        #[arg(long)]
        authoritative_restore: bool,
        /// Last serial published elsewhere, for rebuilding an older authoritative copy.
        #[arg(long, requires = "authoritative_restore")]
        serial_floor: Option<u64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Command::Source {
            command:
                SourceCommand::ApplyNrtm4 {
                    source,
                    notification,
                    public_key,
                    files,
                },
        } => {
            let store = open_nrtm4_source(&config, &source)?;
            let result = nrtm4_apply::apply(
                &store,
                &source,
                &notification,
                public_key.as_deref(),
                &files,
                config.max_snapshot_bytes,
            )?;
            println!("{result}");
        }
        Command::Source {
            command:
                SourceCommand::SyncNrtm4 {
                    source,
                    notification_url,
                    public_key,
                    timeout_seconds,
                },
        } => {
            let store = open_nrtm4_source(&config, &source)?;
            let public_key = public_key
                .as_deref()
                .map(|path| nrtm4_local::read_small(path, nrtm4::MAX_PUBLIC_KEY_BYTES))
                .transpose()?;
            let fetcher = nrtm4_remote::Fetcher::new(
                &notification_url,
                Duration::from_secs(config.io_timeout_seconds.min(30)),
                Duration::from_secs(timeout_seconds),
            )?;
            let compact = fetcher.notification()?;
            let result = nrtm4_apply::apply_remote(
                &store,
                &source,
                &compact,
                public_key.as_deref(),
                config.max_snapshot_bytes,
                |reference, remaining| fetcher.file(reference, remaining),
            )?;
            println!("{result}");
        }
        Command::Source {
            command:
                SourceCommand::VerifyNrtm4 {
                    source,
                    notification,
                    public_key,
                    snapshot,
                    delta,
                    delta_version,
                },
        } => {
            ensure!(config.sources.contains(&source), "source is not configured");
            let selected = snapshot
                .as_deref()
                .map(nrtm4_local::SelectedFile::Snapshot)
                .or_else(|| {
                    delta
                        .as_deref()
                        .zip(delta_version)
                        .map(|(path, version)| nrtm4_local::SelectedFile::Delta(path, version))
                });
            let result = nrtm4_local::verify(
                &source,
                &notification,
                &public_key,
                selected,
                config.max_snapshot_bytes,
            )?;
            println!("{result}");
        }
        Command::Config {
            command: ConfigCommand::Check,
        } => {
            RoutePolicy::load(&config)?;
            println!("configuration valid");
        }
        Command::Database {
            command: DatabaseCommand::Verify,
        } => {
            let objects =
                Store::open_existing_with_cache(&config.database, config.database_cache_bytes())?
                    .verify()?;
            println!("{}", serde_json::json!({ "objects": objects }));
        }
        Command::Database {
            command: DatabaseCommand::Count,
        } => {
            let store =
                Store::open_existing_with_cache(&config.database, config.database_cache_bytes())?;
            println!(
                "{}",
                serde_json::json!({"objects": store.object_count()?, "integrity_verified": false})
            );
        }
        Command::Database {
            command: DatabaseCommand::Backup { file },
        } => {
            let bytes = backup::create(&config.database, &file, config.database_cache_bytes())?;
            println!("{}", serde_json::json!({ "database_bytes": bytes }));
        }
        Command::Database {
            command: DatabaseCommand::Restore { file },
        } => {
            let bytes = backup::restore(&file, &config.database, config.database_cache_bytes())?;
            println!("{}", serde_json::json!({ "database_bytes": bytes }));
        }
        Command::Database {
            command: DatabaseCommand::Compact { yes },
        } => {
            ensure!(yes, "database compact requires --yes");
            let mut store = Store::open_for_service_with_cache(
                &config.database,
                config.database_cache_bytes(),
            )?;
            let before = fs::metadata(&config.database)?.len();
            let compacted = store.compact()?;
            let after = fs::metadata(&config.database)?.len();
            println!(
                "{}",
                serde_json::json!({
                    "compacted": compacted, "file_bytes_before": before, "file_bytes_after": after,
                })
            );
        }
        Command::Import {
            command:
                ImportCommand::Snapshot {
                    source,
                    file,
                    authoritative_restore,
                    serial_floor,
                },
        } => {
            ensure!(config.sources.contains(&source), "source is not configured");
            ensure!(
                config.authoritative_sources.contains(&source) == authoritative_restore,
                "authoritative sources require --authoritative-restore for snapshot replacement"
            );
            let start = Instant::now();
            let records = import::snapshot(&file, config.max_snapshot_bytes)?;
            let store = Store::open_with_cache(&config.database, config.database_cache_bytes())?;
            let objects = if authoritative_restore {
                store.replace_authoritative_source(&source, records, serial_floor)?
            } else {
                store.replace_source(&source, records)?
            };
            println!(
                "{}",
                serde_json::json!({
                    "source": source, "objects": objects, "elapsed_seconds": start.elapsed().as_secs_f64()
                })
            );
        }
        Command::Server => {
            let route_policy = RoutePolicy::load(&config)?;
            let store = Arc::new(
                Store::open_for_service_with_cache(
                    &config.database,
                    config.database_cache_bytes(),
                )?
                .with_legacy_md5_sources(config.legacy_md5_sources.clone())
                .with_route_policy(route_policy),
            );
            let mut runtime = tokio::runtime::Builder::new_multi_thread();
            if let Some(threads) = config.worker_threads {
                runtime.worker_threads(threads);
            }
            if let Some(threads) = config.max_blocking_threads {
                runtime.max_blocking_threads(threads);
            }
            runtime
                .enable_all()
                .build()?
                .block_on(serve(config, store))?;
        }
        Command::Export {
            command:
                ExportCommand::Snapshot {
                    source,
                    file,
                    include_sensitive: _,
                    public,
                },
        } => {
            ensure!(config.sources.contains(&source), "source is not configured");
            let store =
                Store::open_existing_with_cache(&config.database, config.database_cache_bytes())?;
            let report = if public {
                let (objects, serial) =
                    export(&file, |output| store.export_public_source(&source, output))?;
                serde_json::json!({ "source": source, "objects": objects, "serial": serial })
            } else {
                let objects = export(&file, |output| store.export_source(&source, output))?;
                serde_json::json!({ "source": source, "objects": objects })
            };
            println!("{report}");
        }
        Command::Source {
            command: SourceCommand::Purge { source, yes },
        } => {
            ensure!(yes, "source purge requires --yes");
            let store = open_source(&config, &source)?;
            if config.authoritative_sources.contains(&source) {
                store.replace_authoritative_source(&source, std::iter::empty(), None)?;
            } else {
                store.replace_source(&source, std::iter::empty())?;
            }
            println!(
                "{}",
                serde_json::json!({ "source": source, "purged": true })
            );
        }
        Command::Source {
            command: SourceCommand::Status { source },
        } => {
            let store = open_source(&config, &source)?;
            let serial = store.source_serial(&source)?;
            let local_serial = store.source_local_serial(&source)?;
            let nrtm4 = store.nrtm4_state(&source)?.map(|state| {
                serde_json::json!({
                    "session_id": state.session_id, "version": state.version,
                })
            });
            println!(
                "{}",
                serde_json::json!({ "source": source, "serial": serial, "local_serial": local_serial, "nrtm4": nrtm4 })
            );
        }
        Command::Source {
            command: SourceCommand::History { source, limit },
        } => {
            let store = open_source(&config, &source)?;
            let history = store.source_history(&source, limit)?;
            println!("{}", serde_json::to_string(&history)?);
        }
        Command::Source {
            command:
                SourceCommand::ApplyNrtm3 {
                    source,
                    file,
                    base_serial,
                },
        } => {
            ensure!(
                !config.authoritative_sources.contains(&source),
                "source is authoritative"
            );
            let store = open_source(&config, &source)?;
            let input = BufReader::new(File::open(file).context("opening NRTMv3 transcript")?);
            let serial = apply_nrtm3(
                &store,
                &source,
                base_serial,
                input,
                config.max_snapshot_bytes,
            )?;
            print_serial(&source, Some(serial));
        }
        Command::Source {
            command:
                SourceCommand::SyncNrtm3 {
                    source,
                    endpoint,
                    base_serial,
                    timeout_seconds,
                },
        } => {
            ensure!(
                !config.authoritative_sources.contains(&source),
                "source is authoritative"
            );
            let store = open_source(&config, &source)?;
            let committed = store.source_serial(&source)?;
            let base = match (committed, base_serial) {
                (Some(base), None) | (None, Some(base)) => base,
                (Some(_), Some(_)) => anyhow::bail!("source already has a committed serial"),
                (None, None) => anyhow::bail!("first sync requires --base-serial"),
            };
            let start = base
                .checked_add(1)
                .context("source serial cannot advance")?;
            let filter = config
                .nrtm3_sync
                .get(&source)
                .filter(|mirror| {
                    mirror.endpoint == endpoint && !mirror.object_class_filter.is_empty()
                })
                .map(|mirror| &mirror.object_class_filter);
            match nrtm_client::fetch_staged(
                endpoint,
                &source,
                start,
                Duration::from_secs(timeout_seconds.unwrap_or(config.io_timeout_seconds)),
                config.max_snapshot_bytes,
            )? {
                Some(input) => {
                    let serial = if let Some(classes) = filter {
                        let transcript =
                            nrtm3::Reader::filtered(input, &source, config.max_snapshot_bytes)
                                .map_err(anyhow::Error::msg)?;
                        store.apply_nrtm3_filtered_if(
                            &source,
                            transcript.range(),
                            base_serial,
                            transcript,
                            Some(classes),
                            || true,
                        )?
                    } else {
                        apply_nrtm3(
                            &store,
                            &source,
                            base_serial,
                            input,
                            config.max_snapshot_bytes,
                        )?
                    };
                    print_serial(&source, Some(serial));
                }
                None => {
                    if let Some(classes) = filter {
                        store.confirm_mirror_base(&source, base, classes)?;
                    }
                    println!(
                        "{}",
                        serde_json::json!({
                            "source":source, "serial":store.source_serial(&source)?, "up_to_date":true,
                        })
                    );
                }
            }
        }
        Command::Rpki {
            command:
                RpkiCommand::Validate {
                    roas,
                    prefix,
                    origin,
                },
        } => {
            let file = File::open(roas).context("opening validated ROA feed")?;
            let table =
                rpki::RoaTable::read(file, config.max_snapshot_bytes.min(256 * 1024 * 1024))?;
            let state = table.validate(&prefix, origin)?;
            println!(
                "{}",
                serde_json::json!({
                    "prefix": prefix, "origin": origin, "state": state, "roas": table.len(),
                })
            );
        }
        Command::Update {
            source,
            file,
            delete,
        } => {
            ensure!(
                config.authoritative_sources.contains(&source),
                "source is not authoritative"
            );
            let mut changes = Vec::new();
            for object in import::snapshot(&file, config.max_snapshot_bytes.min(2 * 1024 * 1024))? {
                ensure!(
                    changes.len() < authoritative::MAX_CHANGES,
                    "too many authoritative changes"
                );
                let object = object?;
                changes.push(if delete {
                    authoritative::Change::Delete(object)
                } else {
                    authoritative::Change::Upsert(object)
                });
            }
            let passwords = read_update_passwords()?;
            let route_policy = RoutePolicy::load(&config)?;
            let store = open_source(&config, &source)?.with_route_policy(route_policy);
            let changed = store.apply_authoritative(&source, changes, &passwords)?;
            println!(
                "{}",
                serde_json::json!({ "source": source, "changed": changed })
            );
        }
    }
    Ok(())
}

fn read_update_passwords() -> Result<Zeroizing<Vec<String>>> {
    const LIMIT: u64 = 2048;
    let stdin = io::stdin();
    ensure!(
        !stdin.is_terminal(),
        "pipe one password per line to standard input"
    );
    let mut input = Zeroizing::new(String::new());
    stdin
        .lock()
        .take(LIMIT + 1)
        .read_to_string(&mut input)
        .context("reading submission credentials")?;
    ensure!(
        input.len() as u64 <= LIMIT,
        "submission credentials exceed byte limit"
    );
    let passwords = Zeroizing::new(input.lines().map(str::to_owned).collect::<Vec<_>>());
    ensure!(
        (1..=20).contains(&passwords.len()),
        "provide between 1 and 20 passwords"
    );
    Ok(passwords)
}

fn open_nrtm4_source(config: &Config, source: &str) -> Result<Store> {
    ensure!(
        !config.authoritative_sources.contains(source),
        "source is authoritative"
    );
    ensure!(
        config.sources.iter().any(|name| name == source),
        "source is not configured"
    );
    if config.database.exists() {
        Store::open_for_service_with_cache(&config.database, config.database_cache_bytes())
    } else {
        Store::open_with_cache(&config.database, config.database_cache_bytes())
    }
}

fn open_source(config: &Config, source: &str) -> Result<Store> {
    ensure!(
        config.sources.iter().any(|configured| configured == source),
        "source is not configured"
    );
    Ok(
        Store::open_for_service_with_cache(&config.database, config.database_cache_bytes())?
            .with_legacy_md5_sources(config.legacy_md5_sources.clone()),
    )
}

fn apply_nrtm3(
    store: &Store,
    source: &str,
    base_serial: Option<u64>,
    input: impl BufRead,
    max_bytes: u64,
) -> Result<u64> {
    let transcript = nrtm3::Reader::new(input, source, max_bytes).map_err(anyhow::Error::msg)?;
    store.apply_nrtm3(source, transcript.range(), base_serial, transcript)
}

fn print_serial(source: &str, serial: Option<u64>) {
    println!(
        "{}",
        serde_json::json!({ "source": source, "serial": serial })
    );
}

async fn serve(config: Config, store: Arc<Store>) -> Result<()> {
    let whois = TcpListener::bind(config.listen)
        .await
        .context("binding WHOIS")?;
    let http = match config.http_listen {
        Some(address) => Some(
            TcpListener::bind(address)
                .await
                .context("binding GraphQL HTTP")?,
        ),
        None => None,
    };
    let (stop, stopped) = watch::channel(false);
    let mut services = JoinSet::new();
    for (source, mirror) in &config.nrtm3_sync {
        services.spawn(mirror_worker::run(
            store.clone(),
            source.clone(),
            mirror.clone(),
            config.max_snapshot_bytes,
            Duration::from_secs(config.io_timeout_seconds),
            stopped.clone(),
        ));
    }
    if let Some(http) = http {
        let store = store.clone();
        let sources = config
            .http_sources
            .clone()
            .unwrap_or_else(|| config.sources.clone());
        let update_sources = config
            .http_authoritative_updates
            .then(|| config.authoritative_sources.clone());
        let timeout = Duration::from_secs(config.io_timeout_seconds);
        let mut stopped = stopped.clone();
        services.spawn(graphql::serve(
            http,
            store,
            graphql::HttpServiceConfig {
                sources: sources.clone(),
                authoritative_sources: config.authoritative_sources.clone(),
                update_sources,
                timeout,
                dashboard: dashboard::Metadata {
                    mode: "live",
                    version: env!("CARGO_PKG_VERSION"),
                    node: config.node.clone(),
                    sources,
                    whois_listen: whois.local_addr()?.to_string(),
                    max_connections: config.max_connections,
                    database_cache_mib: config.database_cache_mib,
                },
            },
            async move {
                let _ = stopped.changed().await;
            },
        ));
    }
    services.spawn(async move { whois::serve(whois, &config, store, stopped).await });
    let mut result = tokio::select! {
        result = shutdown_signal() => result.context("waiting for shutdown"),
        service = services.join_next() => match service {
            Some(Ok(Err(error))) => Err(error),
            Some(Err(error)) => Err(error.into()),
            _ => Err(anyhow::anyhow!("a listener stopped unexpectedly")),
        },
    };
    let _ = stop.send(true);
    while let Some(service) = services.join_next().await {
        result = result.and_then(|()| service?);
    }
    result
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

fn export<T>(path: &Path, write_snapshot: impl FnOnce(&mut dyn Write) -> Result<T>) -> Result<T> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file =
        tempfile::NamedTempFile::new_in(directory).context("creating export temporary file")?;
    let objects = {
        let mut output = BufWriter::new(file.as_file_mut());
        let objects = write_snapshot(&mut output)?;
        output.flush().context("flushing snapshot")?;
        objects
    };
    file.as_file().sync_all().context("syncing snapshot")?;
    file.persist_noclobber(path)
        .context("publishing snapshot")?;
    Ok(objects)
}
