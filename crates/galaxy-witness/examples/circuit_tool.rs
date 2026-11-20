//! Development tooling for `scripts/circuit.sh` (not shipped in any app):
//!
//! * `bundle <artifact_dir> <manifest.json> <policy.json>`: VK bytes (§9),
//!   canonical circuit descriptor, circuit_id and the GXCB circuit bundle (§10)
//!   from the dev-setup outputs.
//! * `e2e-input <dir>`: synthetic (development-only) passing room capture;
//!   writes the private circuit input, the canonical photo and the public
//!   statement a verifier starts from.
//! * `verify <dir> <verification_key.bin>`: reconstructs the 22 public inputs
//!   from the photo and the statement only (never from the witness input) and
//!   verifies the snarkjs proof with galaxy-crypto's Groth16 verifier.

use std::{fs, path::{Path, PathBuf}, process::ExitCode};

use galaxy_crypto::{
    field::{fr_from_dec, fr_to_dec},
    groth16::{verify, Proof, VerifyingKey},
    public::public_inputs_v1,
    sampling::Challenge,
    tree::{FrameTree, TreeKind},
};
use galaxy_evidence::{commit, Salts};
use galaxy_geometry::{build_view, evaluate, synthetic::{synthesize, Scenario}};
use galaxy_protocol::{canonical_json, ids, policy::Policy, wire::CircuitBundle, Hash32};
use serde_json::{json, Value};

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

const NAME: &str = "GalaxyPhysicalConsistencyProofV1";
const PROFILE: &str = "galaxy-pc-v1m";
const PUBLIC_LAYOUT: &str = "galaxy-pc-v1-public-22";
const CEREMONY: &str = "development-single-party";
const N_PUBLIC: u32 = 22;
const DEV_NETWORK: u32 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["bundle", dir, manifest, policy] => bundle(Path::new(dir), Path::new(manifest), Path::new(policy)),
        ["e2e-input", dir] => e2e_input(Path::new(dir)),
        ["verify", dir, vk] => verify_dir(Path::new(dir), Path::new(vk)),
        _ => Err("usage: circuit_tool bundle <dir> <manifest> <policy> | e2e-input <dir> | verify <dir> <vk.bin>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("circuit_tool: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)?)
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).ok_or_else(|| format!("missing {key}").into())
}

fn hex_digest(v: &Value, key: &str) -> Result<String> {
    let s = field(v, key)?.as_str().ok_or_else(|| format!("{key} is not a string"))?;
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).map_err(|_| format!("{key} is not a 32-byte hex digest"))?;
    Ok(hex::encode(out))
}

