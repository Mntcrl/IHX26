//! Galaxy v1 memo and bundle codecs (§10). Encoders take trusted values;
//! decoders check framing, lengths and format invariants before copying variable fields.
use crate::{canonical_json, ids, AccountId, Hash32};
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
pub const CAPTURE_CORE_LEN: usize = 299;
pub const PROOF_LEN: usize = 256;
pub const MAX_ASSERTION_LEN: usize = 600;
/// Dev-unattested assertion: 0xF0 ‖ ed25519 signature[64] (§10).
pub const DEV_ASSERTION_MARKER: u8 = 0xf0;
pub const DEV_ASSERTION_LEN: usize = 65;
pub const MAX_CHUNK_PAYLOAD: usize = 800;
pub const MAX_BUNDLE_LEN: usize = 1024 * 1024;
pub const MEMO_FORMAT: &str = "application/octet-stream";

// Canonical BN254 scalar modulus (r, not the base field q), in big-endian wire order.
const SCALAR_MODULUS: Hash32 = [
    0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29, 0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81,
    0x58, 0x5d, 0x28, 0x33, 0xe8, 0x48, 0x79, 0xb9, 0x70, 0x91, 0x43, 0xe1, 0xf5, 0x93,
    0xf0, 0x00, 0x00, 0x01,
];

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("truncated Galaxy wire value")]
    Truncated,
    #[error("unexpected trailing bytes")]
    Trailing,
    #[error("unknown Galaxy version: {0}")]
    Version(u8),
    #[error("unknown Galaxy memo type")]
    MemoType,
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("non-canonical bundle JSON: {0}")]
    Json(#[from] canonical_json::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoKind { Capture, Proof, DeviceChunk, CircuitChunk, Device, Circuit, Revoke, Node }

impl MemoKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "galaxy/v1/capture", Self::Proof => "galaxy/v1/proof",
            Self::DeviceChunk => "galaxy/v1/device-chunk", Self::CircuitChunk => "galaxy/v1/circuit-chunk",
            Self::Device => "galaxy/v1/device", Self::Circuit => "galaxy/v1/circuit",
            Self::Revoke => "galaxy/v1/revoke", Self::Node => "galaxy/v1/node",
        }
    }

    /// Parses an exact Galaxy v1 MemoType string; anything else (other case, whitespace, other versions) is `None`.
    pub fn from_memo_type(memo_type: &str) -> Option<Self> {
        memo_type.parse().ok()
    }
}

impl std::fmt::Display for MemoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
}

impl std::str::FromStr for MemoKind {
    type Err = WireError;

