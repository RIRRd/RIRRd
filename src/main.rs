#![forbid(unsafe_code)]

mod config;
mod graphql;
mod import;
mod rpsl;
mod sets;
mod storage;
mod whois;

use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use tokio::{net::TcpListener, sync::watch, task::JoinSet};
use tracing_subscriber::EnvFilter;

use crate::{config::Config, storage::Store};

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
}

#[derive(Subcommand)]
enum SourceCommand {
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
        #[arg(long, required = true)]
        include_sensitive: bool,
    },
}

#[derive(Subcommand)]
enum DatabaseCommand {
    Verify,
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
        Command::Config {
            command: ConfigCommand::Check,
        } => println!("configuration valid"),
        Command::Database {
            command: DatabaseCommand::Verify,
        } => {
            let objects = Store::open_existing(&config.database)?.verify()?;
            println!("{}", serde_json::json!({ "objects": objects }));
        }
        Command::Import {
            command: ImportCommand::Snapshot { source, file },
        } => {
            ensure!(config.sources.contains(&source), "source is not configured");
            let start = Instant::now();
            let records = import::snapshot(&file, config.max_snapshot_bytes)?;
            let store = Store::open(&config.database)?;
            let objects = store.replace_source(&source, records)?;
            println!(
                "{}",
                serde_json::json!({
                    "source": source, "objects": objects, "elapsed_seconds": start.elapsed().as_secs_f64()
                })
            );
        }
        Command::Server => {
            let store = Arc::new(Store::open_for_service(&config.database)?);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(serve(config, store))?;
        }
        Command::Export {
            command:
                ExportCommand::Snapshot {
                    source,
                    file,
                    include_sensitive,
                },
        } => {
            ensure!(include_sensitive, "export requires --include-sensitive");
            ensure!(config.sources.contains(&source), "source is not configured");
            let store = Store::open_existing(&config.database)?;
            let objects = export(&store, &source, &file)?;
            println!(
                "{}",
                serde_json::json!({ "source": source, "objects": objects })
            );
        }
        Command::Source {
            command: SourceCommand::Purge { source, yes },
        } => {
            ensure!(yes, "source purge requires --yes");
            ensure!(config.sources.contains(&source), "source is not configured");
            let store = Store::open_for_service(&config.database)?;
            store.replace_source(&source, std::iter::empty())?;
            println!(
                "{}",
                serde_json::json!({ "source": source, "purged": true })
            );
        }
    }
    Ok(())
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
    if let Some(http) = http {
        let store = store.clone();
        let sources = config.sources.clone();
        let timeout = Duration::from_secs(config.io_timeout_seconds);
        let mut stopped = stopped.clone();
        services.spawn(graphql::serve(http, store, sources, timeout, async move {
            let _ = stopped.changed().await;
        }));
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

fn export(store: &Store, source: &str, path: &Path) -> Result<u64> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file =
        tempfile::NamedTempFile::new_in(directory).context("creating export temporary file")?;
    let objects = {
        let mut output = BufWriter::new(file.as_file_mut());
        let objects = store.export_source(source, &mut output)?;
        output.flush().context("flushing snapshot")?;
        objects
    };
    file.as_file().sync_all().context("syncing snapshot")?;
    file.persist_noclobber(path)
        .context("publishing snapshot")?;
    Ok(objects)
}
