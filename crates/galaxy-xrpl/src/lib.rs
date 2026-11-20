//! XRPL codec, self-custodied transaction signing, ledger access and Galaxy registry.

pub mod codec;
#[cfg(all(feature = "native", unix))]
pub mod dev_wallet;
pub mod keys;
pub mod ledger;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod native;
pub mod registry;
pub mod submission;
pub mod transaction;

pub use galaxy_protocol::{AccountId, Hash32};
pub use ledger::LedgerSource;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("XRPL binary codec: {0}")]
    Codec(String),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("XRPL RPC: {0}")]
    Rpc(String),
    #[error("XRPL network mismatch, or mainnet not explicitly enabled")]
    Network,
    #[error("XRPL history unavailable: {0}")]
    HistoryUnavailable(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] galaxy_protocol::wire::WireError),
}