    fn from_str(s: &str) -> Result<Self, WireError> {
        Ok(match s {
            "galaxy/v1/capture" => Self::Capture, "galaxy/v1/proof" => Self::Proof,
            "galaxy/v1/device-chunk" => Self::DeviceChunk, "galaxy/v1/circuit-chunk" => Self::CircuitChunk,
            "galaxy/v1/device" => Self::Device, "galaxy/v1/circuit" => Self::Circuit,
            "galaxy/v1/revoke" => Self::Revoke, "galaxy/v1/node" => Self::Node,
            _ => return Err(WireError::MemoType),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceKind { AppleAppAttest = 1, DevUnattested = 0xf0 }

impl TryFrom<u8> for DeviceKind {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, WireError> {
        match value { 1 => Ok(Self::AppleAppAttest), 0xf0 => Ok(Self::DevUnattested), _ => Err(WireError::Invalid("device kind")) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AttestationEnv { NotApplicable = 0, Development = 1, Production = 2 }

impl TryFrom<u8> for AttestationEnv {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, WireError> {
        match value { 0 => Ok(Self::NotApplicable), 1 => Ok(Self::Development), 2 => Ok(Self::Production), _ => Err(WireError::Invalid("attestation environment")) }
    }
}

fn device_type(r: &mut Reader<'_>) -> Result<(DeviceKind, AttestationEnv), WireError> {
    let kind = DeviceKind::try_from(r.u8()?)?;
    let env = AttestationEnv::try_from(r.u8()?)?;
    if (kind == DeviceKind::DevUnattested) != (env == AttestationEnv::NotApplicable) {
        return Err(WireError::Invalid("device kind/environment combination"));
    }
    Ok((kind, env))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureCore {
    pub network: u32,
    pub registration_tx: Hash32,
    pub preledger_hash: Hash32,
    pub nonce: Hash32,
    pub preledger_index: u32,
    pub claimed_time_ms: u64,
    pub width: u16,
    pub height: u16,
    pub flags: u16,
    pub photo_id: Hash32,
    pub rgb_root: Hash32,
    pub evidence_root: Hash32,
    pub policy_id: Hash32,
    pub circuit_id: Hash32,
    pub account_id: AccountId,
}

impl CaptureCore {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(CAPTURE_CORE_LEN, b"");
        self.write_body(&mut out);
        out.0
    }

    fn write_body(&self, out: &mut Writer) {
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.registration_tx);
        out.raw(&self.preledger_hash);
        out.raw(&self.nonce);
        out.raw(&self.preledger_index.to_be_bytes());
        out.raw(&self.claimed_time_ms.to_be_bytes());
        out.raw(&self.width.to_be_bytes());
        out.raw(&self.height.to_be_bytes());
        out.raw(&self.flags.to_be_bytes());
        out.raw(&self.photo_id);
        out.raw(&self.rgb_root);
        out.raw(&self.evidence_root);
        out.raw(&self.policy_id);
        out.raw(&self.circuit_id);
        out.raw(&self.account_id);
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let result = Self {
            network: r.network()?, registration_tx: r.array()?, preledger_hash: r.array()?, nonce: r.array()?,
            preledger_index: r.u32()?, claimed_time_ms: r.u64()?, width: r.u16()?, height: r.u16()?, flags: r.u16()?,
            photo_id: r.array()?, rgb_root: r.scalar()?, evidence_root: r.scalar()?, policy_id: r.array()?,
            circuit_id: r.array()?, account_id: r.array()?,
        };
        if result.flags & 0xfff0 != 0 { return Err(WireError::Invalid("reserved CAPTURE flags")); }
        if result.flags > 1 { return Err(WireError::Invalid("CAPTURE image profile")); }
        r.done()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureMemo { pub core: CaptureCore, pub assertion: Vec<u8> }

impl CaptureMemo {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(CAPTURE_CORE_LEN + 2 + self.assertion.len(), b"");
        self.core.write_body(&mut out);
        out.blob16(&self.assertion);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let (core, tail) = bytes.split_at_checked(CAPTURE_CORE_LEN).ok_or(WireError::Truncated)?;
        let core = CaptureCore::decode(core)?;
        let mut r = Reader(tail);
        let assertion = r.blob16(1, MAX_ASSERTION_LEN)?;
        // 0xF0 cannot start a CBOR map, so the dev-unattested form is unambiguous.
        if assertion[0] == DEV_ASSERTION_MARKER && assertion.len() != DEV_ASSERTION_LEN {
            return Err(WireError::Invalid("dev-unattested assertion length"));
        }
        r.done()?;
        Ok(Self { core, assertion: assertion.to_vec() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofMemo {
    pub network: u32,
    pub manifest_tx: Hash32,
    pub selection_ledger_index: u32,
    pub selection_ledger_hash: Hash32,
    pub circuit_id: Hash32,
    /// Opaque proof bytes; `galaxy-crypto::groth16::Proof::from_bytes` validates the points.
    pub proof: [u8; PROOF_LEN],
}

impl ProofMemo {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(361, b"");
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.manifest_tx);
        out.raw(&self.selection_ledger_index.to_be_bytes());
        out.raw(&self.selection_ledger_hash);
        out.raw(&self.circuit_id);
        out.raw(&self.proof);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let result = Self { network: r.network()?, manifest_tx: r.array()?, selection_ledger_index: r.u32()?,
            selection_ledger_hash: r.array()?, circuit_id: r.array()?, proof: r.array()? };
        r.done()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkMemo { pub bundle_id: Hash32, pub index: u16, pub total: u16, pub payload: Vec<u8> }

impl ChunkMemo {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(39 + self.payload.len(), b"");
        out.raw(&self.bundle_id);
        out.raw(&self.index.to_be_bytes());
        out.raw(&self.total.to_be_bytes());
        out.blob16(&self.payload);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let bundle_id = r.array()?;
        let (index, total) = (r.u16()?, r.u16()?);
        if index >= total { return Err(WireError::Invalid("chunk index or total")); }
        let payload = r.blob16(1, MAX_CHUNK_PAYLOAD)?;
        r.done()?;
        Ok(Self { bundle_id, index, total, payload: payload.to_vec() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceManifest {
    pub network: u32,
    pub bundle_id: Hash32,
    pub bundle_len: u32,
    pub chunk_total: u16,
    pub first_chunk_ledger: u32,
    pub last_chunk_ledger: u32,
    pub kind: DeviceKind,
    pub env: AttestationEnv,
    pub key_id: Hash32,
    pub account_id: AccountId,
    pub reg_ledger_index: u32,
    pub reg_ledger_hash: Hash32,
}

impl DeviceManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(141, b"");
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.bundle_id);
        out.raw(&self.bundle_len.to_be_bytes());
        out.raw(&self.chunk_total.to_be_bytes());
        out.raw(&self.first_chunk_ledger.to_be_bytes());
        out.raw(&self.last_chunk_ledger.to_be_bytes());
        out.raw(&[self.kind as u8, self.env as u8]);
        out.raw(&self.key_id);
        out.raw(&self.account_id);
        out.raw(&self.reg_ledger_index.to_be_bytes());
        out.raw(&self.reg_ledger_hash);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let network = r.network()?;
        let bundle_id = r.array()?;
        let (bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger) = r.chunk_span()?;
        let (kind, env) = device_type(&mut r)?;
        let result = Self { network, bundle_id, bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger, kind, env,
            key_id: r.array()?, account_id: r.array()?, reg_ledger_index: r.u32()?, reg_ledger_hash: r.array()? };
        r.done()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceBundle {
    pub kind: DeviceKind,
    pub env: AttestationEnv,
    pub app_id: String,
    pub key_id: Hash32,
    pub network: u32,
    pub reg_ledger_index: u32,
    pub reg_ledger_hash: Hash32,
    pub reg_nonce: Hash32,
    pub account_id: AccountId,
    pub attestation: Vec<u8>,
}

impl DeviceBundle {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(137 + self.app_id.len() + self.attestation.len(), b"GXDB");
        out.raw(&[self.kind as u8, self.env as u8]);
        out.blob16(self.app_id.as_bytes());
        out.raw(&self.key_id);
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.reg_ledger_index.to_be_bytes());
        out.raw(&self.reg_ledger_hash);
        out.raw(&self.reg_nonce);
        out.raw(&self.account_id);
        out.blob32(&self.attestation);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        check_bundle_len(bytes)?;
        let mut r = Reader::new(bytes, b"GXDB")?;
        let (kind, env) = device_type(&mut r)?;
        let app_id = std::str::from_utf8(r.blob16(0, usize::from(u16::MAX))?).map_err(|_| WireError::Invalid("app ID"))?;
        if !(valid_app_id(app_id) || kind == DeviceKind::DevUnattested && app_id.is_empty()) {
            return Err(WireError::Invalid("app ID"));
        }
        let key_id = r.array()?;
        let network = r.network()?;
        let reg_ledger_index = r.u32()?;
        let reg_ledger_hash = r.array()?;
        let reg_nonce = r.array()?;
        let account_id = r.array()?;
        let attestation = r.blob32()?;
        if kind == DeviceKind::DevUnattested && attestation.len() != 32 { return Err(WireError::Invalid("dev public key length")); }
        r.done()?;
        Ok(Self { kind, env, app_id: app_id.to_owned(), key_id, network, reg_ledger_index, reg_ledger_hash,
            reg_nonce, account_id, attestation: attestation.to_vec() })
    }
}

pub(crate) fn valid_app_id(app_id: &str) -> bool {
    app_id.contains('.') && app_id.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitManifest {
    pub network: u32,
    pub bundle_id: Hash32,
    pub bundle_len: u32,
    pub chunk_total: u16,
    pub first_chunk_ledger: u32,
    pub last_chunk_ledger: u32,
    pub circuit_id: Hash32,
    pub policy_id: Hash32,
    pub vk_digest: Hash32,
}

impl CircuitManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(147, b"");
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.bundle_id);
        out.raw(&self.bundle_len.to_be_bytes());
        out.raw(&self.chunk_total.to_be_bytes());
        out.raw(&self.first_chunk_ledger.to_be_bytes());
        out.raw(&self.last_chunk_ledger.to_be_bytes());
        out.raw(&self.circuit_id);
        out.raw(&self.policy_id);
        out.raw(&self.vk_digest);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let network = r.network()?;
        let bundle_id = r.array()?;
        let (bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger) = r.chunk_span()?;
        let result = Self { network, bundle_id, bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger,
            circuit_id: r.array()?, policy_id: r.array()?, vk_digest: r.array()? };
        r.done()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitBundle { pub descriptor_json: Vec<u8>, pub vk_bytes: Vec<u8>, pub policy_json: Vec<u8> }

impl CircuitBundle {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(17 + self.descriptor_json.len() + self.vk_bytes.len() + self.policy_json.len(), b"GXCB");
        out.blob32(&self.descriptor_json);
        out.blob32(&self.vk_bytes);
        out.blob32(&self.policy_json);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        check_bundle_len(bytes)?;
        let mut r = Reader::new(bytes, b"GXCB")?;
        let descriptor_json = r.blob32()?;
        let vk_bytes = r.blob32()?;
        let policy_json = r.blob32()?;
        r.done()?;
        canonical_json::validate_object(descriptor_json)?;
        canonical_json::validate_object(policy_json)?;
        Ok(Self { descriptor_json: descriptor_json.to_vec(), vk_bytes: vk_bytes.to_vec(), policy_json: policy_json.to_vec() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevokeMemo { pub network: u32, pub registration_tx: Hash32, pub reason: u8, pub effective_ledger: u32 }

impl RevokeMemo {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(42, b"");
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.registration_tx);
        out.raw(&[self.reason]);
        out.raw(&self.effective_ledger.to_be_bytes());
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let result = Self { network: r.network()?, registration_tx: r.array()?, reason: r.u8()?, effective_ledger: r.u32()? };
        r.done()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAnnouncement {
    pub network: u32,
    pub node_pubkey: Hash32,
    pub expires_at: u64,
    pub url: String,
    pub sig: [u8; 64],
}

impl NodeAnnouncement {
    fn write_payload(&self, out: &mut Writer) {
        out.raw(&self.network.to_be_bytes());
        out.raw(&self.node_pubkey);
        out.raw(&self.expires_at.to_be_bytes());
        out.blob16(self.url.as_bytes());
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(111 + self.url.len(), b"");
        self.write_payload(&mut out);
        out.raw(&self.sig);
        out.0
    }

    pub fn signing_message(&self) -> Vec<u8> {
        let mut out = Writer::new(ids::NODE_DOMAIN.len() + 47 + self.url.len(), ids::NODE_DOMAIN);
        self.write_payload(&mut out);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, b"")?;
        let network = r.network()?;
        let node_pubkey = r.array()?;
        let expires_at = r.u64()?;
        let url = r.blob16(1, usize::from(u16::MAX))?;
        if !url.iter().all(u8::is_ascii_graphic) { return Err(WireError::Invalid("NODE URL")); }
        let sig = r.array()?;
        r.done()?;
        Ok(Self { network, node_pubkey, expires_at, url: std::str::from_utf8(url).unwrap().to_owned(), sig })
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: &[u8]) -> Result<Self, WireError> {
        let mut r = Self(bytes);
        if r.take(magic.len())? != magic { return Err(WireError::Invalid("bundle magic")); }
        let version = r.u8()?;
        if version != VERSION { return Err(WireError::Version(version)); }
        Ok(r)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let (bytes, rest) = self.0.split_at_checked(len).ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> { Ok(self.take(N)?.try_into().unwrap()) }
    fn u8(&mut self) -> Result<u8, WireError> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16, WireError> { Ok(u16::from_be_bytes(self.array()?)) }
    fn u32(&mut self) -> Result<u32, WireError> { Ok(u32::from_be_bytes(self.array()?)) }
    fn u64(&mut self) -> Result<u64, WireError> { Ok(u64::from_be_bytes(self.array()?)) }

    fn network(&mut self) -> Result<u32, WireError> {
        let n = self.u32()?;
        if n > 2 { return Err(WireError::Invalid("network ID")); }
        Ok(n)
    }

    fn scalar(&mut self) -> Result<Hash32, WireError> {
        let bytes = self.array()?;
        if bytes >= SCALAR_MODULUS { return Err(WireError::Invalid("non-canonical BN254 scalar")); }
        Ok(bytes)
    }

    fn blob16(&mut self, min: usize, max: usize) -> Result<&'a [u8], WireError> {
        let len = usize::from(self.u16()?);
        if !(min..=max).contains(&len) { return Err(WireError::Invalid("payload length")); }
        self.take(len)
    }

    fn blob32(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.u32()? as usize;
        if !(1..=MAX_BUNDLE_LEN).contains(&len) { return Err(WireError::Invalid("bundle payload length")); }
        self.take(len)
    }

    fn chunk_span(&mut self) -> Result<(u32, u16, u32, u32), WireError> {
        let (len, total, first, last) = (self.u32()?, self.u16()?, self.u32()?, self.u32()?);
        if len == 0 || len as usize > MAX_BUNDLE_LEN || total == 0 || u32::from(total) > len
            || len > u32::from(total) * MAX_CHUNK_PAYLOAD as u32 || first > last
        {
            return Err(WireError::Invalid("bundle chunk span"));
        }
        Ok((len, total, first, last))
    }

    fn done(self) -> Result<(), WireError> {
        if !self.0.is_empty() { return Err(WireError::Trailing); }
        Ok(())
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn new(capacity: usize, prefix: &[u8]) -> Self {
        let mut out = Self(Vec::with_capacity(capacity));
        out.raw(prefix);
        out.raw(&[VERSION]);
        out
    }

    fn raw(&mut self, bytes: &[u8]) { self.0.extend_from_slice(bytes); }
    fn blob16(&mut self, bytes: &[u8]) {
        self.raw(&u16::try_from(bytes.len()).expect("wire value must fit u16 length").to_be_bytes());
        self.raw(bytes);
    }
    fn blob32(&mut self, bytes: &[u8]) {
        self.raw(&u32::try_from(bytes.len()).expect("wire value must fit u32 length").to_be_bytes());
        self.raw(bytes);
    }
}

fn check_bundle_len(bytes: &[u8]) -> Result<(), WireError> {
    if bytes.len() > MAX_BUNDLE_LEN { return Err(WireError::Invalid("bundle length")); }
    Ok(())
}
