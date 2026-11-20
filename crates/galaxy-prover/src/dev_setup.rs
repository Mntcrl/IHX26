//! DEVELOPMENT-ONLY Groth16 setup: iden3 `.r1cs` + prepared powers-of-tau →
//! snarkjs-format `.zkey`.
//!
//! Phase 1 comes from a prepared (`*_final_*`) powers-of-tau file whose SHA-256
//! the caller pins (the file is hashed before use; mismatch fails closed).
//! Phase 2 is a single contribution: delta is sampled from the OS RNG inside
//! this process and dropped when it returns. Whoever runs it knows delta and
//! could produce proofs of false statements, so the output must never be an
//! accepted circuit key. It exists because `snarkjs groth16 setup` exceeds the
//! shared build machine's memory at ~1M constraints (snarkjs materializes the
//! whole R1CS as JS objects: 5.4 GB RSS at 500k constraints).
//!
//! The result equals `snarkjs zkey new` followed by one `zkey contribute`
//! (same sections 1–9, same QAP conventions, gamma = generator); section 10
//! (contribution transcript) is omitted because no snarkjs ceremony ran:
//! - domain size `n` = 2^(floor(log2(n_constraints + n_public)) + 1), which
//!   must not exceed 2^power of the ptau file;
//! - A gets one extra row per public signal s ∈ 0..=n_public (row
//!   n_constraints + s, coefficient 1 on signal s);
//! - wire points are sparse sums of the ptau Lagrange-basis points
//!   (sections 12–15 at size n): A, B1 = Σ coef·[L_row(τ)]₁, B2 = Σ coef·[L_row(τ)]₂,
//!   K = Σ_A coef·[βL_row]₁ + Σ_B coef·[αL_row]₁ + Σ_C coef·[L_row]₁;
//!   IC = K (public wires), C = K/δ (private wires);
//! - H = the size-2n Lagrange points at odd indices, divided by δ.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::ops::Range;
use std::path::Path;

use ark_bn254::{Fq, Fr, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::scalar_mul::glv::GLVConfig;
use ark_ec::short_weierstrass::{Affine, Projective};
use ark_ec::{AffineRepr, CurveGroup, PrimeGroup, VariableBaseMSM};
use ark_ff::{BigInt, BigInteger, Field, One, PrimeField, Zero};
use memmap2::Mmap;
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::zkey::{unchecked_g1, unchecked_g2};
use crate::ProverError;

const CHUNK: usize = 1 << 16;
/// Wires with more terms than this use arkworks' MSM, called from the setup's own
/// thread (see [`emit`]); lighter wires are combined serially inside the per-wire map.
const HEAVY_TERMS: usize = 1 << 12;
const G1_BYTES: usize = 64;
const G2_BYTES: usize = 128;

fn bad(msg: impl Into<String>) -> ProverError {
    ProverError::InvalidArtifact(format!("r1cs: {}", msg.into()))
}

fn bad_ptau(msg: impl Into<String>) -> ProverError {
    ProverError::InvalidArtifact(format!("ptau: {}", msg.into()))
}

fn rd_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().expect("4")))
}

fn rd_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().expect("8")))
}

fn le_bigint(b: &[u8]) -> BigInt<4> {
    let mut limbs = [0u64; 4];
    for (i, l) in limbs.iter_mut().enumerate() {
        *l = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8"));
    }
    BigInt::new(limbs)
}

fn le_modulus<F: PrimeField<BigInt = BigInt<4>>>() -> Vec<u8> {
    F::MODULUS.0.iter().flat_map(|l| l.to_le_bytes()).collect()
}

/// Section table of an iden3 binary container (`r1cs`, `ptau`): `(id, body)` pairs.
fn sections<'a>(
    b: &'a [u8],
    magic: &[u8; 4],
    err: fn(String) -> ProverError,
) -> Result<Vec<(u32, &'a [u8])>, ProverError> {
    if b.get(0..4) != Some(&magic[..]) || rd_u32(b, 4) != Some(1) {
        return Err(err("bad magic or version".into()));
    }
    let n = rd_u32(b, 8).ok_or_else(|| err("truncated".into()))?;
    let mut pos = 12usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let id = rd_u32(b, pos).ok_or_else(|| err("truncated".into()))?;
        let len = rd_u64(b, pos + 4)
            .and_then(|l| usize::try_from(l).ok())
            .ok_or_else(|| err("truncated".into()))?;
        let start = pos + 12;
        let body = start
            .checked_add(len)
            .and_then(|end| b.get(start..end))
            .ok_or_else(|| err("truncated section".into()))?;
        if out.iter().any(|(i, _)| *i == id) {
            return Err(err(format!("duplicate section {id}")));
        }
        out.push((id, body));
        pos = start + len;
    }
    if pos != b.len() {
        return Err(err("trailing bytes".into()));
    }
    Ok(out)
}

