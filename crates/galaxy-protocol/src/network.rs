//! XRPL network configuration (§12, ADR 0009). The accepted circuits, app ids and
//! attestation environments are the verifier's explicit trust anchors (§17): they come only
//! from this file, never from the ledger or from peers. Mainnet is refused unless
//! `allow_mainnet = true`.
use crate::wire::{valid_app_id, AttestationEnv};
use crate::Hash32;
use serde::Deserialize;
use std::path::Path;

/// Configuration files are small; larger inputs are rejected before parsing.
pub const MAX_CONFIG_LEN: usize = 64 * 1024;
pub const DEFAULT_MAX_FEE_DROPS: u64 = 1000;
/// Upper bound for `max_fee_drops` (1 XRP): a fee cap above this is treated as a configuration error.
pub const MAX_FEE_DROPS_LIMIT: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("cannot read network config: {0}")]
    Io(#[from] std::io::Error),
    #[error("network config exceeds {MAX_CONFIG_LEN} bytes")]
    TooLarge,
    #[error("invalid network config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid network config: {0}")]
    Invalid(&'static str),
    #[error("xrpl-mainnet is refused unless allow_mainnet = true")]
    MainnetRefused,
}

/// XRPL NetworkID values used by Galaxy (§3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum NetworkId {
    Mainnet = 0,
    Testnet = 1,
    Devnet = 2,
}

impl NetworkId {
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    pub const fn from_u32(id: u32) -> Option<Self> {
        match id {
            0 => Some(Self::Mainnet),
            1 => Some(Self::Testnet),
            2 => Some(Self::Devnet),
            _ => None,
        }
    }

