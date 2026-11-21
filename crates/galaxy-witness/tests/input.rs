//! Circuit input round trip on synthetic (development-only) evidence. The
//! circuit-level witness check runs in `scripts/circuit.sh e2e`.

use galaxy_crypto::{
    field::{fr_from_dec, hash_limbs},
    layout::WIDTH,
    sampling::Challenge,
    tree::{FrameTree, TreeKind},
    Fr,
};
use galaxy_evidence::{commit, Evidence, Salts};
use galaxy_geometry::{
    build_view, evaluate,
    synthetic::{synthesize, Scenario},
};
use galaxy_protocol::{policy::Policy, Hash32};
use galaxy_witness::{build_input, build_input_from_evidence, public_inputs, WitnessError};
use serde_json::Value;

const NETWORK: u32 = 1;
const LSEL: u32 = 1_000_005;

fn policy() -> Policy {
    Policy::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../policies/galaxy-pc-v1m.json"
    ))
    .unwrap()
}

fn capture_id(seed: u64) -> Hash32 {
    let mut id = [0x5a; 32];
    id[24..].copy_from_slice(&seed.to_be_bytes());
    id
}

fn salts() -> Salts {
    let f = |i: u64| Fr::from(i * 0x1_0000_0001 + 7);
    Salts {
        depth0: f(1),
        frame_minus: f(2),
        frame_plus: f(3),
        camera: f(4),
        imu: f(5),
        bluetooth: f(6),
        noise: f(7),
        meta: f(8),
    }
}

/// First synthetic room seed whose reference verdict passes for its challenge.
fn passing_room() -> (Evidence, Challenge, Hash32) {
    let policy = policy();
    for seed in 0..32 {
        let evidence = synthesize(Scenario::Room, seed, &policy).unwrap();
        let id = capture_id(seed);
        let challenge = Challenge::derive(NETWORK, &id, LSEL, &[0x33; 32]);
        if evaluate(&build_view(&evidence, &challenge).unwrap(), &policy).pass {
            return (evidence, challenge, id);
        }
    }
    panic!("no passing synthetic room seed");
}

fn dec(v: &Value) -> Fr {
    fr_from_dec(v.as_str().expect("decimal string")).expect("canonical decimal")
}

fn shape(v: &Value) -> Vec<usize> {
    match v {
        Value::Array(items) => {
            let inner = shape(&items[0]);
            assert!(items.iter().all(|x| shape(x) == inner), "ragged array");
            [vec![items.len()], inner].concat()
        }
        Value::String(_) => vec![],
        other => panic!("unexpected JSON value {other}"),
    }
}

fn leaves(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(items) => items.iter().flat_map(leaves).collect(),
        x => vec![x],
    }
}

#[test]
fn input_matches_schema_and_verifier_public_inputs() {
    let (evidence, challenge, id) = passing_room();
    let salts = salts();
    let commitments = commit(&evidence, &salts);
    let input =
        build_input_from_evidence(&evidence, &salts, &commitments, &challenge, &id).unwrap();

    // Exact INPUTS.md schema (key set and array shapes); every value is a canonical decimal.
    let expected: &[(&str, &[usize])] = &[
        ("rgb_root", &[]),
        ("evidence_root", &[]),
        ("capture_id_hi", &[]),
        ("capture_id_lo", &[]),
        ("seed_hi", &[]),
        ("seed_lo", &[]),
        ("sampled_tiles", &[16]),
        ("roots", &[3]),
        ("salts", &[8]),
        ("camera", &[54]),
        ("imu", &[133]),
        ("bluetooth", &[24]),
        ("noise", &[18]),
        ("meta", &[9]),
        ("blk_rgb", &[16, 4, 2]),
        ("blk_depth", &[16, 4, 2]),
        ("blk_rgb_siblings", &[16, 4, 3, 3]),
        ("blk_depth_siblings", &[16, 4, 3, 3]),
        ("tile_rgb_siblings", &[16, 3, 3]),
        ("tile_depth_siblings", &[16, 3, 3]),
        ("pr_bx", &[64]),
        ("pr_by", &[64]),
        ("pr_rgb", &[64, 2]),
        ("pr_depth", &[64, 2]),
        ("pr_siblings", &[64, 6, 3]),
    ];
    let object = input.as_object().unwrap();
    assert_eq!(object.len(), expected.len());
    for (key, dims) in expected {
        assert_eq!(shape(&object[*key]), *dims, "{key}");
        leaves(&object[*key]).into_iter().for_each(|v| {
            dec(v);
        });
    }

    // What a verifier reconstructs without the witness: rgb_root from the
    // photo, evidence_root from CAPTURE, limbs from the ids, tiles from Lsel.
    let analysis = galaxy_image::analysis_raster(
        &galaxy_image::decode_any(&evidence.image.encoded)
            .unwrap()
            .raster,
    )
    .unwrap();
    assert_eq!(analysis.len(), WIDTH * 192 * 3);
    let rgb_root = FrameTree::build(TreeKind::Rgb, 1, &analysis, &[], &[])
        .unwrap()
        .root();
    let verifier_challenge = Challenge::derive(NETWORK, &id, LSEL, &[0x33; 32]);
    let (capture_hi, capture_lo) = hash_limbs(&id);
    let (seed_hi, seed_lo) = hash_limbs(&verifier_challenge.seed);
    let mut publics = vec![
        rgb_root,
        commitments.evidence_root,
        capture_hi,
        capture_lo,
        seed_hi,
        seed_lo,
    ];
    publics.extend(verifier_challenge.tiles.map(Fr::from));
    let from_input: Vec<Fr> = [
        "rgb_root",
        "evidence_root",
        "capture_id_hi",
        "capture_id_lo",
        "seed_hi",
        "seed_lo",
    ]
    .iter()
    .map(|k| dec(&input[*k]))
    .chain(input["sampled_tiles"].as_array().unwrap().iter().map(dec))
    .collect();
    assert_eq!(from_input, publics);
    assert_eq!(
        public_inputs(&commitments, &id, &challenge).to_vec(),
        publics
    );
    assert_eq!(dec(&input["roots"][0]), commitments.depth_tree.root());
}

