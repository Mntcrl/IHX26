//! UniFFI surface of the shared Rust core for the iOS capture app (spec §15).
//! No networking, logging, or secret storage. Sensitive persistence is Swift's responsibility.

mod capture;
mod evidence;
mod export;
mod ledger;
mod registration;

pub use capture::*;
pub use evidence::*;
pub use export::*;
pub use ledger::*;
pub use registration::*;

uniffi::setup_scaffolding!("galaxy_ffi");

/// Stable Swift error cases. Append new cases rather than changing their order.
/// Do not include downstream errors: they may contain private input or RPC bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum GalaxyError {
    #[error("Invalid Galaxy input")]
    InvalidInput,
    #[error("Invalid capture evidence")]
    Evidence,
    #[error("Could not persist or restore capture evidence")]
    EvidenceBundle,
    #[error("Secure randomness unavailable")]
    Randomness,
    #[error("Invalid App Attest input")]
    Attestation,
    #[error("Invalid or unaccepted circuit, policy or proving artifact")]
    Circuit,
    #[error("Capture does not match the committed statement")]
    CaptureBinding,
    #[error("Unexpected ledger or transaction window")]
    LedgerWindow,
    #[error("Capture did not pass the reference policy")]
    Inconclusive,
    #[error("Could not build the witness")]
    Witness,
    #[error("Could not produce a proof")]
    Proving,
    #[error("Proof failed local verification")]
    ProofVerification,
    #[error("Proving was cancelled")]
    Cancelled,
    #[error("Invalid image export")]
    Image,
    #[error("XRPL network not permitted or does not match")]
    Network,
    #[error("Invalid XRPL RPC response")]
    Rpc,
    #[error("Could not sign or use the XRPL transaction record")]
    Transaction,
    #[error("Internal Galaxy error")]
    Internal,
}

type Result<T, E = GalaxyError> = std::result::Result<T, E>;

fn guarded<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or(Err(GalaxyError::Internal))
}

fn exact<const N: usize>(bytes: &[u8]) -> Result<[u8; N]> {
    bytes.try_into().map_err(|_| GalaxyError::InvalidInput)
}

fn hash_hex(text: &str) -> Result<[u8; 32]> {
    let mut hash = [0; 32];
    hex::decode_to_slice(text, &mut hash).map_err(|_| GalaxyError::InvalidInput)?;
    Ok(hash)
}

fn network_id(value: u32) -> Result<galaxy_protocol::network::NetworkId> {
    galaxy_protocol::network::NetworkId::from_u32(value).ok_or(GalaxyError::Network)
}

#[derive(uniffi::Record)]
pub struct Memo {
    pub memo_type: String,
    pub data: Vec<u8>,
}

impl Memo {
    fn new(kind: galaxy_protocol::wire::MemoKind, data: Vec<u8>) -> Self {
        Self {
            memo_type: kind.as_str().into(),
            data,
        }
    }
}
