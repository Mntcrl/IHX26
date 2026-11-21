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
pub const MAX_LOOKUP_RESPONSE_BYTES: usize = 256 * 1024;
pub const DEFAULT_LOOKUP_PAGE_SIZE: usize = 50;
pub const MAX_LOOKUP_PAGES: usize = 8;

#[derive(Debug)]
pub struct LookupPage {
    pub result: LookupResult,
    pub next_cursor: Option<String>,
}

#[derive(serde::Deserialize)]
struct NodeLookupResponse {
    protocol: String,
    network: String,
    network_id: u32,
    software_version: String,
    photo_id: String,
    #[serde(deserialize_with = "nullable")]
    indexed_from: Option<u32>,
    #[serde(deserialize_with = "nullable")]
    indexed_through: Option<u32>,
    indexed_ranges: Vec<NodeRange>,
    incomplete: bool,
    captures: Vec<NodeRef>,
    #[serde(deserialize_with = "nullable")]
    next_cursor: Option<String>,
}

#[derive(serde::Deserialize)]
struct NodeRange { from: u32, through: u32 }

#[derive(serde::Deserialize)]
struct NodeRef {
    capture_id: String,
    manifest_tx: String,
    ledger_index: u32,
    #[serde(deserialize_with = "nullable")]
    proof_tx: Option<String>,
}

fn nullable<'de, D: serde::Deserializer<'de>, T: serde::Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    serde::Deserialize::deserialize(d)
}

fn valid_semver(version: &str) -> bool {
    if version.len() > 64 { return false; }
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let identifiers = |part: &str, prerelease: bool| part.split('.').all(|id| {
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && (!prerelease || !digits(id) || id.len() == 1 || !id.starts_with('0'))
    });
    let (version, build) = version.split_once('+').map_or((version, None), |(v, b)| (v, Some(b)));
    let (version, prerelease) = version.split_once('-').map_or((version, None), |(v, p)| (v, Some(p)));
    let mut core = version.split('.');
    (0..3).all(|_| core.next().is_some_and(|part| digits(part) && (part.len() == 1 || !part.starts_with('0'))))
        && core.next().is_none() && prerelease.is_none_or(|p| identifiers(p, true)) && build.is_none_or(|b| identifiers(b, false))
}

/// Parse one untrusted REST lookup page (spec §14). Only XRPL verification can
/// establish the correctness of the advertised transaction references.
pub fn parse_lookup_response(body: &[u8], photo_id: &Hash32, network_id: u32, limit: usize) -> Result<LookupPage, LookupError> {
    let invalid = || LookupError("malformed node lookup response".into());
    if body.len() > MAX_LOOKUP_RESPONSE_BYTES {
        return Err(LookupError("oversized node response".into()));
    }
    let response: NodeLookupResponse = serde_json::from_slice(body).map_err(|_| invalid())?;
    let network = galaxy_protocol::network::NetworkId::from_u32(network_id).ok_or_else(invalid)?;
    let hash = |text: &str, uppercase: bool| {
        if text.bytes().any(|b| if uppercase { b.is_ascii_lowercase() } else { b.is_ascii_uppercase() }) { return Err(invalid()); }
        let mut out = [0u8; 32];
        hex::decode_to_slice(text, &mut out).map_err(|_| invalid())?;
        Ok(out)
    };
    if response.protocol != "galaxy-node/v1" || response.network_id != network_id || response.network != network.name()
        || hash(&response.photo_id, false)? != *photo_id || !valid_semver(&response.software_version)
        || !(1..=100).contains(&limit) || response.captures.len() > limit
        || response.indexed_ranges.first().map(|r| r.from) != response.indexed_from
        || response.indexed_ranges.last().map(|r| r.through) != response.indexed_through
        || !response.incomplete && response.indexed_ranges.len() != 1
        || response.indexed_ranges.iter().any(|r| r.from == 0 || r.through < r.from)
        || response.indexed_ranges.windows(2).any(|pair| pair[0].through.saturating_add(1) >= pair[1].from)
        || response.next_cursor.as_ref().is_some_and(|cursor| response.captures.is_empty()
            || cursor.is_empty() || cursor.len() > 256 || !cursor.bytes().all(|b| b.is_ascii_graphic()))
        || response.captures.windows(2).any(|rows| rows[0].ledger_index > rows[1].ledger_index) {
        return Err(invalid());
    }
    let candidates = response.captures.into_iter().map(|entry| {
        hash(&entry.capture_id, false)?;
        if entry.ledger_index == 0 { return Err(invalid()); }
        Ok(Refs { manifest_tx: hash(&entry.manifest_tx, true)?, proof_tx: entry.proof_tx.as_deref().map(|v| hash(v, true)).transpose()? })
    }).collect::<Result<_, LookupError>>()?;
    let ranges = response.indexed_ranges.into_iter().map(|r| LedgerRange { min: r.from, max: r.through }).collect();
    Ok(LookupPage { result: LookupResult { candidates, ranges, incomplete: response.incomplete }, next_cursor: response.next_cursor })
}
