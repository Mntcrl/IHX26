//! Ledger-derived stratified tile and in-circuit point selection (§7).
use crate::{
    field::hash_limbs,
    layout::{CENTRAL_TILES, PERIPHERAL_TILES},
    poseidon,
    tags::{self, tag},
    Fr,
};
use ark_ff::PrimeField;
use sha2::{Digest, Sha256};

pub const SAMPLED_TILES: usize = 16;
const PICKS_PER_STRATUM: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotPicks {
    pub blocks: [(u8, u8); 4],
    pub temporal: [(u8, u8); 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub seed: [u8; 32],
    pub tiles: [u8; SAMPLED_TILES],
    pub ctx: Fr,
    pub picks: [SlotPicks; SAMPLED_TILES],
    pub hyp: [[u16; 3]; 8],
}

impl Challenge {
    pub fn derive(network_id: u32, capture_id: &[u8; 32], lsel: u32, lsel_hash: &[u8; 32]) -> Self {
        let seed = seed(network_id, capture_id, lsel, lsel_hash);
        let tiles = select_tiles(&seed);
        let ctx = ctx(capture_id, &seed);
        let picks = std::array::from_fn(|s| slot_picks(s, &ctx, tiles[s]));
        let hyp = hypothesis_indices(&ctx);
        Self { seed, tiles, ctx, picks, hyp }
    }
}

pub fn seed(network_id: u32, capture_id: &[u8; 32], lsel: u32, lsel_hash: &[u8; 32]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"GALAXY/SAMPLE/v1\0");
    sha.update(network_id.to_be_bytes());
    sha.update(capture_id);
    sha.update(lsel.to_be_bytes());
    sha.update(lsel_hash);
    sha.finalize().into()
}

pub fn tile_stream_block(seed: &[u8; 32], counter: u32) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"GALAXY/SAMPLE-TILES/v1\0");
    sha.update(seed);
    sha.update(counter.to_be_bytes());
    sha.finalize().into()
}

pub fn select_tiles(seed: &[u8; 32]) -> [u8; SAMPLED_TILES] {
    let mut words = (0..=u32::MAX).flat_map(|counter| {
        let bytes = tile_stream_block(seed, counter);
        std::array::from_fn::<_, 8, _>(|i| u32::from_be_bytes(bytes[4*i..4*i+4].try_into().unwrap()))
    });
    select_from_words(|| words.next().expect("32-bit counter exhausted"))
}

fn select_from_words(mut word: impl FnMut() -> u32) -> [u8; SAMPLED_TILES] {
    let mut central = CENTRAL_TILES;
    let mut peripheral = PERIPHERAL_TILES;
    let mut out = [0; SAMPLED_TILES];
    let (c, p) = out.split_at_mut(PICKS_PER_STRATUM);
    partial_shuffle(&mut central, c, &mut word);
    partial_shuffle(&mut peripheral, p, &mut word);
    c.sort_unstable();
    p.sort_unstable();
    out
}

fn partial_shuffle(list: &mut [u8], picks: &mut [u8], word: &mut impl FnMut() -> u32) {
    for (i, pick) in picks.iter_mut().enumerate() {
        let n = (list.len() - i) as u64;
        let limit = ((1u64 << 32) / n) * n;
        let offset = loop {
            let x = u64::from(word());
            if x < limit {
                break (x % n) as usize;
            }
        };
        list.swap(i, i + offset);
        *pick = list[i];
    }
}

pub fn ctx(capture_id: &[u8; 32], seed: &[u8; 32]) -> Fr {
    let (capture_hi, capture_lo) = hash_limbs(capture_id);
    let (seed_hi, seed_lo) = hash_limbs(seed);
    poseidon::hash(&[tag(tags::CTX, 0), capture_hi, capture_lo, seed_hi, seed_lo])
}

pub fn slot_hash(slot: usize, ctx: &Fr, tile: u8) -> Fr {
    assert!(slot < SAMPLED_TILES && tile < 48);
    poseidon::hash(&[tag(tags::SAMPLE_TILE, slot as u64), *ctx, Fr::from(tile)])
}

pub fn hypothesis_hash(index: usize, ctx: &Fr) -> Fr {
    assert!(index < 8);
    poseidon::hash(&[tag(tags::HYP, index as u64), *ctx])
}

/// Exact canonical little-endian integer bits, with no modular aliases.
pub fn canonical_le_bits(value: &Fr) -> [u8; 254] {
    let words = value.into_bigint().0;
    std::array::from_fn(|i| ((words[i / 64] >> (i % 64)) & 1) as u8)
}

pub fn slot_picks(slot: usize, ctx: &Fr, tile: u8) -> SlotPicks {
    // Only the low 24 canonical bits are consumed.
    let bits = slot_hash(slot, ctx, tile).into_bigint().0[0] as u32;
    SlotPicks {
        blocks: std::array::from_fn(|q| {
            let nibble = (bits >> (4*q)) as u8;
            (4*(q as u8 % 2) + (nibble & 3), 4*(q as u8 / 2) + ((nibble >> 2) & 3))
        }),
        temporal: std::array::from_fn(|q| {
            let pick = (bits >> (16 + 2*q)) as u8;
            (1 + (pick & 1), 1 + ((pick >> 1) & 1))
        }),
    }
}

pub fn hypothesis_indices(ctx: &Fr) -> [[u16; 3]; 8] {
    std::array::from_fn(|j| {
        let bits = hypothesis_hash(j, ctx).into_bigint().0[0];
        let width = if j < 4 { 8 } else { 7 };
        let mask = (1 << width) - 1;
        std::array::from_fn(|m| ((bits >> (width*m)) & mask) as u16)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_and_single_remaining_item() {
        let mut input = [0xffff_ffff, 0xffff_fffe, 0xffff_fffd, 0xffff_ffff, 0xffff_ffff].into_iter();
        let mut list = [10, 11, 12];
        let mut picks = [0; 3];
        // n=3 rejects x == limit, accepts x == limit-1; n=2 and n=1 accept u32::MAX.
        partial_shuffle(&mut list, &mut picks, &mut || input.next().unwrap());
        assert_eq!(picks, [12, 10, 11]);
        assert_eq!(input.next(), Some(u32::MAX));
    }

    #[test]
    fn rejects_greater_than_limit_and_continues_stream_across_strata() {
        let mut input = [0, u32::MAX, 15].into_iter().chain((3..=16).map(|i| i * 1000));
        let picks = select_from_words(|| input.next().unwrap());
        assert!(input.next().is_none());
        assert_eq!(picks[0], 10);
        assert_eq!(picks[8..].len(), 8);
        assert!(picks[..8].windows(2).all(|x| x[0] < x[1]));
        assert!(picks[8..].windows(2).all(|x| x[0] < x[1]));
        assert!(picks[..8].iter().all(|x| CENTRAL_TILES.contains(x)));
        assert!(picks[8..].iter().all(|x| PERIPHERAL_TILES.contains(x)));
    }
}
