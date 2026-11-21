//! `galaxy` binary against the committed offline vectors (no network).

use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn vectors() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-vectors/verifier/dev-unattested"))
}

fn refs() -> Value {
    serde_json::from_slice(&std::fs::read(vectors().join("refs.json")).unwrap()).unwrap()
}

fn hash(value: &Value) -> [u8; 32] {
    let mut out = [0u8; 32];
    hex::decode_to_slice(value.as_str().unwrap(), &mut out).unwrap();
    out
}

/// The vector's raster (see crates/galaxy-verifier/tests/vectors.rs).
fn raster() -> Vec<u8> {
    (0..galaxy_image::RGB_LEN).map(|i| ((i * 31 + (i / 3072) * 7 + 13) % 251) as u8).collect()
}

/// The certified PNG of the vector: raster + Galaxy XMP; checked against refs.json.
fn certified_image(tag: &str) -> PathBuf {
    let refs = refs();
    let xmp = galaxy_protocol::xmp::GalaxyXmp {
        protocol: "galaxy/v1".into(), network: "xrpl-testnet".into(), image_profile: "png-v1".into(),
        photo_id: hash(&refs["photo_id"]), capture_id: hash(&refs["capture_id"]),
        manifest_tx: hash(&refs["manifest_tx"]), proof_tx: hash(&refs["proof_tx"]),
    };
    let png = galaxy_image::encode(galaxy_image::ImageProfile::PngV1, &raster()).unwrap();
    let image = galaxy_image::embed_xmp(&png, &xmp).unwrap();
    temp(tag, &image)
}

/// Run `galaxy --recording … --network-config … <args>`; returns (exit code, stdout).
fn galaxy(args: &[&str]) -> (i32, String) {
    let dir = vectors();
    let output = Command::new(env!("CARGO_BIN_EXE_galaxy"))
        .env_remove("GALAXY_NODES")
        .arg("--recording").arg(dir.join("recording.json"))
        .arg("--network-config").arg(dir.join("network.toml"))
        .args(args)
        .output()
        .unwrap();
    (output.status.code().unwrap(), String::from_utf8(output.stdout).unwrap())
}

fn json(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|err| panic!("{err}: {stdout}"))
}