#[test]
fn inconsistent_arguments_fail_closed() {
    let (evidence, challenge, id) = passing_room();
    let salts = salts();
    let commitments = commit(&evidence, &salts);
    let view = build_view(&evidence, &challenge).unwrap();
    assert!(build_input(&evidence, &salts, &commitments, &challenge, &id, &view).is_ok());

    // Challenge for another capture id (ctx and picks no longer match).
    let other_id = capture_id(999);
    assert_eq!(
        build_input(
            &evidence,
            &salts,
            &commitments,
            &challenge,
            &other_id,
            &view
        )
        .unwrap_err(),
        WitnessError::Challenge
    );
    let mut edited = challenge.clone();
    edited.picks[3].blocks[1].0 ^= 1;
    assert_eq!(
        build_input(&evidence, &salts, &commitments, &edited, &id, &view).unwrap_err(),
        WitnessError::Challenge
    );

    // View built for a different challenge.
    let other = Challenge::derive(NETWORK, &id, LSEL + 1, &[0x44; 32]);
    let other_view = build_view(&evidence, &other).unwrap();
    assert_eq!(
        build_input(
            &evidence,
            &salts,
            &commitments,
            &challenge,
            &id,
            &other_view
        )
        .unwrap_err(),
        WitnessError::View
    );
    let mut moved = view.clone();
    let p = (0..64)
        .find(|&p| moved.pairs[p].inb)
        .expect("an in-bounds pair");
    moved.pr_bx[p] = (moved.pr_bx[p] + 1) % 64;
    moved.pairs[p].inb = false;
    assert_eq!(
        build_input(&evidence, &salts, &commitments, &challenge, &id, &moved).unwrap_err(),
        WitnessError::View
    );

    // Commitments made with different salts, and salts outside 248 bits.
    let mut changed = salts.clone();
    changed.imu += Fr::from(1u8);
    assert_eq!(
        build_input(&evidence, &changed, &commitments, &challenge, &id, &view).unwrap_err(),
        WitnessError::Commitment
    );
    let mut wide = salts.clone();
    wide.noise = -Fr::from(1u8);
    assert_eq!(
        build_input(&evidence, &wide, &commitments, &challenge, &id, &view).unwrap_err(),
        WitnessError::SaltDomain
    );

    // Commitments of different evidence (one depth value changed inside a sampled block).
    let mut other_evidence = evidence.clone();
    let (tx, ty) = galaxy_crypto::layout::tile_xy(challenge.tiles[0]);
    let (bx, by) = challenge.picks[0].blocks[0];
    let i = (usize::from(ty) * 32 + usize::from(by) * 4 + 1) * WIDTH
        + usize::from(tx) * 32
        + usize::from(bx) * 4
        + 1;
    other_evidence.frames[1].depth_mm[i] ^= 1;
    let other_commitments = commit(&other_evidence, &salts);
    assert_eq!(
        build_input(
            &evidence,
            &salts,
            &other_commitments,
            &challenge,
            &id,
            &view
        )
        .unwrap_err(),
        WitnessError::Commitment
    );

    // Out-of-domain evidence is rejected, never clamped.
    let mut wide_evidence = evidence.clone();
    wide_evidence.camera[0].fx = 1 << 26;
    assert_eq!(
        build_input(&wide_evidence, &salts, &commitments, &challenge, &id, &view).unwrap_err(),
        WitnessError::EvidenceDomain
    );

    // A self-consistent root with a changed padding slot cannot match the circuit.
    let mut padded = commitments;
    padded.components[9] += Fr::from(1u8);
    padded.evidence_root = galaxy_crypto::commit::evidence_root(&padded.components);
    assert_eq!(
        build_input(&evidence, &salts, &padded, &challenge, &id, &view).unwrap_err(),
        WitnessError::Commitment
    );
}
