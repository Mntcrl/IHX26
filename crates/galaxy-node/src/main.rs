//! `galaxy-node` daemon: XRPL backfill and live subscription into a SQLite cache, REST API and signed peer discovery; nodes never decide validity (spec §14).

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use galaxy_node::{Config, Result};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API and synchronize continuously.
    Run {
        #[arg(long, short)]
        config: PathBuf,
    },
    /// Index available history through the current tip or configured end.
    Sync {
        #[arg(long, short)]
        config: PathBuf,
    },
    /// Clear the disposable cache and synchronize it again; keep node identity.
    Rebuild {
        #[arg(long, short)]
        config: PathBuf,
    },
    /// Show local or remote synchronization status.
    Status {
        #[arg(long, short, required_unless_present = "url", conflicts_with = "url")]
        config: Option<PathBuf>,
        #[arg(long)]
        url: Option<String>,
    },
    /// Show local or remote discovered peer descriptors.
    Peers {
        #[arg(long, short, required_unless_present = "url", conflicts_with = "url")]
        config: Option<PathBuf>,
        #[arg(long)]
        url: Option<String>,
    },
    /// Check service health over HTTP (also used by the container healthcheck).
    Health {
        #[arg(long)]
        url: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match execute(Args::parse().command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("galaxy-node: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn execute(command: Command) -> Result<()> {
    let document = match command {
        Command::Run { config } => {
            let config = Config::load(config)?;
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .or_else(|_| tracing_subscriber::EnvFilter::try_new(config.log_filter()))
                .map_err(|_| galaxy_node::Error::Config("invalid log filter"))?;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();
            return galaxy_node::run(config).await;
        }
        Command::Sync { config } => galaxy_node::sync_once(Config::load(config)?).await?,
        Command::Rebuild { config } => galaxy_node::rebuild(Config::load(config)?).await?,
        Command::Status {
            config: Some(config),
            ..
        } => galaxy_node::local_status(Config::load(config)?)?,
        Command::Status { url: Some(url), .. } => galaxy_node::status(&url).await?,
        Command::Peers {
            config: Some(config),
            ..
        } => galaxy_node::local_peers(Config::load(config)?)?,
        Command::Peers { url: Some(url), .. } => galaxy_node::peers(&url).await?,
        Command::Health { url } => galaxy_node::health(&url).await?,
        _ => return Err(galaxy_node::Error::Config("provide --config or --url")),
    };
    println!("{}", serde_json::to_string_pretty(&document)?);
    Ok(())
}
