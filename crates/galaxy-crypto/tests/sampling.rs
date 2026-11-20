use galaxy_crypto::{field::{fr_from_dec, fr_to_dec}, public::public_inputs_v1, sampling::*};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors<T> { cases: Vec<T> }

#[derive(Deserialize)]
struct Sample {
    network_id: u32,
    capture_id: String,
    lsel: u32,
    lsel_hash: String,
    seed: String,
    block0: String,
    tiles: [u8; 16],
    ctx: String,
    slots: [Slot; 16],
    hypotheses: [Hypothesis; 8],
}

#[derive(Deserialize)]
struct Slot { h: String, bits: String, blocks: [(u8, u8); 4], temporal: [(u8, u8); 4] }

#[derive(Deserialize)]
struct Hypothesis { h: String, bits: String, indices: [u16; 3] }

fn bytes(s: &str) -> [u8; 32] { hex::decode(s).unwrap().try_into().unwrap() }

#[test]
fn sampling_matches_circomlib_and_sha256() {
    let vs: Vectors<Sample> = serde_json::from_str(include_str!("../../../test-vectors/protocol/sampling.json")).unwrap();
    assert!(vs.cases.len() >= 20);
    for case in vs.cases {
        let derived = Challenge::derive(case.network_id, &bytes(&case.capture_id), case.lsel, &bytes(&case.lsel_hash));
        assert_eq!(derived.seed, bytes(&case.seed));
        assert_eq!(tile_stream_block(&derived.seed, 0), bytes(&case.block0));
        assert_eq!(derived.tiles, case.tiles);
        assert_eq!(fr_to_dec(&derived.ctx), case.ctx);
        for (i, slot) in case.slots.into_iter().enumerate() {
            let h = slot_hash(i, &derived.ctx, case.tiles[i]);
            assert_eq!(fr_to_dec(&h), slot.h);
            assert_eq!(canonical_le_bits(&h).map(|b| b + b'0'), slot.bits.as_bytes());
            assert_eq!(derived.picks[i], SlotPicks { blocks: slot.blocks, temporal: slot.temporal });
        }
        for (j, hyp) in case.hypotheses.into_iter().enumerate() {
            let h = hypothesis_hash(j, &derived.ctx);
            assert_eq!(fr_to_dec(&h), hyp.h);
            assert_eq!(canonical_le_bits(&h).map(|b| b + b'0'), hyp.bits.as_bytes());
            assert_eq!(derived.hyp[j], hyp.indices);
        }
    }
}

#[derive(Deserialize)]
struct Public {
    rgb_root: String, evidence_root: String, capture_id: String, seed: String,
    tiles: [u8; 16], inputs: [String; 22],
}

#[test]
fn public_statement_exact_order() {
    let vs: Vectors<Public> = serde_json::from_str(include_str!("../../../test-vectors/protocol/public.json")).unwrap();
    for v in vs.cases {
        let result = public_inputs_v1(&fr_from_dec(&v.rgb_root).unwrap(), &fr_from_dec(&v.evidence_root).unwrap(), &bytes(&v.capture_id), &bytes(&v.seed), &v.tiles);
        assert_eq!(result.map(|f| fr_to_dec(&f)), v.inputs);
    }
}