fn bundle(dir: &Path, manifest_path: &Path, policy_path: &Path) -> Result<()> {
    let build = read_json(&dir.join("build.json"))?;
    let setup = read_json(&dir.join("setup.json"))?;
    let r1cs = read_json(&dir.join("r1cs-info.json"))?;
    if hex_digest(&build, "r1cs_sha256")? != hex_digest(&setup, "r1cs_sha256")? {
        return Err("setup was run on a different R1CS".into());
    }
    let n_public = field(&r1cs, "n_public_inputs")?.as_u64().ok_or("n_public_inputs")?
        + field(&r1cs, "n_public_outputs")?.as_u64().ok_or("n_public_outputs")?;
    if n_public != u64::from(N_PUBLIC) || field(&r1cs, "n_public_outputs")?.as_u64() != Some(0) {
        return Err("R1CS must have exactly 22 public inputs and no public outputs".into());
    }
    let n_constraints = field(&r1cs, "n_constraints")?.as_u64().ok_or("n_constraints")?;

    let vk = VerifyingKey::from_snarkjs_json(&fs::read(dir.join("verification_key.json"))?)?;
    if vk.n_public() != N_PUBLIC as usize {
        return Err("verification key does not have 22 public inputs".into());
    }
    let vk_bytes = vk.to_bytes();
    if VerifyingKey::from_bytes(&vk_bytes)?.to_bytes() != vk_bytes {
        return Err("VK bytes do not round-trip".into());
    }
    let vk_digest = ids::vk_digest(&vk_bytes);

    let policy = Policy::load(policy_path)?;
    let policy_json = policy.canonical_json()?;
    let policy_id = policy.id()?;

    let descriptor = json!({
        "name": NAME,
        "profile": PROFILE,
        "version": 1,
        "policy_id": hex::encode(policy_id),
        "circom_version": field(&build, "circom_version")?.as_str().ok_or("circom_version")?,
        "source_sha256": hex_digest(&build, "source_sha256")?,
        "r1cs_sha256": hex_digest(&build, "r1cs_sha256")?,
        "wasm_sha256": hex_digest(&build, "wasm_sha256")?,
        "witness_graph_sha256": hex_digest(&build, "witness_graph_sha256")?,
        "zkey_sha256": hex_digest(&setup, "zkey_sha256")?,
        "vk_digest": hex::encode(vk_digest),
        "ptau_name": field(&setup, "ptau_name")?.as_str().ok_or("ptau_name")?,
        "ptau_sha256": hex_digest(&setup, "ptau_sha256")?,
        "setup_transcript_sha256": hex_digest(&setup, "setup_transcript_sha256")?,
        "n_constraints": n_constraints,
        "n_public": N_PUBLIC,
        "public_layout": PUBLIC_LAYOUT,
        "ceremony": CEREMONY,
    });
    let descriptor_json = canonical_json::to_vec(&descriptor)?;
    let circuit_id = ids::circuit_id(&descriptor_json)?;
    let bundle = CircuitBundle { descriptor_json: descriptor_json.clone(), vk_bytes: vk_bytes.clone(), policy_json };
    let bundle_bytes = bundle.encode();
    if CircuitBundle::decode(&bundle_bytes)? != bundle {
        return Err("circuit bundle does not round-trip".into());
    }

    fs::write(dir.join("verification_key.bin"), &vk_bytes)?;
    fs::write(dir.join("descriptor.json"), &descriptor_json)?;
    fs::write(dir.join("circuit_id.hex"), format!("{}\n", hex::encode(circuit_id)))?;
    fs::write(dir.join("circuit_bundle.bin"), &bundle_bytes)?;
    let manifest = json!({
        "circuit_id": hex::encode(circuit_id),
        "policy_id": hex::encode(policy_id),
        "vk_digest": hex::encode(vk_digest),
        "bundle_id": hex::encode(ids::bundle_id(&bundle_bytes)),
        "bundle_len": bundle_bytes.len(),
        "vk_bytes_hex": hex::encode(&vk_bytes),
        "descriptor": descriptor,
        "files": "circuits/artifacts/v1m/{descriptor.json,verification_key.bin,verification_key.json,circuit_bundle.bin,main_v1m.zkey,main_v1m.graph.bin,main_v1m_js/main_v1m.wasm}",
        "warning": "development-single-party setup: not production-trusted (see docs/adr/0043-groth16-setup-ceremony.md)",
    });
    fs::write(manifest_path, format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    println!("circuit_id {}", hex::encode(circuit_id));
    println!("vk_digest {}", hex::encode(vk_digest));
    println!("policy_id {}", hex::encode(policy_id));
    println!("bundle_len {}", bundle_bytes.len());
    Ok(())
}

fn synthetic_capture_id(seed: u64) -> Hash32 {
    let mut id = [0; 32];
    id[..24].copy_from_slice(b"GALAXY/DEV/E2E/CAPTURE/1");
    id[24..].copy_from_slice(&seed.to_be_bytes());
    id
}

fn policy_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../policies/galaxy-pc-v1m.json")
}

