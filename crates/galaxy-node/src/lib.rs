//! PHØTON lookup node (Galaxy v1 protocol, spec §14): XRPL backfill and live
//! subscription into a disposable SQLite cache, proof-only verification, a
//! versioned REST API, and signed peer discovery. Nodes do not decide validity;
//! clients verify candidate references independently.

mod api;
mod client;
mod config;
mod db;
mod discovery;
mod records;
mod sync;
mod verify;

#[cfg(test)]
mod tests;

use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tokio::sync::Notify;

pub use config::Config;

pub const SOFTWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Protocol whose records this node indexes.
pub const PROTOCOL_VERSION: &str = "galaxy/v1";
/// Node API and descriptor protocol.
pub const API_VERSION: &str = "galaxy-node/v1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("node configuration: {0}")]
    Config(&'static str),
    #[error("network configuration: {0}")]
    NetworkConfig(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("cache database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Xrpl(#[from] galaxy_xrpl::Error),
    #[error(
        "ledger chain mismatch at ledger {0}: the endpoint history contradicts stored checkpoints"
    )]
    Chain(u32),
    #[error("internal invariant: {0}")]
    Invariant(&'static str),
    #[error("peer: {0}")]
    Peer(&'static str),
    #[error("HTTP: {0}")]
    Http(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Run the daemon until Ctrl-C/SIGTERM: sync (backfill + live), proof
/// verification, discovery and the REST API on `listen`.
pub async fn run(config: Config) -> Result<()> {
    #[cfg(feature = "test-faults")]
    if let Some(fault) = config.node.test_fault {
        tracing::warn!(
            ?fault,
            "test fault injection enabled; this node serves incorrect lookup results on purpose"
        );
    }
    let config = Arc::new(config);
    let db = Arc::new(db::Db::open(&config)?);
    let identity = Arc::new(discovery::Identity::load_or_create(&config.node.data_dir)?);
    let rpc = sync::rpc_client(&config)?;
    let sources = Arc::new(sync::Sources::new(&config)?);
    let limiter = Arc::new(sync::Limiter::new(
        config.node.limits.sync_requests_per_second,
    ));
    let resync = Arc::new(Notify::new());
    let proof = Arc::new(Notify::new());
    let listener = tokio::net::TcpListener::bind(config.node.listen).await?;
    tracing::info!(listen = %config.node.listen, network = %config.network.name, "galaxy-node started");

    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(sync::run(
        config.clone(),
        db.clone(),
        sources.clone(),
        limiter.clone(),
        resync.clone(),
        proof.clone(),
    ));
    if config.node.live {
        tasks.spawn(sync::live(
            config.clone(),
            db.clone(),
            rpc.clone(),
            sources,
            limiter,
            resync,
            proof.clone(),
        ));
    }
    tasks.spawn(verify::run(config.clone(), db.clone(), rpc, proof));
    if config.node.discovery {
        tasks.spawn(discovery::run(config.clone(), db.clone(), identity.clone()));
    }
    let app = api::router(api::State::new(config, db, identity));
    tasks.spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(Error::from)
    });
    tokio::select! {
        () = shutdown() => Ok(()),
        Some(result) = tasks.join_next() => match result {
            Ok(result) => result,
            Err(_) => Err(Error::Invariant("node task panicked")),
        },
    }
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => return std::future::pending().await,
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// Backfill to the current validated tip (or `end_ledger`), verify every
/// queued PROOF once, and return the local status document.
pub async fn sync_once(config: Config) -> Result<Value> {
    let db = db::Db::open(&config)?;
    let rpc = sync::rpc_client(&config)?;
    let sources = sync::Sources::new(&config)?;
    let limiter = sync::Limiter::new(config.node.limits.sync_requests_per_second);
    sync::catch_up(&config, &db, &sources, &limiter).await?;
    verify::drain(&config, &db, &rpc).await?;
    api::status_document(&config, &db)
}

/// Delete every cached row (the cache is reconstructible from XRPL), then
/// run [`sync_once`]. The node identity key is kept.
pub async fn rebuild(config: Config) -> Result<Value> {
    db::Db::wipe(&config.node.data_dir)?;
    sync_once(config).await
}

/// Status of a local cache, read without network access.
pub fn local_status(config: Config) -> Result<Value> {
    let db = db::Db::open(&config)?;
    api::status_document(&config, &db)
}

/// Cached, still unexpired peer descriptors of a local cache.
pub fn local_peers(config: Config) -> Result<Value> {
    let db = db::Db::open(&config)?;
    api::peers_document(&config, &db)
}

/// `GET {url}/v1/status` of any node. The URL is chosen by the caller, so
/// loopback and private addresses are allowed; redirects are never followed.
pub async fn status(url: &str) -> Result<Value> {
    client::get_json(url, "v1/status", true).await
}

/// `GET {url}/v1/peers` of any node (see [`status`]).
pub async fn peers(url: &str) -> Result<Value> {
    client::get_json(url, "v1/peers", true).await
}

/// `GET {url}/health`; like [`status`], the caller chooses the destination.
pub async fn health(url: &str) -> Result<Value> {
    client::get_json(url, "health", true).await
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(crate) const RETRY_DELAY: Duration = Duration::from_secs(10);