/// Parsed view of an iden3 `.r1cs` (v1) file.
struct R1cs<'a> {
    n_wires: usize,
    n_public: usize,
    n_constraints: usize,
    constraints: &'a [u8],
}

impl<'a> R1cs<'a> {
    fn parse(b: &'a [u8]) -> Result<Self, ProverError> {
        let secs = sections(b, b"r1cs", bad)?;
        let (mut header, mut constraints) = (None, None);
        for (id, body) in secs {
            match id {
                1 => header = Some(body),
                2 => constraints = Some(body),
                3 => {}
                _ => return Err(bad(format!("unsupported section {id} (custom gates?)"))),
            }
        }
        let h = header.ok_or_else(|| bad("missing header"))?;
        // header: n8, prime, nWires@36, nPubOut@40, nPubIn@44, nPrvIn@48, nLabels(u64)@52, nConstraints@60
        if rd_u32(h, 0) != Some(32) || h.get(4..36) != Some(&le_modulus::<Fr>()[..]) {
            return Err(bad("prime is not BN254 Fr"));
        }
        let field = |at| {
            rd_u32(h, at)
                .map(|v| v as usize)
                .ok_or_else(|| bad("truncated header"))
        };
        let constraints = constraints.ok_or_else(|| bad("missing constraints"))?;
        if u32::try_from(constraints.len()).is_err() {
            return Err(bad("constraints section larger than 4 GiB"));
        }
        Ok(Self {
            n_wires: field(36)?,
            n_public: field(40)? + field(44)?,
            n_constraints: field(60)?,
            constraints,
        })
    }

    /// Visits every term: `f(matrix 0|1|2, constraint, signal, offset of the coefficient in `constraints`)`.
    /// Coefficients must be canonical (< r).
    fn for_each_term(
        &self,
        mut f: impl FnMut(usize, usize, usize, u32),
    ) -> Result<(), ProverError> {
        let b = self.constraints;
        let modulus = Fr::MODULUS;
        let mut pos = 0usize;
        for c in 0..self.n_constraints {
            for m in 0..3 {
                let n = rd_u32(b, pos).ok_or_else(|| bad("truncated constraint"))? as usize;
                pos += 4;
                for _ in 0..n {
                    let signal = rd_u32(b, pos).ok_or_else(|| bad("truncated term"))? as usize;
                    let value = b
                        .get(pos + 4..pos + 36)
                        .ok_or_else(|| bad("truncated term"))?;
                    if signal >= self.n_wires {
                        return Err(bad("signal index out of range"));
                    }
                    if le_bigint(value) >= modulus {
                        return Err(bad("non-canonical coefficient"));
                    }
                    f(m, c, signal, (pos + 4) as u32);
                    pos += 36;
                }
            }
        }
        if pos != b.len() {
            return Err(bad("trailing bytes in constraints"));
        }
        Ok(())
    }

    fn coef(&self, at: u32) -> BigInt<4> {
        le_bigint(&self.constraints[at as usize..at as usize + 32])
    }
}

/// Per-wire term lists of one R1CS matrix: `(row, coefficient offset)`.
struct Csr {
    off: Vec<usize>,
    ent: Vec<(u32, u32)>,
}

impl Csr {
    fn terms(&self, wire: usize) -> &[(u32, u32)] {
        &self.ent[self.off[wire]..self.off[wire + 1]]
    }
}

fn transpose(r1cs: &R1cs<'_>) -> Result<[Csr; 3], ProverError> {
    let mut counts = [(); 3].map(|_| vec![0usize; r1cs.n_wires + 1]);
    r1cs.for_each_term(|m, _, s, _| counts[m][s + 1] += 1)?;
    let mut out = counts.map(|mut off| {
        for i in 1..off.len() {
            off[i] += off[i - 1];
        }
        let ent = vec![(0u32, 0u32); off[off.len() - 1]];
        Csr { off, ent }
    });
    let mut cursor: Vec<Vec<usize>> = out.iter().map(|c| c.off.clone()).collect();
    r1cs.for_each_term(|m, row, s, at| {
        out[m].ent[cursor[m][s]] = (row as u32, at);
        cursor[m][s] += 1;
    })?;
    Ok(out)
}

