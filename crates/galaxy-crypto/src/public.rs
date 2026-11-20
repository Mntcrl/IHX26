//! Shared v1 public statement ordering (§9).
use crate::{field::hash_limbs, Fr};

pub fn public_inputs_v1(
    rgb_root: &Fr,
    evidence_root: &Fr,
    capture_id: &[u8; 32],
    seed: &[u8; 32],
    tiles: &[u8; 16],
) -> [Fr; 22] {
    let (capture_hi, capture_lo) = hash_limbs(capture_id);
    let (seed_hi, seed_lo) = hash_limbs(seed);
    let mut out = [Fr::from(0u8); 22];
    out[..6].copy_from_slice(&[*rgb_root, *evidence_root, capture_hi, capture_lo, seed_hi, seed_lo]);
    for (value, &tile) in out[6..].iter_mut().zip(tiles) {
        *value = Fr::from(tile);
    }
    out
}
