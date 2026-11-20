//! Domain-separated SHA-256 identifiers and attestation messages (§3, §11).
use crate::{canonical_json, sha256, AccountId, Hash32};

pub const NODE_DOMAIN: &[u8] = b"GALAXY/NODE/v1\0";
const ASSERT_DOMAIN: &[u8] = b"GALAXY/ASSERT/v1\0";
const DEV_ASSERT_DOMAIN: &[u8] = b"GALAXY/DEV-ASSERT/v1\0";

pub fn photo_id(w: u32, h: u32, rgb: &[u8]) -> Hash32 {
    sha256(&[b"GALAXY/PHOTO/v1\0", &w.to_be_bytes(), &h.to_be_bytes(), rgb])
}

/// `core299` must be the encoded or decoded-and-validated CAPTURE core, not a whole memo.
pub fn capture_id(core299: &[u8]) -> Hash32 {
    sha256(&[b"GALAXY/CAPTURE/v1\0", core299])
}

pub fn assertion_client_data(core299: &[u8]) -> Vec<u8> {
    prefixed(ASSERT_DOMAIN, core299)
}

pub fn assertion_client_data_hash(core299: &[u8]) -> Hash32 {
    sha256(&[ASSERT_DOMAIN, core299])
}

pub fn registration_client_data_hash(
    network_id: u32,
    reg_ledger_index: u32,
    reg_ledger_hash: &Hash32,
    reg_nonce: &Hash32,
    key_id: &Hash32,
    account_id: &AccountId,
) -> Hash32 {
    sha256(&[
        b"GALAXY/DEVICE-REG/v1\0", &network_id.to_be_bytes(), &reg_ledger_index.to_be_bytes(),
        reg_ledger_hash, reg_nonce, key_id, account_id,
    ])
}

pub fn dev_assertion_message(core299: &[u8]) -> Vec<u8> {
    prefixed(DEV_ASSERT_DOMAIN, core299)
}

pub fn dev_key_id(public_key: &Hash32) -> Hash32 {
    sha256(&[b"GALAXY/DEV-KEY/v1\0", public_key])
}

pub fn bundle_id(bundle: &[u8]) -> Hash32 {
    sha256(&[b"GALAXY/BUNDLE/v1\0", bundle])
}

pub fn policy_id(canonical_policy_json: &[u8]) -> Result<Hash32, canonical_json::Error> {
    canonical_json::validate(canonical_policy_json)?;
    Ok(policy_id_canonical(canonical_policy_json))
}

pub(crate) fn policy_id_canonical(canonical_policy_json: &[u8]) -> Hash32 {
    sha256(&[b"GALAXY/POLICY/v1\0", canonical_policy_json])
}

pub fn circuit_id(canonical_descriptor_json: &[u8]) -> Result<Hash32, canonical_json::Error> {
    canonical_json::validate(canonical_descriptor_json)?;
    Ok(sha256(&[b"GALAXY/CIRCUIT/v1\0", canonical_descriptor_json]))
}

pub fn vk_digest(vk_bytes: &[u8]) -> Hash32 {
    sha256(&[b"GALAXY/VK/v1\0", vk_bytes])
}

/// `descriptor_without_signature` must omit the top-level `signature` property.
pub fn node_descriptor_signature_message(descriptor_without_signature: &[u8]) -> Result<Vec<u8>, canonical_json::Error> {
    let descriptor: serde_json::Value = serde_json::from_slice(descriptor_without_signature)?;
    if descriptor.as_object().is_none_or(|fields| fields.contains_key("signature")) {
        return Err(canonical_json::Error::NonCanonical);
    }
    if canonical_json::to_vec(&descriptor)? != descriptor_without_signature {
        return Err(canonical_json::Error::NonCanonical);
    }
    Ok(prefixed(NODE_DOMAIN, descriptor_without_signature))
}

pub(crate) fn prefixed(prefix: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(prefix.len() + payload.len());
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(payload);
    bytes
}
