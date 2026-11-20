//! Development tooling for `scripts/circuit.sh` (not shipped in any app):
//!
//! * `bundle <artifact_dir> <manifest.json> <policy.json>`: VK bytes (§9),
//!   canonical circuit descriptor, circuit_id and the GXCB circuit bundle (§10)
//!   from the dev-setup outputs.
//! * `commit <dir> [seed]`: commits the synthetic (development-only) room
//!   scene `seed` (default 0) with fresh salts: photo.png, commitment.json
//!   (seed, rgb_root, evidence_root) and salts.json (kept for later statements).
//! * `e2e-input <dir> [<capture_id hex> <lsel> <lsel_hash hex>]`: private
//!   circuit input for the committed scene and that statement (network 1), or,
//!   without a statement, for the first room seed passing the development
//!   challenge; writes input.json and the public statement.json a verifier
//!   starts from. Fails unless the reference verdict is PASS.
//! * `verify <dir> <verification_key.bin>`: reconstructs the 22 public inputs
//!   from the photo and the statement only (never from the witness input) and
//!   verifies the snarkjs proof with galaxy-crypto's Groth16 verifier.
//! * `check-witness <dir> <main.r1cs> <main.graph.bin>`: computes the witness
//!   of `<dir>/input.json` with the circom-witnesscalc graph (the prover's
//!   path), requires it to equal the WASM witness `<dir>/witness.wtns` wire by
//!   wire, checks every R1CS constraint against it and compares the 22 public
//!   wires with the independently reconstructed public inputs; then requires
//!   that the witness with one public wire changed violates a constraint.

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ark_ff::{BigInt, PrimeField};
use galaxy_crypto::Fr;
use galaxy_crypto::{
    field::{fr_from_dec, fr_to_dec},
    groth16::{verify, Proof, VerifyingKey},
    public::public_inputs_v1,
    sampling::Challenge,
    tree::{FrameTree, TreeKind},
};
use galaxy_evidence::{commit, Salts};
use galaxy_geometry::{
    build_view, evaluate,
    synthetic::{synthesize, Scenario},
};
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
        ["commit", dir] => commit_scene(Path::new(dir), 0),
        ["commit", dir, seed] => seed.parse().map_err(Into::into).and_then(|s| commit_scene(Path::new(dir), s)),
        ["e2e-input", dir] => e2e_input(Path::new(dir), None),
        ["e2e-input", dir, id, lsel, lsel_hash] => e2e_input(Path::new(dir), Some((id, lsel, lsel_hash))),
        ["verify", dir, vk] => verify_dir(Path::new(dir), Path::new(vk)),
        ["check-witness", dir, r1cs, graph] => check_witness(Path::new(dir), Path::new(r1cs), Path::new(graph)),
        _ => Err("usage: circuit_tool bundle <dir> <manifest> <policy> | commit <dir> [seed] \
                  | e2e-input <dir> [<capture_id hex> <lsel> <lsel_hash hex>] | verify <dir> <vk.bin> \
                  | check-witness <dir> <r1cs> <graph.bin>"
            .into()),
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
    Ok(serde_json::from_slice(
        &fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )?)
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).ok_or_else(|| format!("missing {key}").into())
}

