#![forbid(unsafe_code)]

mod config;
mod import;
mod rpsl;
mod storage;
mod whois;

use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
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
            let store = Arc::new(Store::open_existing(&config.database)?);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(whois::serve(&config, store))?;
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
    }
    Ok(())
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
