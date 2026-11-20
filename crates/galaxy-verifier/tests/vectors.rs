//! Committed offline vectors in `test-vectors/verifier/dev-unattested/` (used by
//! the CLI tests and other verifier front ends). They are regenerated
//! deterministically by this test; set `GALAXY_WRITE_VECTORS=1` to rewrite them.
//! The image itself is not committed (2.4 MB): it is the PNG of `raster(1)`
//! (`rgb[i] = (31·i + 7·⌊i/3072⌋ + 13) mod 251`) with the XMP described in refs.json.

mod support;

use galaxy_verifier::{recorded::Recording, verify_photo, State};
use std::path::PathBuf;
use support::*;

fn dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-vectors/verifier/dev-unattested"))
}

/// The repository Testnet config with the test inception ledger and the test circuit accepted.
fn network_toml(world: &World) -> String {
    let base = include_str!("../../../deployments/networks/xrpl-testnet.toml");
    let entry = world.config.accepted_circuits[0].clone();
    let circuit = format!(
        "[[accepted_circuits]]\ncircuit_id = \"{}\"\nvk_digest = \"{}\"\npolicy_id = \"{}\"\nregistration_tx = \"{}\"\n",
        hex::encode(entry.circuit_id), hex::encode(entry.vk_digest), hex::encode(entry.policy_id),
        hex::encode_upper(entry.registration_tx),
    );
    let text = base
        .replace("inception_ledger = 22480444", &format!("inception_ledger = {INCEPTION}"))
        .replace("accepted_circuits = []\n", "");
    format!("{text}\n# Offline test vector: development circuit (tests/support), never a real trust anchor.\n{circuit}")
}

#[tokio::test]
async fn dev_unattested_vectors_are_current() {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    let proof_tx = world.publish_proof(&capture, PROOF_LEDGER);
    let image = galaxy_image::embed_xmp(&png(&rgb), &world.xmp(&capture, proof_tx)).unwrap();
    let recording = world.chain.recording().to_json();
    let network = network_toml(&world);
    let parsed = galaxy_protocol::network::NetworkConfig::from_toml(&network).unwrap();
    assert_eq!(parsed.accepted_circuits, world.config.accepted_circuits);

    let ledger = Recording::from_json(recording.as_bytes()).unwrap().into_ledger();
    let report = verify_photo(&image, None, &world.ctx(&ledger)).await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    let refs = serde_json::json!({
        "manifest_tx": hex::encode_upper(capture.hash), "proof_tx": hex::encode_upper(proof_tx),
        "capture_id": hex::encode(capture.capture_id), "photo_id": hex::encode(capture.core.photo_id),
        "circuit_id": hex::encode(world.ids.circuit_id), "policy_id": hex::encode(world.ids.policy_id),
        "expected_state": "UNSUPPORTED", "expected_exit_code": 60, "image_sha256": hex::encode(sha256(&image)),
    });
    let refs = format!("{}\n", serde_json::to_string_pretty(&refs).unwrap());
    let files: [(&str, &[u8]); 3] = [
        ("recording.json", recording.as_bytes()), ("network.toml", network.as_bytes()), ("refs.json", refs.as_bytes()),
    ];
    let dir = dir();
    if std::env::var_os("GALAXY_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
    }
    for (name, bytes) in files {
        let stored = std::fs::read(dir.join(name)).unwrap_or_else(|err| panic!("{name}: {err}; run with GALAXY_WRITE_VECTORS=1"));
        assert!(stored == bytes, "{name} is stale; run with GALAXY_WRITE_VECTORS=1");
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}