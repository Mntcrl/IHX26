//! Galaxy v1 wire formats (CAPTURE, PROOF, CHUNK, DEVICE, CIRCUIT, REVOKE, NODE memos), SHA-256 identifiers, canonical JSON, policy struct and `policy_id`, network configuration and the XMP model (spec §2, §3, §10, §12).

pub mod canonical_json;
pub mod ids;
pub mod network;
pub mod policy;
pub mod wire;
pub mod xmp;

pub type Hash32 = [u8; 32];
pub type AccountId = [u8; 20];

pub(crate) fn sha256(parts: &[&[u8]]) -> Hash32 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}
