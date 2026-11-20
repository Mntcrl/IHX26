//! Wire encodings used by the prover (spec §9) and snarkjs JSON interop for
//! development cross-checks.
//!
//! - Proof (256 B): A.x, A.y, B.x.c0, B.x.c1, B.y.c0, B.y.c1, C.x, C.y, each a
//!   32-byte big-endian canonical base-field element (< q).
//! - VK bytes: u8 version = 1 ‖ u8 curve = 1 (bn254) ‖ u32be n_public ‖
//!   alpha_g1 (64) ‖ beta_g2 (128) ‖ gamma_g2 (128) ‖ delta_g2 (128) ‖
//!   IC[n_public + 1] (64 each); G2 coordinates in the same c0, c1 order.
//! - Public input: 32-byte big-endian canonical scalar-field element (< r).
//!
//! Decoding is strict: non-canonical coordinates, points not on the curve,
//! points outside the prime-order subgroup (G2) and the point at infinity are
//! rejected, as are trailing bytes.

use std::str::FromStr;

use ark_bn254::{Bn254, Fq, Fq2, Fr, G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::{BigInteger, PrimeField};
use ark_groth16::{Proof, VerifyingKey};
use num_bigint::BigUint;

use crate::ProverError;

/// Size of an encoded proof.
pub const PROOF_BYTES: usize = 256;

const VK_VERSION: u8 = 1;
const CURVE_BN254: u8 = 1;

fn invalid(msg: impl Into<String>) -> ProverError {
    ProverError::InvalidEncoding(msg.into())
}

fn put_fq(out: &mut Vec<u8>, v: &Fq) {
    out.extend_from_slice(&v.into_bigint().to_bytes_be());
}

fn put_g1(out: &mut Vec<u8>, p: &G1Affine) -> Result<(), ProverError> {
    let (x, y) = p.xy().ok_or_else(|| invalid("G1 point at infinity"))?;
    put_fq(out, &x);
    put_fq(out, &y);
    Ok(())
}

fn put_g2(out: &mut Vec<u8>, p: &G2Affine) -> Result<(), ProverError> {
    let (x, y) = p.xy().ok_or_else(|| invalid("G2 point at infinity"))?;
    put_fq(out, &x.c0);
    put_fq(out, &x.c1);
    put_fq(out, &y.c0);
    put_fq(out, &y.c1);
    Ok(())
}

fn get_fq(b: &[u8]) -> Result<Fq, ProverError> {
    let v = BigUint::from_bytes_be(b);
    if v >= BigUint::from(Fq::MODULUS) {
        return Err(invalid("non-canonical base field element"));
    }
    Ok(Fq::from(v))
}

fn get_g1(b: &[u8]) -> Result<G1Affine, ProverError> {
    let p = G1Affine::new_unchecked(get_fq(&b[..32])?, get_fq(&b[32..64])?);
    if !p.is_on_curve() || !p.is_in_correct_subgroup_assuming_on_curve() {
        return Err(invalid("G1 point not on curve"));
    }
    Ok(p)
}

fn get_g2(b: &[u8]) -> Result<G2Affine, ProverError> {
    let x = Fq2::new(get_fq(&b[..32])?, get_fq(&b[32..64])?);
    let y = Fq2::new(get_fq(&b[64..96])?, get_fq(&b[96..128])?);
    let p = G2Affine::new_unchecked(x, y);
    if !p.is_on_curve() || !p.is_in_correct_subgroup_assuming_on_curve() {
        return Err(invalid("G2 point not on curve or not in the prime-order subgroup"));
    }
    Ok(p)
}

/// Encodes a proof into the 256-byte wire form.
pub fn encode_proof(proof: &Proof<Bn254>) -> Result<[u8; PROOF_BYTES], ProverError> {
    let mut out = Vec::with_capacity(PROOF_BYTES);
    put_g1(&mut out, &proof.a)?;
    put_g2(&mut out, &proof.b)?;
    put_g1(&mut out, &proof.c)?;
    Ok(out.try_into().expect("256 bytes"))
}

/// Decodes and validates a 256-byte proof.
pub fn decode_proof(b: &[u8]) -> Result<Proof<Bn254>, ProverError> {
    if b.len() != PROOF_BYTES {
        return Err(invalid(format!("proof must be {PROOF_BYTES} bytes, got {}", b.len())));
    }
    Ok(Proof { a: get_g1(&b[..64])?, b: get_g2(&b[64..192])?, c: get_g1(&b[192..256])? })
}

/// Encodes a verifying key into the spec §9 VK bytes.
pub fn encode_vk(vk: &VerifyingKey<Bn254>) -> Result<Vec<u8>, ProverError> {
    let n_public = vk.gamma_abc_g1.len().checked_sub(1).ok_or_else(|| invalid("empty IC"))?;
    let mut out = Vec::with_capacity(6 + 64 + 3 * 128 + 64 * (n_public + 1));
    out.push(VK_VERSION);
    out.push(CURVE_BN254);
    out.extend_from_slice(&u32::try_from(n_public).map_err(|_| invalid("n_public"))?.to_be_bytes());
    put_g1(&mut out, &vk.alpha_g1)?;
    put_g2(&mut out, &vk.beta_g2)?;
    put_g2(&mut out, &vk.gamma_g2)?;
    put_g2(&mut out, &vk.delta_g2)?;
    for p in &vk.gamma_abc_g1 {
        put_g1(&mut out, p)?;
    }
    Ok(out)
}

/// Decodes and validates spec §9 VK bytes.
pub fn decode_vk(b: &[u8]) -> Result<VerifyingKey<Bn254>, ProverError> {
    if b.len() < 6 || b[0] != VK_VERSION || b[1] != CURVE_BN254 {
        return Err(invalid("unknown VK version or curve"));
    }
    let n_public = u32::from_be_bytes(b[2..6].try_into().expect("4 bytes")) as usize;
    let want = n_public
        .checked_add(1)
        .and_then(|n| n.checked_mul(64))
        .and_then(|n| n.checked_add(6 + 64 + 3 * 128))
        .ok_or_else(|| invalid("n_public overflow"))?;
    if b.len() != want {
        return Err(invalid(format!("VK must be {want} bytes, got {}", b.len())));
    }
    let mut at = 6;
    let g1 = |b: &[u8], at: &mut usize| {
        let p = get_g1(&b[*at..*at + 64]);
        *at += 64;
        p
    };
    let alpha_g1 = g1(b, &mut at)?;
    let beta_g2 = get_g2(&b[at..at + 128])?;
    let gamma_g2 = get_g2(&b[at + 128..at + 256])?;
    let delta_g2 = get_g2(&b[at + 256..at + 384])?;
    at += 384;
    let gamma_abc_g1 = (0..=n_public).map(|_| g1(b, &mut at)).collect::<Result<Vec<_>, _>>()?;
    Ok(VerifyingKey { alpha_g1, beta_g2, gamma_g2, delta_g2, gamma_abc_g1 })
}

/// Encodes a public input as 32 big-endian bytes.
pub fn encode_scalar(v: &Fr) -> [u8; 32] {
    v.into_bigint().to_bytes_be().try_into().expect("32 bytes")
}

/// Decodes a 32-byte big-endian public input, rejecting values >= r.
pub fn decode_scalar(b: &[u8]) -> Result<Fr, ProverError> {
    if b.len() != 32 {
        return Err(invalid("public input must be 32 bytes"));
    }
    let v = BigUint::from_bytes_be(b);
    if v >= BigUint::from(Fr::MODULUS) {
        return Err(invalid("public input >= r"));
    }
    Ok(Fr::from(v))
}

/// Parses a canonical decimal scalar (no sign, no leading zeros, < r).
pub fn scalar_from_decimal(s: &str) -> Result<Fr, ProverError> {
    let canonical = !s.is_empty()
        && s.bytes().all(|c| c.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    if !canonical {
        return Err(invalid(format!("not a canonical decimal: {s:?}")));
    }
    let v = BigUint::from_str(s).map_err(|_| invalid("bad decimal"))?;
    if v >= BigUint::from(Fr::MODULUS) {
        return Err(invalid("scalar >= r"));
    }
    Ok(Fr::from(v))
}

/// Decimal string of a field element.
pub fn to_decimal<F: PrimeField>(v: &F) -> String {
    BigUint::from_bytes_le(&v.into_bigint().to_bytes_le()).to_string()
}

fn fq_from_decimal(s: &str) -> Result<Fq, ProverError> {
    let v = BigUint::from_str(s).map_err(|_| invalid("bad decimal"))?;
    if v >= BigUint::from(Fq::MODULUS) {
        return Err(invalid("coordinate >= q"));
    }
    Ok(Fq::from(v))
}

/// snarkjs `proof.json` for a proof (development interop).
pub fn proof_to_snarkjs_json(proof: &Proof<Bn254>) -> Result<String, ProverError> {
    let (ax, ay) = proof.a.xy().ok_or_else(|| invalid("A at infinity"))?;
    let (bx, by) = proof.b.xy().ok_or_else(|| invalid("B at infinity"))?;
    let (cx, cy) = proof.c.xy().ok_or_else(|| invalid("C at infinity"))?;
    let d = to_decimal::<Fq>;
    let v = serde_json::json!({
        "pi_a": [d(&ax), d(&ay), "1"],
        "pi_b": [[d(&bx.c0), d(&bx.c1)], [d(&by.c0), d(&by.c1)], ["1", "0"]],
        "pi_c": [d(&cx), d(&cy), "1"],
        "protocol": "groth16",
        "curve": "bn128",
    });
    Ok(serde_json::to_string_pretty(&v).expect("json"))
}

/// Parses a snarkjs `proof.json` (affine coordinates, z = 1) and validates it.
pub fn proof_from_snarkjs_json(json: &str) -> Result<Proof<Bn254>, ProverError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
    let s = |p: &serde_json::Value| -> Result<Fq, ProverError> {
        fq_from_decimal(p.as_str().ok_or_else(|| invalid("coordinate is not a string"))?)
    };
    let pa = &v["pi_a"];
    let pb = &v["pi_b"];
    let pc = &v["pi_c"];
    if pa[2] != "1" || pc[2] != "1" || pb[2][0] != "1" || pb[2][1] != "0" {
        return Err(invalid("snarkjs proof is not in affine form"));
    }
    let mut enc = Vec::with_capacity(PROOF_BYTES);
    for f in [
        s(&pa[0])?,
        s(&pa[1])?,
        s(&pb[0][0])?,
        s(&pb[0][1])?,
        s(&pb[1][0])?,
        s(&pb[1][1])?,
        s(&pc[0])?,
        s(&pc[1])?,
    ] {
        put_fq(&mut enc, &f);
    }
    decode_proof(&enc)
}

/// snarkjs `public.json` for a list of public inputs.
pub fn public_to_snarkjs_json(public: &[Fr]) -> String {
    let v: Vec<String> = public.iter().map(to_decimal).collect();
    serde_json::to_string_pretty(&v).expect("json")
}

/// Parses a snarkjs `public.json` strictly.
pub fn public_from_snarkjs_json(json: &str) -> Result<Vec<Fr>, ProverError> {
    let v: Vec<String> = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
    v.iter().map(|s| scalar_from_decimal(s)).collect()
}
