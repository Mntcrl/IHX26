//! The real main_v1m circuit (development single-party setup, CircuitCore
//! artifacts in `test-vectors/verifier/main-v1m/`): a CAPTURE of a synthetic scene
//! committed by circuit_tool, published in an offline history, and a 256-byte
//! Groth16 proof produced by the circuit prover for this history's challenge.

mod support;

use galaxy_crypto::field;
use galaxy_protocol::wire::{CircuitBundle, MemoKind};
use galaxy_verifier::{verify_photo, State, Status};
use std::path::PathBuf;
use support::*;

fn dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-vectors/verifier/main-v1m"))
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
}

struct Scene {
    world: World,
    photo: Vec<u8>,
    capture: CaptureInfo,
}

/// The statement is fully determined by the committed vectors.
fn scene() -> Scene {
    let bundle = CircuitBundle::decode(&read("circuit_bundle.bin")).expect("GXCB");
    let world = World::with_circuit(bundle);
    let commitment: serde_json::Value = serde_json::from_slice(&read("commitment.json")).unwrap();
    let evidence = field::fr_from_dec(commitment["evidence_root"].as_str().unwrap()).unwrap();
    let photo = read("photo.png");
    let rgb = galaxy_image::decode_any(&photo).expect("photo").raster;
    let core = world.core_with(&rgb, 1, field::fr_to_be(&evidence));
    assert_eq!(
        field::fr_to_dec(&field::fr_from_be_canonical(&core.rgb_root).unwrap()),
        commitment["rgb_root"].as_str().unwrap(),
        "photo.png does not match the committed rgb_root",
    );
    let mut world = world;
    let capture = world.publish_capture(core, L0);
    Scene { world, photo, capture }
}

#[test]
fn statement_matches_the_proved_statement() {
    let scene = scene();
    // The values the circuit prover needs to regenerate proof.bin for this history.
    eprintln!("capture_id {} lsel {LSEL} lsel_hash {}", hex::encode(scene.capture.capture_id), hex::encode(ledger_hash(LSEL)));
    let statement: serde_json::Value = serde_json::from_slice(&read("statement.json")).unwrap();
    assert_eq!(statement["capture_id"], hex::encode(scene.capture.capture_id));
    assert_eq!(statement["lsel"], LSEL);
    assert_eq!(statement["lsel_hash"], hex::encode(ledger_hash(LSEL)));
    assert_eq!(statement["network"], NETWORK);
}

#[tokio::test]
async fn real_main_v1m_proof_passes_the_full_pipeline() {
    let mut scene = scene();
    let proof: [u8; 256] = read("proof.bin").try_into().expect("256-byte proof");
    let memo = scene.world.proof_memo(&scene.capture, proof);
    let proof_tx = scene.world.add_device_tx(MemoKind::Proof, &memo, PROOF_LEDGER);
    let image = galaxy_image::embed_xmp(&scene.photo, &scene.world.xmp(&scene.capture, proof_tx)).unwrap();
    let ledger = scene.world.chain.ledger();
    let report = verify_photo(&image, None, &scene.world.ctx(&ledger)).await;
    // Development credential: never VERIFIED, but the proof itself verifies.
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert!(report.summary.contains("Proof status VERIFIED"), "{}", report.summary);
    for id in ["circuit.accepted", "circuit.registration", "ledger.challenge", "proof.binding", "proof.groth16", "policy.lidar_geometry"] {
        assert_eq!(report.check(id).unwrap().status, Status::Pass, "{id}: {report}");
    }

    // Tampered proof bytes: INVALID.
    let mut scene = self::scene();
    let mut tampered = proof;
    tampered[255] ^= 1;
    let memo = scene.world.proof_memo(&scene.capture, tampered);
    let proof_tx = scene.world.add_device_tx(MemoKind::Proof, &memo, PROOF_LEDGER);
    let image = galaxy_image::embed_xmp(&scene.photo, &scene.world.xmp(&scene.capture, proof_tx)).unwrap();
    let ledger = scene.world.chain.ledger();
    let report = verify_photo(&image, None, &scene.world.ctx(&ledger)).await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_eq!(report.check("proof.groth16").unwrap().status, Status::Fail);
}
