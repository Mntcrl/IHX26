//! Verification pipeline from an image file to a `galaxy-report/v1` report: image ids, XRPL transaction retrieval, attestation, ledger windows, sampling, public inputs and Groth16 verification (spec §13).

mod circuit;
mod pipeline;
pub mod recorded;
mod report;

use galaxy_protocol::{network::NetworkConfig, Hash32};
use galaxy_xrpl::LedgerSource;
use std::future::Future;

pub use circuit::{inspect_circuit, CircuitInfo};
pub use pipeline::{
    photo_identity, verify_by_photo_id, verify_manifest, verify_photo, verify_proof_only,
    verify_proof_transaction, PhotoIdentity,
};
pub use report::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Refs {
    pub manifest_tx: Hash32,
    pub proof_tx: Option<Hash32>,
}

/// The caller controls the ledger and accepted trust roots. The transport
/// must bound each request, including response bodies and elapsed time.
pub struct VerifyContext<'a, L: LedgerSource> {
    pub ledger: &'a L,
    pub config: &'a NetworkConfig,
    pub rpc_endpoints: Vec<String>,
    pub rpc_trust: RpcTrust,
    /// Optional canonical GXCB bytes, matched against all three configured IDs.
    pub local_circuit: Option<&'a [u8]>,
}

impl<'a, L: LedgerSource> VerifyContext<'a, L> {
    pub fn new(ledger: &'a L, config: &'a NetworkConfig) -> Self {
        Self {
            ledger,
            config,
            rpc_endpoints: Vec::new(),
            rpc_trust: RpcTrust::Remote,
            local_circuit: None,
        }
    }

    fn report(&self, mode: &'static str) -> Report {
        Report::new(
            self.config.name.clone(),
            mode,
            TrustReport {
                rpc_endpoints: self.rpc_endpoints.clone(),
                rpc_trust: self.rpc_trust,
                ..TrustReport::default()
            },
        )
    }
}

pub const MAX_LOOKUP_CANDIDATES: usize = 32;
pub const MAX_LOOKUP_NODES: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct LookupResult {
    pub candidates: Vec<Refs>,
    pub ranges: Vec<LedgerRange>,
    /// True if the requested pagination was incomplete or the index reports gaps.
    pub incomplete: bool,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct LookupError(pub String);

/// A lookup provider sees only the photo identifier and network. Returned
/// references do not establish image binding, validity, or global absence.
pub trait LookupSource {
    fn endpoint(&self) -> &str;
    fn lookup(
        &self,
        photo_id: &Hash32,
        network_id: u32,
    ) -> impl Future<Output = Result<LookupResult, LookupError>>;
}

/// Maximum size of one node lookup response body (bytes).
pub const MAX_LOOKUP_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeLookupResponse {
    candidates: Vec<NodeRef>,
    ranges: Vec<LedgerRange>,
    incomplete: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeRef {
    manifest_tx: String,
    proof_tx: Option<String>,
}

/// Parse the body of `GET {node}/v1/photos/{photo_id}?network_id={n}`:
/// `{"candidates": [{"manifest_tx", "proof_tx"|null}], "ranges": [{"min", "max"}], "incomplete": bool}`.
/// The result is an untrusted hint list; every candidate is verified from XRPL.
pub fn parse_lookup_response(body: &[u8]) -> Result<LookupResult, LookupError> {
    if body.len() > MAX_LOOKUP_RESPONSE_BYTES {
        return Err(LookupError("oversized node response".into()));
    }
    let response: NodeLookupResponse =
        serde_json::from_slice(body).map_err(|_| LookupError("malformed node response".into()))?;
    let hash = |text: &str| {
        let mut out = [0u8; 32];
        hex::decode_to_slice(text, &mut out).map_err(|_| LookupError("malformed transaction hash in node response".into()))?;
        Ok::<_, LookupError>(out)
    };
    let candidates = response
        .candidates
        .iter()
        .map(|entry| {
            Ok(Refs { manifest_tx: hash(&entry.manifest_tx)?, proof_tx: entry.proof_tx.as_deref().map(hash).transpose()? })
        })
        .collect::<Result<_, LookupError>>()?;
    Ok(LookupResult { candidates, ranges: response.ranges, incomplete: response.incomplete })
}