/// The phase-1 points this setup consumes (prepared ptau, Lagrange sections at domain size `n`).
struct Phase1<'a> {
    alpha1: &'a [u8],
    beta1: &'a [u8],
    beta2: &'a [u8],
    /// [L_i(τ)]₁, [L_i(τ)]₂, [αL_i(τ)]₁, [βL_i(τ)]₁ for the size-n domain.
    l1: &'a [u8],
    l2: &'a [u8],
    alpha_l1: &'a [u8],
    beta_l1: &'a [u8],
    /// [L_i(τ)]₁ for the size-2n domain (H query).
    l1_2n: &'a [u8],
}

impl<'a> Phase1<'a> {
    fn parse(b: &'a [u8], n: usize) -> Result<(Self, u32), ProverError> {
        let secs = sections(b, b"ptau", bad_ptau)?;
        let sec = |id: u32| {
            secs.iter()
                .find(|(i, _)| *i == id)
                .map(|(_, s)| *s)
                .ok_or_else(|| bad_ptau(format!("missing section {id} (not a prepared ptau?)")))
        };
        let h = sec(1)?;
        if rd_u32(h, 0) != Some(32) || h.get(4..36) != Some(&le_modulus::<Fq>()[..]) {
            return Err(bad_ptau("curve is not BN254"));
        }
        let power = rd_u32(h, 36).ok_or_else(|| bad_ptau("truncated header"))?;
        if power >= 28 || n > 1usize << power {
            return Err(bad_ptau(format!(
                "circuit too big for this ptau: domain {n} > 2^{power}"
            )));
        }
        // Lagrange sections hold the bases for sizes 1, 2, 4, …: size m starts at element m − 1.
        let lagrange = |id: u32, m: usize, point: usize| -> Result<&'a [u8], ProverError> {
            sec(id)?
                .get((m - 1) * point..(2 * m - 1) * point)
                .ok_or_else(|| bad_ptau(format!("section {id} lacks size {m}")))
        };
        let first = |id: u32, point: usize| -> Result<&'a [u8], ProverError> {
            sec(id)?
                .get(..point)
                .ok_or_else(|| bad_ptau(format!("section {id} empty")))
        };
        let p = Self {
            alpha1: first(4, G1_BYTES)?,
            beta1: first(5, G1_BYTES)?,
            beta2: first(6, G2_BYTES)?,
            l1: lagrange(12, n, G1_BYTES)?,
            l2: lagrange(13, n, G2_BYTES)?,
            alpha_l1: lagrange(14, n, G1_BYTES)?,
            beta_l1: lagrange(15, n, G1_BYTES)?,
            l1_2n: lagrange(12, 2 * n, G1_BYTES)?,
        };
        Ok((p, power))
    }
}

fn g1_at(t: &[u8], i: usize) -> G1Affine {
    unchecked_g1(&t[i * G1_BYTES..(i + 1) * G1_BYTES])
}

fn g2_at(t: &[u8], i: usize) -> G2Affine {
    unchecked_g2(&t[i * G2_BYTES..(i + 1) * G2_BYTES])
}

fn nonzero_random() -> Result<Fr, ProverError> {
    let mut buf = [0u8; 64];
    getrandom::fill(&mut buf).map_err(|e| ProverError::Rng(e.to_string()))?;
    let v = Fr::from_le_bytes_mod_order(&buf);
    if v.is_zero() {
        return Err(ProverError::Rng("zero toxic-waste scalar".into()));
    }
    Ok(v)
}

fn put_fq(w: &mut impl Write, v: &Fq) -> std::io::Result<()> {
    // snarkjs stores base-field elements in Montgomery form (R = 2^256), little endian.
    for l in v.0 .0 {
        w.write_all(&l.to_le_bytes())?;
    }
    Ok(())
}

fn put_g1(w: &mut impl Write, p: &G1Affine) -> std::io::Result<()> {
    match p.xy() {
        Some((x, y)) => {
            put_fq(w, &x)?;
            put_fq(w, &y)
        }
        None => w.write_all(&[0u8; G1_BYTES]),
    }
}

