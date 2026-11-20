//! Arity-four Morton frame trees and strictly ordered openings (§6).
use crate::{layout::{morton, HEIGHT, WIDTH}, poseidon, tags::{self, tag}, Fr};
use ark_ff::{AdditiveGroup, BigInt, PrimeField};

const PIXELS: usize = WIDTH * HEIGHT;
const TILE_LEVELS: [usize; 3] = [0, 64, 80];
const TILE_NODES: usize = 84;
const TOP_LEVELS: [usize; 4] = [0, 64, 80, 84];
const TOP_NODES: usize = 85;
const ZERO: Fr = Fr::ZERO;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TreeKind { Rgb = 1, Depth = 2, Frame = 3 }

impl TreeKind {
    fn valid_frame(self, frame: u8) -> bool {
        match self { Self::Rgb | Self::Depth => frame == 1, Self::Frame => frame == 0 || frame == 2 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    #[error("wrong frame for tree kind")]
    Frame,
    #[error("incorrect {0} plane length")]
    Plane(&'static str),
    #[error("depth confidence is greater than 3")]
    Confidence,
    #[error("leaf slot is out of bounds or belongs to a padding tile")]
    Slot,
}

pub fn pack_rgb(rgb: &[u8; 48]) -> (Fr, Fr) {
    fn limb(bytes: &[u8]) -> Fr {
        let words = std::array::from_fn(|i| if i == 3 { 0 } else {
            let j = 16 - i * 8;
            u64::from_be_bytes(bytes[j..j+8].try_into().unwrap())
        });
        Fr::from_bigint(BigInt(words)).expect("192-bit RGB limb is canonical")
    }
    (limb(&rgb[..24]), limb(&rgb[24..]))
}

/// Pack a 4×4 row-major depth/confidence block. Panics on a confidence value > 3;
/// `FrameTree::build` validates untrusted planes before packing.
pub fn pack_depth(depth: &[u16; 16], conf: &[u8; 16]) -> (Fr, Fr) {
    assert!(conf.iter().all(|&c| c <= 3), "confidence must fit in two bits");
    pack_depth_valid(depth, conf)
}

fn pack_depth_valid(depth: &[u16; 16], conf: &[u8; 16]) -> (Fr, Fr) {
    fn limb(depth: &[u16], conf: &[u8]) -> Fr {
        let entry = |i: usize| (u128::from(depth[i]) << 2) | u128::from(conf[i]);
        let high = (0..7).fold(0u128, |n, i| (n << 18) | entry(i));
        let repr = BigInt([((high as u64) << 18) | entry(7) as u64, (high >> 46) as u64, (high >> 110) as u64, 0]);
        Fr::from_bigint(repr).expect("144-bit depth limb is canonical")
    }
    (limb(&depth[..8], &conf[..8]), limb(&depth[8..], &conf[8..]))
}

fn leaf_tag(code: u16, frame: u8, slot: u16) -> Fr {
    assert!(slot < 4096 && frame <= 2);
    tag(code, (u64::from(frame) << 12) | u64::from(slot))
}

pub fn rgb_leaf(frame: u8, slot: u16, rgb: (Fr, Fr)) -> Fr {
    poseidon::hash(&[leaf_tag(tags::RGB_LEAF, frame, slot), rgb.0, rgb.1])
}

pub fn depth_leaf(frame: u8, slot: u16, depth: (Fr, Fr)) -> Fr {
    poseidon::hash(&[leaf_tag(tags::DEPTH_LEAF, frame, slot), depth.0, depth.1])
}

pub fn frame_leaf(frame: u8, slot: u16, rgb: (Fr, Fr), depth: (Fr, Fr)) -> Fr {
    poseidon::hash(&[leaf_tag(tags::FRAME_LEAF, frame, slot), rgb.0, rgb.1, depth.0, depth.1])
}

fn node(kind: TreeKind, frame: u8, level: usize, index: usize, c: &[Fr]) -> Fr {
    let aux = ((kind as u64) << 24) | (u64::from(frame) << 20) | ((level as u64) << 16) | index as u64;
    poseidon::hash(&[tag(tags::NODE, aux), c[0], c[1], c[2], c[3]])
}

fn pad_tile(kind: TreeKind, frame: u8, pos: usize) -> Fr {
    let aux = ((kind as u64) << 16) | (u64::from(frame) << 8) | pos as u64;
    poseidon::hash(&[tag(tags::PAD_TILE, aux), ZERO])
}

#[derive(Clone)]
pub struct FrameTree {
    kind: TreeKind,
    frame: u8,
    // Actual tiles only, in row-major raster order. Padding exists only at level 3.
    tiles: Vec<[Fr; TILE_NODES]>,
    top: [Fr; TOP_NODES],
}

#[derive(Clone, PartialEq, Eq)]
pub struct Opening {
    pub leaf_slot: u16,
    /// Per ascending level, siblings in ascending child order, omitting the path child.
    pub siblings: [[Fr; 3]; 6],
}

impl FrameTree {
    pub fn build(kind: TreeKind, frame: u8, rgb: &[u8], depth_mm: &[u16], conf: &[u8]) -> Result<Self, TreeError> {
        if !kind.valid_frame(frame) { return Err(TreeError::Frame); }
        let has_rgb = kind != TreeKind::Depth;
        let has_depth = kind != TreeKind::Rgb;
        if rgb.len() != if has_rgb { PIXELS * 3 } else { 0 } { return Err(TreeError::Plane("rgb")); }
        let depth_len = if has_depth { PIXELS } else { 0 };
        if depth_mm.len() != depth_len { return Err(TreeError::Plane("depth")); }
        if conf.len() != depth_len { return Err(TreeError::Plane("confidence")); }
        if conf.iter().any(|&c| c > 3) { return Err(TreeError::Confidence); }

        let mut tiles = Vec::with_capacity(48);
        let mut top = [ZERO; TOP_NODES];
        for ty in 0..8u8 {
            for tx in 0..8u8 {
                let pos = usize::from(morton(tx, ty));
                if ty >= 6 {
                    top[pos] = pad_tile(kind, frame, pos);
                    continue;
                }
                let mut tile = [ZERO; TILE_NODES];
                for by in 0..8u8 {
                    for bx in 0..8u8 {
                        let block = usize::from(morton(bx, by));
                        let slot = (pos * 64 + block) as u16;
                        let first = (usize::from(ty) * 32 + usize::from(by) * 4) * WIDTH + usize::from(tx) * 32 + usize::from(bx) * 4;
                        let rgb_l = || {
                            let mut data = [0; 48];
                            copy_block(&mut data, rgb, first, 3);
                            pack_rgb(&data)
                        };
                        let depth_l = || {
                            let (mut d, mut c) = ([0; 16], [0; 16]);
                            copy_block(&mut d, depth_mm, first, 1);
                            copy_block(&mut c, conf, first, 1);
                            pack_depth_valid(&d, &c)
                        };
                        tile[block] = match kind {
                            TreeKind::Rgb => rgb_leaf(frame, slot, rgb_l()),
                            TreeKind::Depth => depth_leaf(frame, slot, depth_l()),
                            TreeKind::Frame => frame_leaf(frame, slot, rgb_l(), depth_l()),
                        };
                    }
                }
                for level in 1..=2 {
                    let count = 64 >> (2 * level);
                    for i in 0..count {
                        let start = TILE_LEVELS[level - 1] + i * 4;
                        tile[TILE_LEVELS[level] + i] = node(kind, frame, level, pos * count + i, &tile[start..start + 4]);
                    }
                }
                top[pos] = node(kind, frame, 3, pos, &tile[80..84]);
                tiles.push(tile);
            }
        }
        for i in 1..=3 {
            for j in 0..(64 >> (2 * i)) {
                let start = TOP_LEVELS[i - 1] + j * 4;
                top[TOP_LEVELS[i] + j] = node(kind, frame, 3 + i, j, &top[start..start + 4]);
            }
        }
        Ok(Self { kind, frame, tiles, top })
    }

    pub fn root(&self) -> Fr { self.top[TOP_NODES - 1] }

    pub fn kind(&self) -> TreeKind { self.kind }

    pub fn frame(&self) -> u8 { self.frame }

    pub fn opening(&self, leaf_slot: u16) -> Result<Opening, TreeError> {
        let tile = &self.tiles[tile_index(leaf_slot)?];
        let slot = usize::from(leaf_slot);
        let siblings = std::array::from_fn(|level| {
            let index = slot >> (level * 2);
            let digit = index & 3;
            let (nodes, base) = if level < 3 {
                (tile.as_slice(), TILE_LEVELS[level] + ((index & (63 >> (2 * level))) & !3))
            } else {
                (self.top.as_slice(), TOP_LEVELS[level - 3] + (index & !3))
            };
            std::array::from_fn(|j| nodes[base + j + usize::from(j >= digit)])
        });
        Ok(Opening { leaf_slot, siblings })
    }
}

impl Opening {
    pub fn root_from_leaf(&self, kind: TreeKind, frame: u8, leaf: Fr) -> Result<Fr, TreeError> {
        if !kind.valid_frame(frame) { return Err(TreeError::Frame); }
        tile_index(self.leaf_slot)?;
        let mut index = usize::from(self.leaf_slot);
        let mut h = leaf;
        for (level, s) in self.siblings.iter().enumerate() {
            let digit = index & 3;
            let children: [Fr; 4] = std::array::from_fn(|j| {
                if j == digit { h } else { s[j - usize::from(j > digit)] }
            });
            index >>= 2;
            h = node(kind, frame, level + 1, index, &children);
        }
        Ok(h)
    }
}

fn tile_index(leaf_slot: u16) -> Result<usize, TreeError> {
    let pos = leaf_slot >> 6;
    if pos >= 64 { return Err(TreeError::Slot); }
    let compact = |x: u16| ((x & 1) | ((x >> 1) & 2) | ((x >> 2) & 4)) as usize;
    let y = compact(pos >> 1);
    if y >= 6 { return Err(TreeError::Slot); }
    Ok(8 * y + compact(pos))
}

fn copy_block<T: Copy, const N: usize>(out: &mut [T; N], raster: &[T], first: usize, channels: usize) {
    let row_len = 4 * channels;
    for (y, row) in out.chunks_exact_mut(row_len).enumerate() {
        let start = (first + y * WIDTH) * channels;
        row.copy_from_slice(&raster[start..start + row_len]);
    }
}
