//! Desktop prove/verify round trip and snarkjs interop on the fixture in
//! test-vectors/prover/ (scripts/prover-build-fixture.sh).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use galaxy_prover::{codec, verify, Phase, ProveControl, Progress, Prover, ProverConfig, ProverError};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/prover")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name)).unwrap()
}

fn prover(cfg: ProverConfig) -> Prover {
    Prover::open_with_config(&dir().join("fixture.graph.bin"), &dir().join("fixture.zkey"), cfg).unwrap()
}

fn input_pub() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(&read("input.json")).unwrap();
    v["pub_in"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect()
}

#[derive(Default)]
struct Recorder(Mutex<Vec<(Phase, bool)>>);

impl Progress for Recorder {
    fn phase_started(&self, p: Phase) {
        self.0.lock().unwrap().push((p, true));
    }
    fn phase_finished(&self, p: Phase, _: f64) {
        self.0.lock().unwrap().push((p, false));
    }
}

#[test]
fn prove_then_verify_round_trip() {
    let p = prover(ProverConfig::default());
    assert_eq!(p.n_public(), 22);
    let rec = Recorder::default();
    let out = p.prove(&read("input.json"), &ProveControl { progress: Some(&rec), cancel: None }).unwrap();

    // Public inputs are the circuit's pub_in, 32-byte big-endian.
    let want: Vec<[u8; 32]> =
        input_pub().iter().map(|s| codec::encode_scalar(&codec::scalar_from_decimal(s).unwrap())).collect();
    assert_eq!(out.public_inputs, want);

    // Verifies with the spec §9 VK bytes, which equal the committed vk.bin.
    let vk = p.vk_bytes().unwrap();
    assert_eq!(vk, std::fs::read(dir().join("vk.bin")).unwrap());
    assert!(verify(&vk, &out.public_inputs, &out.proof).unwrap());

    // Every phase reported, in order, started then finished.
    let events = rec.0.into_inner().unwrap();
    let expected: Vec<(Phase, bool)> = Phase::ALL.iter().flat_map(|p| [(*p, true), (*p, false)]).collect();
    assert_eq!(events, expected);

    // A wrong public input or a proof mixing two proofs' elements does not verify.
    let mut bad_pub = out.public_inputs.clone();
    bad_pub[21][31] ^= 1;
    assert!(!verify(&vk, &bad_pub, &out.proof).unwrap());
    let other = p.prove(&read("input.json"), &ProveControl::default()).unwrap();
    assert_ne!(other.proof, out.proof, "proofs are randomized");
    let mut mixed = out.proof;
    mixed[192..].copy_from_slice(&other.proof[192..]);
    assert!(!verify(&vk, &out.public_inputs, &mixed).unwrap());
}

#[test]
fn small_msm_chunks_give_valid_proofs() {
    let p = prover(ProverConfig { msm_chunk: 7 });
    let out = p.prove(&read("input.json"), &ProveControl::default()).unwrap();
    assert!(verify(&p.vk_bytes().unwrap(), &out.public_inputs, &out.proof).unwrap());
}

#[test]
fn snarkjs_interop() {
    let p = prover(ProverConfig::default());
    let vk = p.verifying_key();
    // A proof produced by snarkjs (wasm witness) verifies here: c0/c1 convention.
    let sproof = codec::proof_from_snarkjs_json(&read("snarkjs_proof.json")).unwrap();
    let spublic = codec::public_from_snarkjs_json(&read("snarkjs_public.json")).unwrap();
    assert!(galaxy_prover::verify_ark(vk, &spublic, &sproof).unwrap());
    // Wire encoding round-trips and matches the snarkjs JSON.
    let bytes = codec::encode_proof(&sproof).unwrap();
    assert_eq!(codec::decode_proof(&bytes).unwrap(), sproof);
    let again = codec::proof_from_snarkjs_json(&codec::proof_to_snarkjs_json(&sproof).unwrap()).unwrap();
    assert_eq!(again, sproof);

    // VK bytes match snarkjs vk.json coordinates.
    let vj: serde_json::Value = serde_json::from_str(&read("vk.json")).unwrap();
    let dec = |b: &[u8]| codec::to_decimal(&ark_bn254::Fq::from(num_bigint::BigUint::from_bytes_be(b)));
    let vkb = p.vk_bytes().unwrap();
    assert_eq!(vkb[0..2], [1, 1]);
    assert_eq!(u32::from_be_bytes(vkb[2..6].try_into().unwrap()), 22);
    assert_eq!(dec(&vkb[6..38]), vj["vk_alpha_1"][0].as_str().unwrap());
    assert_eq!(dec(&vkb[38..70]), vj["vk_alpha_1"][1].as_str().unwrap());
    assert_eq!(dec(&vkb[70..102]), vj["vk_beta_2"][0][0].as_str().unwrap());
    assert_eq!(dec(&vkb[102..134]), vj["vk_beta_2"][0][1].as_str().unwrap());
    assert_eq!(dec(&vkb[134..166]), vj["vk_beta_2"][1][0].as_str().unwrap());
    let ic = vj["IC"].as_array().unwrap();
    assert_eq!(ic.len(), 23);
    let last = &vkb[vkb.len() - 64..];
    assert_eq!(dec(&last[..32]), ic[22][0].as_str().unwrap());
    assert_eq!(dec(&last[32..]), ic[22][1].as_str().unwrap());
}

