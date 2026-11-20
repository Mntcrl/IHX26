//! Domain separator encoding for all v1 Poseidon calls (§6).
use crate::Fr;

pub const RGB_LEAF: u16 = 1;
pub const DEPTH_LEAF: u16 = 2;
pub const FRAME_LEAF: u16 = 3;
pub const NODE: u16 = 4;
pub const PAD_TILE: u16 = 5;
pub const COMPONENT: u16 = 6;
pub const SCALARS: u16 = 7;
pub const EVIDENCE_NODE: u16 = 8;
pub const PAD_EVIDENCE: u16 = 9;
pub const CTX: u16 = 10;
pub const SAMPLE_TILE: u16 = 11;
pub const HYP: u16 = 12;

pub fn tag(code: u16, aux: u64) -> Fr {
    Fr::from((0x4741_4c58u128 << 96) | (1u128 << 80) | (u128::from(code) << 64) | u128::from(aux))
}