/// A per-test file in the temp directory (tests run in parallel threads of one process).
fn temp(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("galaxy-cli-test-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn verify_direct_reports_dev_credential_as_unsupported() {
    let image = certified_image("direct.png");
    let (code, out) = galaxy(&["--json", "verify", image.to_str().unwrap(), "--no-discovery"]);
    assert_eq!(code, 60, "{out}");
    let report = json(&out);
    assert_eq!(report["version"], "galaxy-report/v1");
    assert_eq!(report["state"], "UNSUPPORTED");
    assert_eq!(report["mode"], "direct");
    assert_eq!(report["proof"]["proof_tx"], refs()["proof_tx"]);
    let groth16 = report["checks"].as_array().unwrap().iter().find(|c| c["id"] == "proof.groth16").unwrap();
    assert_eq!(groth16["status"], "pass");

    let (code, out) = galaxy(&["verify", image.to_str().unwrap()]);
    assert_eq!(code, 60);
    assert!(out.starts_with("UNSUPPORTED: "), "{out}");
    assert!(out.contains("development unattested credential"));
    std::fs::remove_file(image).unwrap();
}

#[test]
fn verify_modified_and_bare_images() {
    let mut rgb = raster();
    let bare = temp("bare.png", &galaxy_image::encode(galaxy_image::ImageProfile::PngV1, &rgb).unwrap());
    let manifest = refs()["manifest_tx"].as_str().unwrap().to_owned();

    let (code, out) = galaxy(&["--json", "verify", bare.to_str().unwrap(), "--no-discovery"]);
    assert_eq!(code, 40, "{out}");
    assert_eq!(json(&out)["state"], "NOT_FOUND");

    let (code, out) = galaxy(&["--json", "verify", bare.to_str().unwrap(), "--manifest-tx", &manifest]);
    assert_eq!(code, 60, "{out}");
    assert_eq!(json(&out)["mode"], "manual");

    rgb[0] ^= 1;
    let modified = temp("modified.png", &galaxy_image::encode(galaxy_image::ImageProfile::PngV1, &rgb).unwrap());
    let (code, out) = galaxy(&["--json", "verify", modified.to_str().unwrap(), "--manifest-tx", &manifest]);
    assert_eq!(code, 20, "{out}");
    assert_eq!(json(&out)["state"], "INVALID");
    for path in [bare, modified] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn configured_lookup_nodes_are_never_used_without_explicit_consent() {
    let png = galaxy_image::encode(galaxy_image::ImageProfile::PngV1, &raster()).unwrap();
    let bare = temp("lookup-consent.png", &png);
    let dir = vectors();
    let run = |opts: &[&str]| Command::new(env!("CARGO_BIN_EXE_galaxy"))
        .env("GALAXY_NODES", "invalid-node-url")
        .arg("--recording").arg(dir.join("recording.json"))
        .arg("--network-config").arg(dir.join("network.toml"))
        .args(["--json", "verify"]).arg(&bare)
        .args(opts).output().unwrap();
    let direct = run(&[]);
    assert_eq!(direct.status.code(), Some(40), "{:?}", direct.stderr);
    let report: Value = serde_json::from_slice(&direct.stdout).unwrap();
    assert_eq!(report["trust"]["lookup_endpoints"], serde_json::json!([]));
    assert_eq!(run(&["--lookup"]).status.code(), Some(2), "requested lookup validates the node URL");
    std::fs::remove_file(bare).unwrap();
}

#[test]
fn history_diagnostics_can_force_a_walk_and_bound_its_steps() {
    let manifest = refs()["manifest_tx"].as_str().unwrap().to_owned();
    let (code, output) = galaxy(&["--json", "--history-pages", "0", "inspect", &manifest]);
    assert_eq!(code, 60, "{output}");
    let report = json(&output);
    let checks = report["checks"].as_array().unwrap();
    let history = checks.iter().find(|check| check["id"] == "history.complete").unwrap();
    assert_eq!(history["status"], "pass");
    assert_eq!(history["detail"].as_str().unwrap().matches("0 account_tx pages").count(), 2);

    let (code, output) = galaxy(&["--json", "--history-pages", "0", "--history-walk", "1", "inspect", &manifest]);
    assert_eq!(code, 50, "{output}");
    assert!(json(&output)["summary"].as_str().unwrap().contains("walk budget exhausted"));

    let (code, output) = galaxy(&["--json", "--history-walk", "65536", "inspect", &manifest]);
    assert_eq!(code, 60, "{output}");
    for out_of_range in ["0", "65537"] {
        let (code, _) = galaxy(&["--history-walk", out_of_range, "inspect", &manifest]);
        assert_eq!(code, 2);
    }
}

#[test]
fn proof_only_commands() {
    let refs = refs();
    let (code, out) = galaxy(&["--json", "inspect", refs["manifest_tx"].as_str().unwrap()]);
    assert_eq!(code, 60, "{out}");
    assert_eq!(json(&out)["mode"], "proof-only");

    let (code, out) = galaxy(&["--json", "proof", "verify", refs["proof_tx"].as_str().unwrap()]);
    assert_eq!(code, 60, "{out}");

    // Not in the recording: unanswered requests are UNAVAILABLE, never absence.
    let (code, out) = galaxy(&["--json", "inspect", &"AB".repeat(32)]);
    assert_eq!(code, 50, "{out}");
}

#[test]
fn circuit_policy_and_photo_commands() {
    let refs = refs();
    let (code, out) = galaxy(&["--json", "circuit", "inspect", refs["circuit_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    let info = json(&out);
    assert_eq!(info["policy_id"], refs["policy_id"]);
    assert_eq!(info["source"], "xrpl-digest-pinned");

    let (code, out) = galaxy(&["--json", "policy", "inspect", refs["policy_id"].as_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(json(&out)["policy"]["name"], "galaxy-pc-v1m");

    let policy = vectors().join("policy.json");
    let (code, out) = galaxy(&["--json", "policy", "inspect", policy.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(json(&out)["policy_id"], refs["policy_id"]);

    let (code, _) = galaxy(&["circuit", "inspect", &"00".repeat(32)]);
    assert_eq!(code, 60);

    let image = certified_image("photo-id.png");
    let (code, out) = galaxy(&["--json", "photo", "id", image.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(json(&out)["photo_id"], refs["photo_id"]);
    std::fs::remove_file(image).unwrap();
}

#[test]
fn usage_errors_exit_2() {
    assert_eq!(galaxy(&["verify"]).0, 2);
    assert_eq!(galaxy(&["inspect", "not-hex"]).0, 2);
    let image = certified_image("usage.png");
    assert_eq!(galaxy(&["verify", image.to_str().unwrap(), "--lookup"]).0, 2);
    assert_eq!(galaxy(&["verify", image.to_str().unwrap(), "--proof-tx", &"AB".repeat(32)]).0, 2);
    std::fs::remove_file(image).unwrap();
}