#[test]
fn cancellation_is_honoured() {
    let p = prover(ProverConfig::default());
    let flag = AtomicBool::new(true);
    let r = p.prove(&read("input.json"), &ProveControl { progress: None, cancel: Some(&flag) });
    assert!(matches!(r, Err(ProverError::Cancelled)));

    // Cancel from the progress callback at the MSM stage.
    struct CancelAt<'a>(&'a AtomicBool);
    impl Progress for CancelAt<'_> {
        fn phase_started(&self, p: Phase) {
            if p == Phase::MsmH {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        fn phase_finished(&self, _: Phase, _: f64) {}
    }
    let flag = AtomicBool::new(false);
    let sink = CancelAt(&flag);
    let r = p.prove(&read("input.json"), &ProveControl { progress: Some(&sink), cancel: Some(&flag) });
    assert!(matches!(r, Err(ProverError::Cancelled)));
}

#[test]
fn unsatisfying_input_fails_closed() {
    let p = prover(ProverConfig::default());
    let mut v: serde_json::Value = serde_json::from_str(&read("input.json")).unwrap();
    // secret[0] = 2^64 violates Num2Bits(64).
    v["secret"][0] = "18446744073709551616".into();
    let r = p.prove(&v.to_string(), &ProveControl::default());
    assert!(matches!(r, Err(ProverError::SelfCheck(_)) | Err(ProverError::Witness(_))), "{r:?}");
    // Unknown signal name.
    let r = p.prove(r#"{"nope": "1"}"#, &ProveControl::default());
    assert!(matches!(r, Err(ProverError::Witness(_))), "{r:?}");
}

#[test]
fn strict_decoding() {
    let p = prover(ProverConfig::default());
    let out = p.prove(&read("input.json"), &ProveControl::default()).unwrap();
    let vk = p.vk_bytes().unwrap();
    // Trailing / missing bytes.
    assert!(verify(&vk, &out.public_inputs, &out.proof[..255]).is_err());
    let mut long = out.proof.to_vec();
    long.push(0);
    assert!(verify(&vk, &out.public_inputs, &long).is_err());
    let mut vk_long = vk.clone();
    vk_long.push(0);
    assert!(verify(&vk_long, &out.public_inputs, &out.proof).is_err());
    // Non-canonical coordinate (>= q) and off-curve point.
    let mut nc = out.proof;
    nc[..32].copy_from_slice(&[0xff; 32]);
    assert!(matches!(verify(&vk, &out.public_inputs, &nc), Err(ProverError::InvalidEncoding(_))));
    let mut off = out.proof;
    off[63] ^= 1;
    assert!(matches!(verify(&vk, &out.public_inputs, &off), Err(ProverError::InvalidEncoding(_))));
    // Public input >= r and wrong count.
    let mut big = out.public_inputs.clone();
    big[0] = [0xff; 32];
    assert!(verify(&vk, &big, &out.proof).is_err());
    assert!(verify(&vk, &out.public_inputs[1..], &out.proof).is_err());
    // Decimal parsing is canonical only.
    assert!(codec::scalar_from_decimal("01").is_err());
    assert!(codec::scalar_from_decimal("-1").is_err());
    assert!(codec::scalar_from_decimal(
        "21888242871839275222246405745257275088548364400416034343698204186575808495617"
    )
    .is_err());
}

#[test]
fn corrupt_zkey_is_rejected() {
    let raw = std::fs::read(dir().join("fixture.zkey")).unwrap();
    let tmp = std::env::temp_dir().join(format!("galaxy-prover-corrupt-{}.zkey", std::process::id()));
    // Truncated file.
    std::fs::write(&tmp, &raw[..raw.len() - 1]).unwrap();
    assert!(matches!(galaxy_prover::zkey::Zkey::open(&tmp), Err(ProverError::InvalidArtifact(_))));
    // Trailing byte.
    let mut t = raw.clone();
    t.push(0);
    std::fs::write(&tmp, &t).unwrap();
    assert!(matches!(galaxy_prover::zkey::Zkey::open(&tmp), Err(ProverError::InvalidArtifact(_))));
    // Wrong magic.
    let mut m = raw.clone();
    m[0] = b'x';
    std::fs::write(&tmp, &m).unwrap();
    assert!(matches!(galaxy_prover::zkey::Zkey::open(&tmp), Err(ProverError::InvalidArtifact(_))));
    std::fs::remove_file(&tmp).unwrap();
}

#[test]
fn wtns_parser_rejects_garbage() {
    assert!(galaxy_prover::witness::parse_wtns(b"wtns").is_err());
    assert!(galaxy_prover::witness::parse_wtns(b"nope\x02\0\0\0\x02\0\0\0").is_err());
}