fn put_g2(w: &mut impl Write, p: &G2Affine) -> std::io::Result<()> {
    match p.xy() {
        Some((x, y)) => {
            put_fq(w, &x.c0)?;
            put_fq(w, &x.c1)?;
            put_fq(w, &y.c0)?;
            put_fq(w, &y.c1)
        }
        None => w.write_all(&[0u8; G2_BYTES]),
    }
}

fn section(w: &mut impl Write, id: u32, len: usize) -> std::io::Result<()> {
    w.write_all(&id.to_le_bytes())?;
    w.write_all(&(len as u64).to_le_bytes())
}

/// Σ sᵢ·Pᵢ computed serially with GLV scalar multiplication (affine `mul_bigint` would
/// use plain double-and-add); a scalar above r/2 is applied as −(r − s)·P, so ±small
/// R1CS coefficients cost a few group operations.
fn lincomb<C: GLVConfig<ScalarField = Fr>>(
    bases: &[Affine<C>],
    scalars: &[BigInt<4>],
) -> Projective<C> {
    let mut acc = Projective::<C>::zero();
    for (p, s) in bases.iter().zip(scalars) {
        if s.is_zero() {
            continue;
        }
        if *s == BigInt::from(1u64) {
            acc += p;
            continue;
        }
        let mut neg = Fr::MODULUS;
        neg.sub_with_borrow(s);
        let (k, negate) = if neg.num_bits() < s.num_bits() {
            (neg, true)
        } else {
            (*s, false)
        };
        let k = Fr::from_bigint(k).expect("R1CS coefficients are canonical");
        let term = C::glv_mul_projective(p.into_group(), k);
        if negate {
            acc -= term;
        } else {
            acc += term;
        }
    }
    acc
}

/// Writes `post(Σ s·P)` for every index of `range` in order, where `terms(i, bases, scalars)`
/// pushes the (point, scalar) terms of index `i`.
///
/// Thread bound: the per-index map runs on the current rayon pool and combines terms
/// serially ([`lincomb`]). ark-ec 0.6's parallel MSM (`msm_bigint_wnaf`) builds a fresh
/// rayon pool on every call, so it is only called here, from the calling thread, for the
/// few wires with more than [`HEAVY_TERMS`] terms (calling it per wire inside the parallel
/// map started ~16k threads on a 500k-constraint circuit and overflowed worker stacks).
fn emit<C, W>(
    w: &mut W,
    range: Range<usize>,
    terms: impl Fn(usize, &mut Vec<Affine<C>>, &mut Vec<BigInt<4>>) + Sync,
    post: impl Fn(Projective<C>) -> Projective<C> + Sync,
    put: fn(&mut W, &Affine<C>) -> std::io::Result<()>,
) -> std::io::Result<()>
where
    C: GLVConfig<ScalarField = Fr>,
    W: Write,
{
    let (mut bases, mut scalars) = (Vec::new(), Vec::new());
    for start in range.clone().step_by(CHUNK) {
        let end = (start + CHUNK).min(range.end);
        let light: Vec<Option<Projective<C>>> = (start..end)
            .into_par_iter()
            .map_init(
                || (Vec::new(), Vec::new()),
                |(b, s), i| {
                    b.clear();
                    s.clear();
                    terms(i, b, s);
                    (b.len() <= HEAVY_TERMS).then(|| post(lincomb(b, s)))
                },
            )
            .collect();
        let proj: Vec<Projective<C>> = light
            .into_iter()
            .zip(start..end)
            .map(|(p, i)| {
                p.unwrap_or_else(|| {
                    bases.clear();
                    scalars.clear();
                    terms(i, &mut bases, &mut scalars);
                    post(Projective::<C>::msm_bigint(&bases, &scalars))
                })
            })
            .collect();
        for p in Projective::<C>::normalize_batch(&proj) {
            put(w, &p)?;
        }
    }
    Ok(())
}

/// Summary of a generated zkey.
#[derive(Debug, Clone)]
pub struct DevSetupInfo {
    /// Wires.
    pub n_vars: usize,
    /// Public signals.
    pub n_public: usize,
    /// R1CS constraints.
    pub n_constraints: usize,
    /// FFT domain size.
    pub domain_size: usize,
    /// A/B coefficient records written.
    pub n_coefs: usize,
    /// Power of the consumed ptau file.
    pub ptau_power: u32,
}