fn e2e_input(dir: &Path) -> Result<()> {
    let policy = Policy::load(policy_path())?;
    let (lsel, lsel_hash) = (1_000_005u32, [0x5e; 32]);
    for seed in 0..64u64 {
        let evidence = synthesize(Scenario::Room, seed, &policy)?;
        let capture_id = synthetic_capture_id(seed);
        let challenge = Challenge::derive(DEV_NETWORK, &capture_id, lsel, &lsel_hash);
        let view = build_view(&evidence, &challenge).map_err(|_| "view")?;
        if !evaluate(&view, &policy).pass {
            continue;
        }
        let salts = Salts::random()?;
        let commitments = commit(&evidence, &salts);
        let input = galaxy_witness::build_input(&evidence, &salts, &commitments, &challenge, &capture_id, &view)?;
        fs::create_dir_all(dir)?;
        fs::write(dir.join("input.json"), serde_json::to_vec(&input)?)?;
        fs::write(dir.join("photo.png"), &evidence.image.encoded)?;
        let statement = json!({
            "scene": format!("synthetic room seed {seed} (development only)"),
            "network": DEV_NETWORK,
            "capture_id": hex::encode(capture_id),
            "lsel": lsel,
            "lsel_hash": hex::encode(lsel_hash),
            "evidence_root": fr_to_dec(&commitments.evidence_root),
        });
        fs::write(dir.join("statement.json"), serde_json::to_vec_pretty(&statement)?)?;
        println!("synthetic room seed {seed}: reference verdict PASS; input written");
        return Ok(());
    }
    Err("no passing synthetic room seed".into())
}

fn verify_dir(dir: &Path, vk_path: &Path) -> Result<()> {
    let statement = read_json(&dir.join("statement.json"))?;
    let network = u32::try_from(field(&statement, "network")?.as_u64().ok_or("network")?)?;
    let lsel = u32::try_from(field(&statement, "lsel")?.as_u64().ok_or("lsel")?)?;
    let mut capture_id = [0; 32];
    let mut lsel_hash = [0; 32];
    hex::decode_to_slice(field(&statement, "capture_id")?.as_str().ok_or("capture_id")?, &mut capture_id)?;
    hex::decode_to_slice(field(&statement, "lsel_hash")?.as_str().ok_or("lsel_hash")?, &mut lsel_hash)?;
    let evidence_root = fr_from_dec(field(&statement, "evidence_root")?.as_str().ok_or("evidence_root")?)
        .map_err(|_| "evidence_root")?;

    // rgb_root from the photo, tiles and seed from the selection ledger.
    let image = galaxy_image::decode_any(&fs::read(dir.join("photo.png"))?)?;
    let analysis = galaxy_image::analysis_raster(&image.raster)?;
    let rgb_root = FrameTree::build(TreeKind::Rgb, 1, &analysis, &[], &[]).map_err(|_| "rgb tree")?.root();
    let challenge = Challenge::derive(network, &capture_id, lsel, &lsel_hash);
    let public = public_inputs_v1(&rgb_root, &evidence_root, &capture_id, &challenge.seed, &challenge.tiles);

    // snarkjs public.json must list exactly the same statement.
    let snark_public = read_json(&dir.join("public.json"))?;
    let listed: Vec<String> = snark_public.as_array().ok_or("public.json")?
        .iter().map(|v| v.as_str().map(str::to_owned).ok_or("public.json entry")).collect::<std::result::Result<_, _>>()?;
    let reconstructed: Vec<String> = public.iter().map(fr_to_dec).collect();
    if listed != reconstructed {
        return Err("snarkjs public signals differ from the independently reconstructed public inputs".into());
    }

    let vk = VerifyingKey::from_bytes(&fs::read(vk_path)?)?;
    let proof = Proof::from_snarkjs_json(&fs::read(dir.join("proof.json"))?)?;
    let wire = proof.to_bytes();
    if wire.len() != 256 || Proof::from_bytes(&wire)?.to_bytes() != wire {
        return Err("proof wire encoding does not round-trip".into());
    }
    let valid = verify(&vk, &public, &Proof::from_bytes(&wire)?)?;
    println!("galaxy-crypto groth16 verify (reconstructed public inputs): {valid}");
    let mut altered = public;
    altered[6] += galaxy_crypto::Fr::from(1u8);
    let altered_valid = verify(&vk, &altered, &proof)?;
    println!("galaxy-crypto groth16 verify (sampled_tiles[0] + 1): {altered_valid}");
    if !valid || altered_valid {
        return Err("unexpected verification result".into());
    }
    Ok(())
}
