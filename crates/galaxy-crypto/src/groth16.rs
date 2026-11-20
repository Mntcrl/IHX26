//! Groth16 over BN254 (§9): proof wire codec, VK bytes codec, snarkjs JSON importers and
//! verification via ark-groth16.
//!
//! - Proof (256 B): A.x, A.y, B.x.c0, B.x.c1, B.y.c0, B.y.c1, C.x, C.y; each coordinate is a
//!   32-byte big-endian base-field element < q.
//! - VK bytes: u8 version = 1 ‖ u8 curve = 1 (bn254) ‖ u32be n_public ‖ alpha_g1 (64) ‖
//!   beta_g2 (128) ‖ gamma_g2 (128) ‖ delta_g2 (128) ‖ IC[n_public + 1] (64 each), G2
//!   coordinates in the same c0, c1 order.
//!
//! Every decoder (bytes or snarkjs JSON) rejects non-canonical coordinates, points not on the
//! curve, points outside the prime-order subgroup (checked for G1 and G2), the point at
//! infinity, wrong lengths and trailing bytes. snarkjs stores each Fq2 as `[c0, c1]`; the
//! convention is pinned by `test-vectors/protocol/groth16/` (a real snarkjs proof).
use crate::field::{bigint_from_be, bigint_from_decimal, bigint_to_be};
use crate::Fr;
use ark_bn254::{Bn254, Fq, Fq2, G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::PrimeField;
use serde_json::Value;

pub const PROOF_LEN: usize = 256;
pub const VK_VERSION: u8 = 1;
pub const CURVE_BN254: u8 = 1;
const VK_HEADER_LEN: usize = 6;
const VK_FIXED_LEN: usize = VK_HEADER_LEN + 64 + 3 * 128;
/// Upper bound on n_public accepted by decoders (Galaxy v1 uses 22).
pub const MAX_PUBLIC_INPUTS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Groth16Error {
    #[error("wrong length: expected {expected} bytes, got {actual}")]
    Length { expected: usize, actual: usize },
    #[error("unknown VK version or curve")]
    VkHeader,
    #[error("n_public out of range")]
    PublicCount,
    #[error("coordinate is not a canonical base-field element (>= q)")]
    NonCanonical,
    #[error("point at infinity")]
    Infinity,
    #[error("point not on the curve")]
    NotOnCurve,
    #[error("point not in the prime-order subgroup")]
    NotInSubgroup,
    #[error("invalid snarkjs JSON: {0}")]
    Json(String),
    #[error("expected {expected} public inputs, got {actual}")]
    PublicInputs { expected: usize, actual: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Proof(ark_groth16::Proof<Bn254>);

#[derive(Clone, Debug, PartialEq)]
pub struct VerifyingKey(ark_groth16::VerifyingKey<Bn254>);

impl Proof {
    /// Wrap an arkworks proof after checking the same point invariants as `from_bytes`.
    pub fn from_ark(proof: ark_groth16::Proof<Bn254>) -> Result<Self, Groth16Error> {
        check_g1(&proof.a)?;
        check_g2(&proof.b)?;
        check_g1(&proof.c)?;
        Ok(Self(proof))
    }

    pub fn as_ark(&self) -> &ark_groth16::Proof<Bn254> {
        &self.0
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Groth16Error> {
        let bytes: &[u8; PROOF_LEN] =
            bytes.try_into().map_err(|_| Groth16Error::Length { expected: PROOF_LEN, actual: bytes.len() })?;
        Ok(Self(ark_groth16::Proof {
            a: g1_from_bytes(&bytes[..64])?,
            b: g2_from_bytes(&bytes[64..192])?,
            c: g1_from_bytes(&bytes[192..])?,
        }))
    }

    pub fn to_bytes(&self) -> [u8; PROOF_LEN] {
        let mut out = [0u8; PROOF_LEN];
        g1_to_bytes(&self.0.a, &mut out[..64]);
        g2_to_bytes(&self.0.b, &mut out[64..192]);
        g1_to_bytes(&self.0.c, &mut out[192..]);
        out
    }

    /// Import a snarkjs `proof.json` (`pi_a`, `pi_b`, `pi_c` in projective form with z = 1).
    pub fn from_snarkjs_json(json: &[u8]) -> Result<Self, Groth16Error> {
        let value = parse_json(json)?;
        check_protocol(&value)?;
        Ok(Self(ark_groth16::Proof {
            a: g1_from_json(field(&value, "pi_a")?)?,
            b: g2_from_json(field(&value, "pi_b")?)?,
            c: g1_from_json(field(&value, "pi_c")?)?,
        }))
    }
}

impl VerifyingKey {
    /// Wrap an arkworks verifying key after checking the same invariants as `from_bytes`.
    pub fn from_ark(vk: ark_groth16::VerifyingKey<Bn254>) -> Result<Self, Groth16Error> {
        if vk.gamma_abc_g1.is_empty() || vk.gamma_abc_g1.len() > MAX_PUBLIC_INPUTS + 1 {
            return Err(Groth16Error::PublicCount);
        }
        check_g1(&vk.alpha_g1)?;
        for point in [&vk.beta_g2, &vk.gamma_g2, &vk.delta_g2] {
            check_g2(point)?;
        }
        vk.gamma_abc_g1.iter().try_for_each(check_g1)?;
        Ok(Self(vk))
    }

    pub fn as_ark(&self) -> &ark_groth16::VerifyingKey<Bn254> {
        &self.0
    }

    pub fn n_public(&self) -> usize {
        self.0.gamma_abc_g1.len() - 1
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Groth16Error> {
        let header = bytes
            .get(..VK_HEADER_LEN)
            .ok_or(Groth16Error::Length { expected: VK_FIXED_LEN + 64, actual: bytes.len() })?;
        if header[0] != VK_VERSION || header[1] != CURVE_BN254 {
            return Err(Groth16Error::VkHeader);
        }
        let n_public = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
        if n_public > MAX_PUBLIC_INPUTS {
            return Err(Groth16Error::PublicCount);
        }
        let expected = VK_FIXED_LEN + 64 * (n_public + 1);
        if bytes.len() != expected {
            return Err(Groth16Error::Length { expected, actual: bytes.len() });
        }
        let g2 = |at: usize| g2_from_bytes(&bytes[at..at + 128]);
        Ok(Self(ark_groth16::VerifyingKey {
            alpha_g1: g1_from_bytes(&bytes[6..70])?,
            beta_g2: g2(70)?,
            gamma_g2: g2(198)?,
            delta_g2: g2(326)?,
            gamma_abc_g1: bytes[VK_FIXED_LEN..].chunks_exact(64).map(g1_from_bytes).collect::<Result<_, _>>()?,
        }))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let vk = &self.0;
        let mut out = vec![0u8; VK_FIXED_LEN + 64 * vk.gamma_abc_g1.len()];
        out[0] = VK_VERSION;
        out[1] = CURVE_BN254;
        out[2..6].copy_from_slice(&(self.n_public() as u32).to_be_bytes());
        g1_to_bytes(&vk.alpha_g1, &mut out[6..70]);
        g2_to_bytes(&vk.beta_g2, &mut out[70..198]);
        g2_to_bytes(&vk.gamma_g2, &mut out[198..326]);
        g2_to_bytes(&vk.delta_g2, &mut out[326..454]);
        for (point, dst) in vk.gamma_abc_g1.iter().zip(out[VK_FIXED_LEN..].chunks_exact_mut(64)) {
            g1_to_bytes(point, dst);
        }
        out
    }

    /// Import a snarkjs `verification_key.json` (`protocol` "groth16", `curve` "bn128",
    /// `nPublic` = IC.len() − 1). `vk_alphabeta_12` is derived data and is ignored.
    pub fn from_snarkjs_json(json: &[u8]) -> Result<Self, Groth16Error> {
        let value = parse_json(json)?;
        check_protocol(&value)?;
        let n_public = field(&value, "nPublic")?
            .as_u64()
            .filter(|&n| n as usize <= MAX_PUBLIC_INPUTS)
            .ok_or(Groth16Error::PublicCount)? as usize;
        let ic = field(&value, "IC")?.as_array().ok_or_else(|| json_error("IC is not an array"))?;
        if ic.len() != n_public + 1 {
            return Err(Groth16Error::PublicCount);
        }
        Ok(Self(ark_groth16::VerifyingKey {
            alpha_g1: g1_from_json(field(&value, "vk_alpha_1")?)?,
            beta_g2: g2_from_json(field(&value, "vk_beta_2")?)?,
            gamma_g2: g2_from_json(field(&value, "vk_gamma_2")?)?,
            delta_g2: g2_from_json(field(&value, "vk_delta_2")?)?,
            gamma_abc_g1: ic.iter().map(g1_from_json).collect::<Result<_, _>>()?,
        }))
    }
}

/// Parse a snarkjs `public.json` (array of canonical decimal strings < r).
pub fn public_from_snarkjs_json(json: &[u8]) -> Result<Vec<Fr>, Groth16Error> {
    parse_json(json)?
        .as_array()
        .ok_or_else(|| json_error("public signals are not an array"))?
        .iter()
        .map(|v| {
            let text = v.as_str().ok_or_else(|| json_error("public signal is not a string"))?;
            crate::field::fr_from_dec(text).map_err(|_| json_error("public signal is not a canonical scalar"))
        })
        .collect()
}

/// Groth16 verification. `public.len()` must equal `vk.n_public()` (ark-groth16 alone would
/// silently ignore extra or missing inputs). `Ok(false)` means the pairing check failed.
pub fn verify(vk: &VerifyingKey, public: &[Fr], proof: &Proof) -> Result<bool, Groth16Error> {
    if public.len() != vk.n_public() {
        return Err(Groth16Error::PublicInputs { expected: vk.n_public(), actual: public.len() });
    }
    let pvk = ark_groth16::prepare_verifying_key(&vk.0);
    // `verify_proof` has no failure mode for a well-formed VK of matching length.
    Ok(ark_groth16::Groth16::<Bn254>::verify_proof(&pvk, &proof.0, public).unwrap_or(false))
}

fn fq_from_bytes(bytes: &[u8]) -> Result<Fq, Groth16Error> {
    Fq::from_bigint(bigint_from_be(bytes.try_into().unwrap())).ok_or(Groth16Error::NonCanonical)
}

fn fq_to_bytes(value: &Fq, out: &mut [u8]) {
    out.copy_from_slice(&bigint_to_be(value.into_bigint()));
}

fn check_g1(point: &G1Affine) -> Result<(), Groth16Error> {
    if point.is_zero() {
        return Err(Groth16Error::Infinity);
    }
    if !point.is_on_curve() {
        return Err(Groth16Error::NotOnCurve);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(Groth16Error::NotInSubgroup);
    }
    Ok(())
}

fn check_g2(point: &G2Affine) -> Result<(), Groth16Error> {
    if point.is_zero() {
        return Err(Groth16Error::Infinity);
    }
    if !point.is_on_curve() {
        return Err(Groth16Error::NotOnCurve);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(Groth16Error::NotInSubgroup);
    }
    Ok(())
}

fn g1_from_coords(x: Fq, y: Fq) -> Result<G1Affine, Groth16Error> {
    let point = G1Affine::new_unchecked(x, y);
    check_g1(&point)?;
    Ok(point)
}

fn g2_from_coords(x: Fq2, y: Fq2) -> Result<G2Affine, Groth16Error> {
    let point = G2Affine::new_unchecked(x, y);
    check_g2(&point)?;
    Ok(point)
}

fn g1_from_bytes(bytes: &[u8]) -> Result<G1Affine, Groth16Error> {
    // The wire has no infinity encoding; all-zero is reported as infinity rather than off-curve.
    if bytes.iter().all(|&b| b == 0) {
        return Err(Groth16Error::Infinity);
    }
    g1_from_coords(fq_from_bytes(&bytes[..32])?, fq_from_bytes(&bytes[32..])?)
}

fn g2_from_bytes(bytes: &[u8]) -> Result<G2Affine, Groth16Error> {
    if bytes.iter().all(|&b| b == 0) {
        return Err(Groth16Error::Infinity);
    }
    let fq = |i: usize| fq_from_bytes(&bytes[32 * i..32 * (i + 1)]);
    g2_from_coords(Fq2::new(fq(0)?, fq(1)?), Fq2::new(fq(2)?, fq(3)?))
}

fn g1_to_bytes(point: &G1Affine, out: &mut [u8]) {
    // Points are validated (never infinity) on every construction path.
    let (x, y) = point.xy().expect("validated G1 point");
    fq_to_bytes(&x, &mut out[..32]);
    fq_to_bytes(&y, &mut out[32..]);
}

fn g2_to_bytes(point: &G2Affine, out: &mut [u8]) {
    let (x, y) = point.xy().expect("validated G2 point");
    for (i, coordinate) in [x.c0, x.c1, y.c0, y.c1].iter().enumerate() {
        fq_to_bytes(coordinate, &mut out[32 * i..32 * (i + 1)]);
    }
}

fn parse_json(json: &[u8]) -> Result<Value, Groth16Error> {
    serde_json::from_slice(json).map_err(|e| json_error(&e.to_string()))
}

fn json_error(message: &str) -> Groth16Error {
    Groth16Error::Json(message.to_owned())
}

fn field<'a>(value: &'a Value, key: &'static str) -> Result<&'a Value, Groth16Error> {
    value.get(key).ok_or_else(|| json_error(&format!("missing {key}")))
}

fn check_protocol(value: &Value) -> Result<(), Groth16Error> {
    if field(value, "protocol")?.as_str() != Some("groth16") || field(value, "curve")?.as_str() != Some("bn128") {
        return Err(json_error("protocol must be groth16 on bn128"));
    }
    Ok(())
}

fn fq_from_json(value: &Value) -> Result<Fq, Groth16Error> {
    let text = value.as_str().ok_or_else(|| json_error("coordinate is not a string"))?;
    let bigint = bigint_from_decimal(text.as_bytes()).ok_or_else(|| json_error("coordinate is not a canonical decimal"))?;
    Fq::from_bigint(bigint).ok_or(Groth16Error::NonCanonical)
}

fn strings<'a, const N: usize>(value: &'a Value, what: &str) -> Result<[&'a Value; N], Groth16Error> {
    let items = value.as_array().filter(|a| a.len() == N).ok_or_else(|| json_error(&format!("{what} must have {N} entries")))?;
    Ok(std::array::from_fn(|i| &items[i]))
}

/// snarkjs projective G1 `[x, y, "1"]`.
fn g1_from_json(value: &Value) -> Result<G1Affine, Groth16Error> {
    let [x, y, z] = strings::<3>(value, "G1 point")?;
    if z.as_str() != Some("1") {
        return Err(json_error("G1 point is not normalized (z != 1)"));
    }
    g1_from_coords(fq_from_json(x)?, fq_from_json(y)?)
}

fn fq2_from_json(value: &Value) -> Result<Fq2, Groth16Error> {
    let [c0, c1] = strings::<2>(value, "Fq2 element")?;
    Ok(Fq2::new(fq_from_json(c0)?, fq_from_json(c1)?))
}

/// snarkjs projective G2 `[[x.c0, x.c1], [y.c0, y.c1], ["1", "0"]]`.
fn g2_from_json(value: &Value) -> Result<G2Affine, Groth16Error> {
    let [x, y, z] = strings::<3>(value, "G2 point")?;
    let [z0, z1] = strings::<2>(z, "G2 z")?;
    if z0.as_str() != Some("1") || z1.as_str() != Some("0") {
        return Err(json_error("G2 point is not normalized (z != 1)"));
    }
    g2_from_coords(fq2_from_json(x)?, fq2_from_json(y)?)
}