fn sha256_file(path: &Path) -> Result<[u8; 32], ProverError> {
    let mut f =
        File::open(path).map_err(|e| ProverError::Io(format!("open {}: {e}", path.display())))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 22];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| ProverError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// Runs the development setup for `r1cs_path` with the phase-1 powers of `ptau_path`
/// (whose SHA-256 must equal `ptau_sha256`), writing a snarkjs Groth16 zkey to `zkey_path`.
pub fn dev_setup(
    r1cs_path: &Path,
    ptau_path: &Path,
    ptau_sha256: &[u8; 32],
    zkey_path: &Path,
) -> Result<DevSetupInfo, ProverError> {
    let io = |e: std::io::Error| ProverError::Io(e.to_string());
    if &sha256_file(ptau_path)? != ptau_sha256 {
        return Err(bad_ptau("SHA-256 does not match the pinned digest"));
    }
    let map_file = |p: &Path| -> Result<Mmap, ProverError> {
        let file =
            File::open(p).map_err(|e| ProverError::Io(format!("open {}: {e}", p.display())))?;
        // SAFETY: read-only mapping of an input file not modified during the run.
        unsafe { Mmap::map(&file) }
            .map_err(|e| ProverError::Io(format!("mmap {}: {e}", p.display())))
    };
    let r1cs_map = map_file(r1cs_path)?;
    let r1cs = R1cs::parse(&r1cs_map)?;
    let (n_vars, n_pub, n_c) = (r1cs.n_wires, r1cs.n_public, r1cs.n_constraints);
    if n_pub + 1 > n_vars {
        return Err(bad("more public signals than wires"));
    }
    let n = 1usize << ((usize::BITS - (n_c + n_pub).leading_zeros()) as usize);
    if n_c + n_pub + 1 > n {
        return Err(bad("domain sizing"));
    }
    let ptau_map = map_file(ptau_path)?;
    let (ph, ptau_power) = Phase1::parse(&ptau_map, n)?;
    let [ma, mb, mc] = transpose(&r1cs)?;
    let one = Fr::one().into_bigint();

    let delta = nonzero_random()?;
    let delta_inv = delta.inverse().expect("nonzero");
    let n_coefs = ma.ent.len() + mb.ent.len() + n_pub + 1;

    // K_i = Σ_A coef·[βL]₁ + Σ_B coef·[αL]₁ + Σ_C coef·[L]₁ (+ [βL]₁ of the public row).
    let k = |i: usize, b: &mut Vec<G1Affine>, s: &mut Vec<BigInt<4>>| {
        for (src, t) in [(&ma, ph.beta_l1), (&mb, ph.alpha_l1), (&mc, ph.l1)] {
            for &(row, at) in src.terms(i) {
                b.push(g1_at(t, row as usize));
                s.push(r1cs.coef(at));
            }
        }
        if i <= n_pub {
            b.push(g1_at(ph.beta_l1, n_c + i));
            s.push(one);
        }
    };
    let id = |p: G1Projective| p;
    let over_delta = |p: G1Projective| p * delta_inv;

    let out = File::create(zkey_path)
        .map_err(|e| ProverError::Io(format!("create {}: {e}", zkey_path.display())))?;
    let mut w = BufWriter::with_capacity(1 << 22, out);
    let mut run = || -> std::io::Result<()> {
        w.write_all(b"zkey")?;
        w.write_all(&1u32.to_le_bytes())?;
        w.write_all(&9u32.to_le_bytes())?;
        section(&mut w, 1, 4)?;
        w.write_all(&1u32.to_le_bytes())?;

        // 2: header. alpha1, beta1, beta2 from phase 1; gamma2 = generator; delta from phase 2.
        section(
            &mut w,
            2,
            4 + 32 + 4 + 32 + 12 + G1_BYTES * 3 + G2_BYTES * 3,
        )?;
        w.write_all(&32u32.to_le_bytes())?;
        w.write_all(&le_modulus::<Fq>())?;
        w.write_all(&32u32.to_le_bytes())?;
        w.write_all(&le_modulus::<Fr>())?;
        w.write_all(&(n_vars as u32).to_le_bytes())?;
        w.write_all(&(n_pub as u32).to_le_bytes())?;
        w.write_all(&(n as u32).to_le_bytes())?;
        w.write_all(ph.alpha1)?;
        w.write_all(ph.beta1)?;
        w.write_all(ph.beta2)?;
        put_g2(&mut w, &G2Projective::generator().into_affine())?;
        put_g1(&mut w, &(G1Projective::generator() * delta).into_affine())?;
        put_g2(&mut w, &(G2Projective::generator() * delta).into_affine())?;

        // 3: IC = K (gamma = 1).
        section(&mut w, 3, (n_pub + 1) * G1_BYTES)?;
        emit(&mut w, 0..n_pub + 1, k, id, put_g1)?;

        // 4: coefficients of A and B (value stored as coef·R² mod r, little endian).
        section(&mut w, 4, 4 + n_coefs * 44)?;
        w.write_all(&(n_coefs as u32).to_le_bytes())?;
        let mut put = |m: u32, row: usize, sig: usize, coef: BigInt<4>| -> std::io::Result<()> {
            let stored = Fr::from_bigint(Fr::from_bigint(coef).expect("canonical").0)
                .expect("Montgomery limbs < r");
            let mut rec = [0u8; 44];
            rec[0..4].copy_from_slice(&m.to_le_bytes());
            rec[4..8].copy_from_slice(&(row as u32).to_le_bytes());
            rec[8..12].copy_from_slice(&(sig as u32).to_le_bytes());
            for (i, l) in stored.0 .0.iter().enumerate() {
                rec[12 + 8 * i..20 + 8 * i].copy_from_slice(&l.to_le_bytes());
            }
            w.write_all(&rec)
        };
        // snarkjs order: constraint by constraint, A terms then B terms.
        let mut first_err = None;
        r1cs.for_each_term(|m, row, sig, at| {
            if m < 2 && first_err.is_none() {
                if let Err(e) = put(m as u32, row, sig, r1cs.coef(at)) {
                    first_err = Some(e);
                }
            }
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        if let Some(e) = first_err {
            return Err(e);
        }
        for s in 0..=n_pub {
            put(0, n_c + s, s, one)?;
        }

        // 5: A.
        section(&mut w, 5, n_vars * G1_BYTES)?;
        emit(
            &mut w,
            0..n_vars,
            |i, b, s| {
                for &(row, at) in ma.terms(i) {
                    b.push(g1_at(ph.l1, row as usize));
                    s.push(r1cs.coef(at));
                }
                if i <= n_pub {
                    b.push(g1_at(ph.l1, n_c + i));
                    s.push(one);
                }
            },
            id,
            put_g1,
        )?;
        // 6: B1.
        section(&mut w, 6, n_vars * G1_BYTES)?;
        emit(
            &mut w,
            0..n_vars,
            |i, b, s| {
                for &(row, at) in mb.terms(i) {
                    b.push(g1_at(ph.l1, row as usize));
                    s.push(r1cs.coef(at));
                }
            },
            id,
            put_g1,
        )?;
        // 7: B2.
        section(&mut w, 7, n_vars * G2_BYTES)?;
        emit(
            &mut w,
            0..n_vars,
            |i, b: &mut Vec<G2Affine>, s| {
                for &(row, at) in mb.terms(i) {
                    b.push(g2_at(ph.l2, row as usize));
                    s.push(r1cs.coef(at));
                }
            },
            |p: G2Projective| p,
            put_g2,
        )?;
        // 8: C = K/δ for private wires.
        section(&mut w, 8, (n_vars - n_pub - 1) * G1_BYTES)?;
        emit(&mut w, n_pub + 1..n_vars, k, over_delta, put_g1)?;
        // 9: H — size-2n Lagrange points at odd indices, / δ.
        section(&mut w, 9, n * G1_BYTES)?;
        emit(
            &mut w,
            0..n,
            |j, b, s| {
                b.push(g1_at(ph.l1_2n, 2 * j + 1));
                s.push(one);
            },
            over_delta,
            put_g1,
        )?;
        w.flush()
    };
    // Runs on the caller's thread and the current rayon pool (RAYON_NUM_THREADS);
    // `emit` documents the thread bound.
    if let Err(e) = run() {
        drop(w);
        // Never leave a truncated key behind; the write error is what the caller needs.
        let _ = std::fs::remove_file(zkey_path);
        return Err(io(e));
    }
    Ok(DevSetupInfo {
        n_vars,
        n_public: n_pub,
        n_constraints: n_c,
        domain_size: n,
        n_coefs,
        ptau_power,
    })
}