fn hex_digest(v: &Value, key: &str) -> Result<String> {
    let s = field(v, key)?
        .as_str()
        .ok_or_else(|| format!("{key} is not a string"))?;
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
    let n_public = field(&r1cs, "n_public_inputs")?
        .as_u64()
        .ok_or("n_public_inputs")?
        + field(&r1cs, "n_public_outputs")?
            .as_u64()
            .ok_or("n_public_outputs")?;
    if n_public != u64::from(N_PUBLIC) || field(&r1cs, "n_public_outputs")?.as_u64() != Some(0) {
        return Err("R1CS must have exactly 22 public inputs and no public outputs".into());
    }
    let n_constraints = field(&r1cs, "n_constraints")?
        .as_u64()
        .ok_or("n_constraints")?;

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
    let bundle = CircuitBundle {
        descriptor_json: descriptor_json.clone(),
        vk_bytes: vk_bytes.clone(),
        policy_json,
    };
    let bundle_bytes = bundle.encode();
    if CircuitBundle::decode(&bundle_bytes)? != bundle {
        return Err("circuit bundle does not round-trip".into());
    }

    fs::write(dir.join("verification_key.bin"), &vk_bytes)?;
    fs::write(dir.join("descriptor.json"), &descriptor_json)?;
    fs::write(
        dir.join("circuit_id.hex"),
        format!("{}\n", hex::encode(circuit_id)),
    )?;
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
    fs::write(
        manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
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

const DEV_LSEL: u32 = 1_000_005;
const DEV_LSEL_HASH: Hash32 = [0x5e; 32];

fn salts_to_json(s: &Salts) -> Value {
    json!([
        s.depth0,
        s.frame_minus,
        s.frame_plus,
        s.camera,
        s.imu,
        s.bluetooth,
        s.noise,
        s.meta
    ]
    .iter()
    .map(fr_to_dec)
    .collect::<Vec<_>>())
}

fn salts_from_json(v: &Value) -> Result<Salts> {
    let f: Vec<Fr> = v
        .as_array()
        .filter(|a| a.len() == 8)
        .ok_or("salts.json: expected 8 decimal strings")?
        .iter()
        .map(|x| {
            x.as_str()
                .and_then(|s| fr_from_dec(s).ok())
                .ok_or("salts.json: bad salt")
        })
        .collect::<std::result::Result<_, _>>()?;
    Ok(Salts {
        depth0: f[0],
        frame_minus: f[1],
        frame_plus: f[2],
        camera: f[3],
        imu: f[4],
        bluetooth: f[5],
        noise: f[6],
        meta: f[7],
    })
}

/// Commits the synthetic room scene `seed` with fresh salts into `dir`:
/// photo.png, commitment.json (public: seed, rgb_root, evidence_root) and
/// salts.json (salts of synthetic development evidence only; gitignored
/// build directory), so later statements can bind the same evidence_root.
fn commit_scene(dir: &Path, seed: u64) -> Result<()> {
    let evidence = synthesize(Scenario::Room, seed, &Policy::load(policy_path())?)?;
    let salts = Salts::random()?;
    let commitments = commit(&evidence, &salts);
    fs::create_dir_all(dir)?;
    fs::write(dir.join("photo.png"), &evidence.image.encoded)?;
    fs::write(
        dir.join("salts.json"),
        serde_json::to_vec(&salts_to_json(&salts))?,
    )?;
    let commitment = json!({
        "scene": format!("synthetic room seed {seed} (development only)"),
        "seed": seed,
        "rgb_root": fr_to_dec(&commitments.rgb_root),
        "evidence_root": fr_to_dec(&commitments.evidence_root),
    });
    fs::write(
        dir.join("commitment.json"),
        serde_json::to_vec_pretty(&commitment)?,
    )?;
    println!(
        "committed synthetic room seed {seed}: evidence_root {}",
        fr_to_dec(&commitments.evidence_root)
    );
    Ok(())
}

/// Circuit input for the scene committed in `dir` and the statement
/// (network 1, capture_id, lsel, lsel_hash). Without a statement, the first
/// synthetic room seed passing the development challenge is committed and used.
fn e2e_input(dir: &Path, statement: Option<(&str, &str, &str)>) -> Result<()> {
    let policy = Policy::load(policy_path())?;
    let (capture_id, lsel, lsel_hash) = match statement {
        Some((id, lsel, lsel_hash)) => {
            let (mut a, mut b) = ([0; 32], [0; 32]);
            hex::decode_to_slice(id, &mut a).map_err(|_| "capture_id must be 32-byte hex")?;
            hex::decode_to_slice(lsel_hash, &mut b).map_err(|_| "lsel_hash must be 32-byte hex")?;
            if !dir.join("salts.json").exists() {
                return Err(format!("run `circuit_tool commit {}` first", dir.display()).into());
            }
            (a, lsel.parse::<u32>().map_err(|_| "lsel must be a u32")?, b)
        }
        None => {
            let seed = (0..64u64)
                .find(|&seed| {
                    let challenge = Challenge::derive(
                        DEV_NETWORK,
                        &synthetic_capture_id(seed),
                        DEV_LSEL,
                        &DEV_LSEL_HASH,
                    );
                    synthesize(Scenario::Room, seed, &policy)
                        .ok()
                        .and_then(|e| build_view(&e, &challenge).ok())
                        .is_some_and(|view| evaluate(&view, &policy).pass)
                })
                .ok_or("no passing synthetic room seed")?;
            commit_scene(dir, seed)?;
            (synthetic_capture_id(seed), DEV_LSEL, DEV_LSEL_HASH)
        }
    };
    let commitment = read_json(&dir.join("commitment.json"))?;
    let seed = field(&commitment, "seed")?
        .as_u64()
        .ok_or("commitment.json: seed")?;
    let evidence = synthesize(Scenario::Room, seed, &policy)?;
    let salts = salts_from_json(&read_json(&dir.join("salts.json"))?)?;
    let commitments = commit(&evidence, &salts);
    if fr_to_dec(&commitments.evidence_root)
        != field(&commitment, "evidence_root")?
            .as_str()
            .ok_or("evidence_root")?
    {
        return Err("salts.json and commitment.json do not match".into());
    }
    let challenge = Challenge::derive(DEV_NETWORK, &capture_id, lsel, &lsel_hash);
    let view = build_view(&evidence, &challenge).map_err(|_| "view")?;
    if !evaluate(&view, &policy).pass {
        return Err(format!(
            "synthetic room seed {seed}: reference verdict is not PASS for this challenge"
        )
        .into());
    }
    let input = galaxy_witness::build_input(
        &evidence,
        &salts,
        &commitments,
        &challenge,
        &capture_id,
        &view,
    )?;
    fs::write(dir.join("input.json"), serde_json::to_vec(&input)?)?;
    let statement = json!({
        "scene": format!("synthetic room seed {seed} (development only)"),
        "network": DEV_NETWORK,
        "capture_id": hex::encode(capture_id),
        "lsel": lsel,
        "lsel_hash": hex::encode(lsel_hash),
        "evidence_root": fr_to_dec(&commitments.evidence_root),
    });
    fs::write(
        dir.join("statement.json"),
        serde_json::to_vec_pretty(&statement)?,
    )?;
    println!("synthetic room seed {seed}: reference verdict PASS; input written");
    Ok(())
}

/// The 22 public inputs from the photo and the public statement only.
fn reconstruct_public(dir: &Path) -> Result<[Fr; 22]> {
    let statement = read_json(&dir.join("statement.json"))?;
    let network = u32::try_from(field(&statement, "network")?.as_u64().ok_or("network")?)?;
    let lsel = u32::try_from(field(&statement, "lsel")?.as_u64().ok_or("lsel")?)?;
    let mut capture_id = [0; 32];
    let mut lsel_hash = [0; 32];
    hex::decode_to_slice(
        field(&statement, "capture_id")?
            .as_str()
            .ok_or("capture_id")?,
        &mut capture_id,
    )?;
    hex::decode_to_slice(
        field(&statement, "lsel_hash")?
            .as_str()
            .ok_or("lsel_hash")?,
        &mut lsel_hash,
    )?;
    let evidence_root = fr_from_dec(
        field(&statement, "evidence_root")?
            .as_str()
            .ok_or("evidence_root")?,
    )
    .map_err(|_| "evidence_root")?;

    // rgb_root from the photo, tiles and seed from the selection ledger.
    let image = galaxy_image::decode_any(&fs::read(dir.join("photo.png"))?)?;
    let analysis = galaxy_image::analysis_raster(&image.raster)?;
    let rgb_root = FrameTree::build(TreeKind::Rgb, 1, &analysis, &[], &[])
        .map_err(|_| "rgb tree")?
        .root();
    let challenge = Challenge::derive(network, &capture_id, lsel, &lsel_hash);
    Ok(public_inputs_v1(
        &rgb_root,
        &evidence_root,
        &capture_id,
        &challenge.seed,
        &challenge.tiles,
    ))
}

fn verify_dir(dir: &Path, vk_path: &Path) -> Result<()> {
    let public = reconstruct_public(dir)?;

    // snarkjs public.json must list exactly the same statement.
    let snark_public = read_json(&dir.join("public.json"))?;
    let listed: Vec<String> = snark_public
        .as_array()
        .ok_or("public.json")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or("public.json entry"))
        .collect::<std::result::Result<_, _>>()?;
    let reconstructed: Vec<String> = public.iter().map(fr_to_dec).collect();
    if listed != reconstructed {
        return Err(
            "snarkjs public signals differ from the independently reconstructed public inputs"
                .into(),
        );
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
    altered[6] += Fr::from(1u8);
    let altered_valid = verify(&vk, &altered, &proof)?;
    println!("galaxy-crypto groth16 verify (sampled_tiles[0] + 1): {altered_valid}");
    if !valid || altered_valid {
        return Err("unexpected verification result".into());
    }
    Ok(())
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or("R1CS: truncated")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn big(&mut self) -> Result<BigInt<4>> {
        let s = self.take(32)?;
        Ok(BigInt::new(std::array::from_fn(|i| {
            u64::from_le_bytes(s[8 * i..8 * i + 8].try_into().expect("8 bytes"))
        })))
    }
}

/// Checks every constraint <A,w>·<B,w> = <C,w> of an iden3 R1CS v1 file over
/// BN254 (non-canonical coefficients rejected). Returns (constraints, public).
fn check_r1cs(bytes: &[u8], w: &[Fr]) -> Result<(usize, usize)> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != b"r1cs" || r.u32()? != 1 {
        return Err("not an R1CS v1 file".into());
    }
    let (mut header, mut constraints) = (None, None);
    for _ in 0..r.u32()? {
        let kind = r.u32()?;
        let len = usize::try_from(r.u64()?)?;
        let body = r.take(len)?;
        match kind {
            1 if header.is_none() => header = Some(body),
            2 if constraints.is_none() => constraints = Some(body),
            1 | 2 => return Err("R1CS: duplicate section".into()),
            _ => {}
        }
    }
    let mut h = Reader {
        b: header.ok_or("R1CS: no header")?,
        at: 0,
    };
    if h.u32()? != 32 || h.big()? != Fr::MODULUS {
        return Err("R1CS is not over BN254 Fr".into());
    }
    let n_wires = usize::try_from(h.u32()?)?;
    let n_public = usize::try_from(h.u32()?)? + usize::try_from(h.u32()?)?;
    let (_private, _labels) = (h.u32()?, h.u64()?);
    let m = usize::try_from(h.u32()?)?;
    if n_wires != w.len() {
        return Err(format!("R1CS has {n_wires} wires, witness {}", w.len()).into());
    }
    let mut c = Reader {
        b: constraints.ok_or("R1CS: no constraints")?,
        at: 0,
    };
    let lc = |c: &mut Reader| -> Result<Fr> {
        let mut acc = Fr::from(0u8);
        for _ in 0..c.u32()? {
            let wire = usize::try_from(c.u32()?)?;
            let coef = Fr::from_bigint(c.big()?).ok_or("R1CS: non-canonical coefficient")?;
            acc += coef * w.get(wire).ok_or("R1CS: wire out of range")?;
        }
        Ok(acc)
    };
    for i in 0..m {
        let (a, b, cc) = (lc(&mut c)?, lc(&mut c)?, lc(&mut c)?);
        if a * b != cc {
            return Err(format!("constraint {i} is not satisfied").into());
        }
    }
    if c.at != c.b.len() {
        return Err("R1CS: trailing constraint data".into());
    }
    Ok((m, n_public))
}

fn check_witness(dir: &Path, r1cs: &Path, graph: &Path) -> Result<()> {
    let input = fs::read_to_string(dir.join("input.json"))?;
    let graph = galaxy_prover::witness::WitnessGraph::open(graph)?;
    let t = std::time::Instant::now();
    let from_graph = graph.calculate(&input)?;
    println!(
        "graph witness: {} wires in {:.3} s",
        from_graph.len(),
        t.elapsed().as_secs_f64()
    );
    let from_wasm = galaxy_prover::witness::parse_wtns(&fs::read(dir.join("witness.wtns"))?)?;
    if from_graph != from_wasm {
        return Err("graph and WASM witnesses differ".into());
    }
    println!(
        "graph witness == WASM witness (all {} wires)",
        from_wasm.len()
    );
    if from_wasm.first() != Some(&Fr::from(1u8)) {
        return Err("witness wire 0 is not 1".into());
    }
    let r1cs_bytes = fs::read(r1cs)?;
    let (m, n_public) = check_r1cs(&r1cs_bytes, &from_wasm)?;
    println!("R1CS: all {m} constraints satisfied");
    let public = reconstruct_public(dir)?;
    if n_public != public.len() || from_wasm[1..=n_public] != public[..] {
        return Err(
            "public wires differ from the independently reconstructed public inputs".into(),
        );
    }
    println!("public wires 1..={n_public} == public_inputs_v1(photo, statement)");
    // Negative check on the full circuit: the same witness with one public wire
    // changed (rgb_root, evidence_root, seed_lo, sampled_tiles[0]) must violate
    // a constraint.
    for wire in [1, 2, 6, 7] {
        let mut modified = from_wasm.clone();
        modified[wire] += Fr::from(1u8);
        match check_r1cs(&r1cs_bytes, &modified) {
            Err(e) if e.to_string().ends_with("is not satisfied") => {
                println!("public wire {wire} + 1: {e} (rejected)")
            }
            Err(e) => return Err(e),
            Ok(_) => return Err(format!("public wire {wire} + 1 still satisfies the R1CS").into()),
        }
    }
    Ok(())
}