    /// Network name as used in config files, XMP and reports.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mainnet => "xrpl-mainnet",
            Self::Testnet => "xrpl-testnet",
            Self::Devnet => "xrpl-devnet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedCircuit {
    pub circuit_id: Hash32,
    pub vk_digest: Hash32,
    pub policy_id: Hash32,
    /// CIRCUIT manifest transaction that publishes the bundle.
    pub registration_tx: Hash32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkConfig {
    pub name: String,
    pub network_id: NetworkId,
    pub json_rpc: String,
    pub websocket: String,
    pub trusted_local_rippled: Option<String>,
    /// Earliest ledger a verifier or node scans for Galaxy transactions.
    pub inception_ledger: u32,
    pub max_fee_drops: u64,
    pub allow_mainnet: bool,
    /// URL template containing `{hash}`.
    pub explorer_tx: String,
    /// URL template containing `{index}`.
    pub explorer_ledger: String,
    pub accepted_circuits: Vec<AcceptedCircuit>,
    pub accepted_app_ids: Vec<String>,
    /// Never contains `AttestationEnv::NotApplicable`.
    pub accepted_attestation_environments: Vec<AttestationEnv>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    name: String,
    network_id: u32,
    json_rpc: String,
    websocket: String,
    trusted_local_rippled: Option<String>,
    inception_ledger: u32,
    #[serde(default = "default_max_fee")]
    max_fee_drops: u64,
    allow_mainnet: bool,
    explorer_tx: String,
    explorer_ledger: String,
    accepted_circuits: Vec<RawCircuit>,
    accepted_app_ids: Vec<String>,
    accepted_attestation_environments: Vec<AttestationEnv>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCircuit {
    circuit_id: String,
    vk_digest: String,
    policy_id: String,
    registration_tx: String,
}

fn default_max_fee() -> u64 {
    DEFAULT_MAX_FEE_DROPS
}

impl NetworkConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, NetworkError> {
        let bytes = std::fs::read(path)?;
        if bytes.len() > MAX_CONFIG_LEN {
            return Err(NetworkError::TooLarge);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| NetworkError::Invalid("config is not UTF-8"))?;
        Self::from_toml(text)
    }

    /// Parse and validate a configuration from TOML text (used by `load` and by the browser verifier).
    pub fn from_toml(text: &str) -> Result<Self, NetworkError> {
        if text.len() > MAX_CONFIG_LEN {
            return Err(NetworkError::TooLarge);
        }
        let raw: RawConfig = toml::from_str(text)?;
        let network_id = NetworkId::from_u32(raw.network_id).ok_or(NetworkError::Invalid("network_id"))?;
        if network_id == NetworkId::Mainnet && !raw.allow_mainnet {
            return Err(NetworkError::MainnetRefused);
        }
        if raw.name != network_id.name() {
            return Err(NetworkError::Invalid("name does not match network_id"));
        }
        check_url(&raw.json_rpc, &["https://", "http://"], "json_rpc")?;
        check_url(&raw.websocket, &["wss://", "ws://"], "websocket")?;
        if let Some(url) = &raw.trusted_local_rippled {
            check_url(url, &["https://", "http://"], "trusted_local_rippled")?;
        }
        if raw.inception_ledger == 0 {
            return Err(NetworkError::Invalid("inception_ledger"));
        }
        if !(1..=MAX_FEE_DROPS_LIMIT).contains(&raw.max_fee_drops) {
            return Err(NetworkError::Invalid("max_fee_drops"));
        }
        check_template(&raw.explorer_tx, "{hash}", "explorer_tx")?;
        check_template(&raw.explorer_ledger, "{index}", "explorer_ledger")?;
        for (i, app_id) in raw.accepted_app_ids.iter().enumerate() {
            if !valid_app_id(app_id) || app_id.len() > usize::from(u16::MAX) {
                return Err(NetworkError::Invalid("accepted_app_ids entry"));
            }
            if raw.accepted_app_ids[..i].contains(app_id) {
                return Err(NetworkError::Invalid("duplicate accepted_app_ids entry"));
            }
        }
        for (i, env) in raw.accepted_attestation_environments.iter().enumerate() {
            if *env == AttestationEnv::NotApplicable {
                return Err(NetworkError::Invalid("accepted_attestation_environments entry"));
            }
            if raw.accepted_attestation_environments[..i].contains(env) {
                return Err(NetworkError::Invalid("duplicate accepted_attestation_environments entry"));
            }
        }
        let mut accepted_circuits: Vec<AcceptedCircuit> = Vec::with_capacity(raw.accepted_circuits.len());
        for circuit in &raw.accepted_circuits {
            let circuit = AcceptedCircuit {
                circuit_id: parse_hash(&circuit.circuit_id, "accepted_circuits.circuit_id")?,
                vk_digest: parse_hash(&circuit.vk_digest, "accepted_circuits.vk_digest")?,
                policy_id: parse_hash(&circuit.policy_id, "accepted_circuits.policy_id")?,
                registration_tx: parse_hash(&circuit.registration_tx, "accepted_circuits.registration_tx")?,
            };
            if accepted_circuits.iter().any(|c| c.circuit_id == circuit.circuit_id) {
                return Err(NetworkError::Invalid("duplicate accepted_circuits.circuit_id"));
            }
            accepted_circuits.push(circuit);
        }
        Ok(Self {
            name: raw.name,
            network_id,
            json_rpc: raw.json_rpc,
            websocket: raw.websocket,
            trusted_local_rippled: raw.trusted_local_rippled,
            inception_ledger: raw.inception_ledger,
            max_fee_drops: raw.max_fee_drops,
            allow_mainnet: raw.allow_mainnet,
            explorer_tx: raw.explorer_tx,
            explorer_ledger: raw.explorer_ledger,
            accepted_circuits,
            accepted_app_ids: raw.accepted_app_ids,
            accepted_attestation_environments: raw.accepted_attestation_environments,
        })
    }

    pub fn accepted_circuit(&self, circuit_id: &Hash32) -> Option<&AcceptedCircuit> {
        self.accepted_circuits.iter().find(|c| &c.circuit_id == circuit_id)
    }

    /// Explorer link for an XRPL transaction (uppercase hex, §2).
    pub fn explorer_tx_url(&self, tx_hash: &Hash32) -> String {
        self.explorer_tx.replace("{hash}", &hex::encode_upper(tx_hash))
    }

    pub fn explorer_ledger_url(&self, index: u32) -> String {
        self.explorer_ledger.replace("{index}", &index.to_string())
    }
}

fn check_url(url: &str, schemes: &[&str], field: &'static str) -> Result<(), NetworkError> {
    let rest = schemes.iter().find_map(|scheme| url.strip_prefix(scheme)).ok_or(NetworkError::Invalid(field))?;
    if rest.is_empty() || rest.starts_with('/') || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetworkError::Invalid(field));
    }
    Ok(())
}

fn check_template(template: &str, placeholder: &str, field: &'static str) -> Result<(), NetworkError> {
    check_url(template, &["https://", "http://"], field)?;
    if template.matches(placeholder).count() != 1 {
        return Err(NetworkError::Invalid(field));
    }
    Ok(())
}

/// 64 hex digits, either case (§2).
fn parse_hash(text: &str, field: &'static str) -> Result<Hash32, NetworkError> {
    let mut out = [0u8; 32];
    if text.len() != 64 || hex::decode_to_slice(text, &mut out).is_err() {
        return Err(NetworkError::Invalid(field));
    }
    Ok(out)
}
