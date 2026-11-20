//! DEVELOPMENT-ONLY Groth16 setup: iden3 `.r1cs` → snarkjs-format `.zkey`.
//!
//! Single party, no powers-of-tau ceremony: the toxic waste (tau, alpha,
//! beta, gamma, delta) is sampled from the OS RNG inside this process and
//! dropped when it returns. Whoever runs it could produce proofs of false
//! statements, so its output must never be an accepted circuit. It exists for
//! prover benchmarks whose size makes `snarkjs groth16 setup` exceed the shared
//! build machine's memory (snarkjs materializes the whole R1CS as JS objects:
//! 5.4 GB RSS at 500k constraints). Peak memory here is a few hundred MB.
//!
//! The produced file has exactly the layout snarkjs writes for Groth16
//! (sections 1–9, see zkey.rs; section 10 "contributions" is omitted because
//! there is no ceremony) and the snarkjs QAP conventions:
//! - domain size `n` = 2^(floor(log2(n_constraints + n_public)) + 1);
//! - A gets one extra row per public signal s ∈ 0..=n_public (row
//!   n_constraints + s, coefficient 1 on signal s);
//! - wire polynomials use the Lagrange basis of the n-th roots of unity;
//! - the H query is the Lagrange basis of the 2n-th roots of unity at the odd
//!   points, divided by delta, matching the prover's odd-coset H evaluations.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use ark_bn254::{Fq, Fr, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::scalar_mul::BatchMulPreprocessing;
use ark_ec::{AffineRepr, PrimeGroup};
use ark_ff::{batch_inversion, BigInt, Field, PrimeField, Zero};
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use memmap2::Mmap;
use rayon::prelude::*;

use crate::ProverError;

const CHUNK: usize = 1 << 18;

fn bad(msg: impl Into<String>) -> ProverError {
    ProverError::InvalidArtifact(format!("r1cs: {}", msg.into()))
}

/// Parsed view of an iden3 `.r1cs` (v1) file.
struct R1cs<'a> {
    n_wires: usize,
    n_public: usize,
    n_constraints: usize,
    constraints: &'a [u8],
}

fn rd_u32(b: &[u8], at: usize) -> Result<u32, ProverError> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().expect("4"))).ok_or_else(|| bad("truncated"))
}

fn rd_u64(b: &[u8], at: usize) -> Result<u64, ProverError> {
    b.get(at..at + 8).map(|s| u64::from_le_bytes(s.try_into().expect("8"))).ok_or_else(|| bad("truncated"))
}

fn le_bigint(b: &[u8]) -> BigInt<4> {
    let mut limbs = [0u64; 4];
    for (i, l) in limbs.iter_mut().enumerate() {
        *l = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8"));
    }
    BigInt::new(limbs)
}

impl<'a> R1cs<'a> {
    fn parse(b: &'a [u8]) -> Result<Self, ProverError> {
        if b.get(0..4) != Some(b"r1cs") || rd_u32(b, 4)? != 1 {
            return Err(bad("not an r1cs v1 file"));
        }
        let n_sections = rd_u32(b, 8)?;
        let mut pos = 12usize;
        let (mut header, mut constraints) = (None, None);
        for _ in 0..n_sections {
            let id = rd_u32(b, pos)?;
            let len = usize::try_from(rd_u64(b, pos + 4)?).map_err(|_| bad("section size"))?;
            let start = pos + 12;
            let body = b.get(start..start + len).ok_or_else(|| bad("truncated section"))?;
            match id {
                1 => header = Some(body),
                2 => constraints = Some(body),
                3 => {}
                _ => return Err(bad(format!("unsupported section {id} (custom gates?)"))),
            }
            pos = start + len;
        }
        let h = header.ok_or_else(|| bad("missing header"))?;
        if rd_u32(h, 0)? != 32 || h.get(4..36) != Some(&Fr::MODULUS.0.iter().flat_map(|l| l.to_le_bytes()).collect::<Vec<_>>()[..]) {
            return Err(bad("prime is not BN254 Fr"));
        }
        let n_wires = rd_u32(h, 36)? as usize;
        let n_public = rd_u32(h, 40)? as usize + rd_u32(h, 44)? as usize;
        // header: n8, prime, nWires@36, nPubOut@40, nPubIn@44, nPrvIn@48, nLabels(u64)@52, nConstraints@60
        let n_constraints = rd_u32(h, 60)? as usize;
        Ok(Self { n_wires, n_public, n_constraints, constraints: constraints.ok_or_else(|| bad("missing constraints"))? })
    }

    /// Visits every term: `f(matrix 0|1|2, constraint, signal, value bytes)`.
    fn for_each_term(&self, mut f: impl FnMut(u32, usize, usize, &[u8])) -> Result<(), ProverError> {
        let b = self.constraints;
        let mut pos = 0usize;
        for c in 0..self.n_constraints {
            for m in 0..3u32 {
                let n = rd_u32(b, pos)? as usize;
                pos += 4;
                for _ in 0..n {
                    let signal = rd_u32(b, pos)? as usize;
                    let value = b.get(pos + 4..pos + 36).ok_or_else(|| bad("truncated term"))?;
                    if signal >= self.n_wires {
                        return Err(bad("signal index out of range"));
                    }
                    f(m, c, signal, value);
                    pos += 36;
                }
            }
        }
        if pos != b.len() {
            return Err(bad("trailing bytes in constraints"));
        }
        Ok(())
    }
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
        None => w.write_all(&[0u8; 64]),
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
        None => w.write_all(&[0u8; 128]),
    }
}

fn section(w: &mut impl Write, id: u32, len: usize) -> std::io::Result<()> {
    w.write_all(&id.to_le_bytes())?;
    w.write_all(&(len as u64).to_le_bytes())
}

fn write_g1s(w: &mut impl Write, table: &BatchMulPreprocessing<G1Projective>, s: &[Fr]) -> std::io::Result<()> {
    for chunk in s.chunks(CHUNK) {
        for p in table.batch_mul(chunk) {
            put_g1(w, &p)?;
        }
    }
    Ok(())
}

/// Lagrange basis of the `size`-th roots of unity at `tau` for the indices `idx`.
fn lagrange_at(size: usize, tau: Fr, idx: impl IndexedParallelIterator<Item = usize>) -> Result<Vec<Fr>, ProverError> {
    let d = Radix2EvaluationDomain::<Fr>::new(size)
        .filter(|d| d.size() == size)
        .ok_or_else(|| bad("domain too large"))?;
    let z = tau.pow([size as u64]) - Fr::ONE;
    let scale = z * Fr::from(size as u64).inverse().expect("nonzero");
    let omegas: Vec<Fr> = idx.map(|i| d.element(i)).collect();
    let mut den: Vec<Fr> = omegas.par_iter().map(|w| tau - w).collect();
    if den.iter().any(|v| v.is_zero()) {
        return Err(ProverError::Rng("tau is a root of unity".into()));
    }
    batch_inversion(&mut den);
    Ok(omegas.par_iter().zip(den.par_iter()).map(|(w, inv)| *w * scale * inv).collect())
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
}

/// Runs the development setup for `r1cs_path`, writing a snarkjs Groth16 zkey to `zkey_path`.
pub fn dev_setup(r1cs_path: &Path, zkey_path: &Path) -> Result<DevSetupInfo, ProverError> {
    let io = |e: std::io::Error| ProverError::Io(e.to_string());
    let file = File::open(r1cs_path).map_err(io)?;
    // SAFETY: read-only mapping of an input file not modified during the run.
    let map = unsafe { Mmap::map(&file) }.map_err(io)?;
    let r1cs = R1cs::parse(&map)?;
    let (n_vars, n_pub, n_c) = (r1cs.n_wires, r1cs.n_public, r1cs.n_constraints);
    if n_pub + 1 > n_vars {
        return Err(bad("more public signals than wires"));
    }
    let rows = n_c + n_pub + 1;
    let domain_size = 1usize << ((usize::BITS - (n_c + n_pub).leading_zeros()) as usize);
    if rows > domain_size {
        return Err(bad("domain sizing"));
    }

    let tau = nonzero_random()?;
    let (alpha, beta, gamma, delta) = (nonzero_random()?, nonzero_random()?, nonzero_random()?, nonzero_random()?);

    // Wire polynomials at tau.
    let lag = lagrange_at(domain_size, tau, (0..rows).into_par_iter())?;
    let (mut a, mut b, mut c) = (vec![Fr::zero(); n_vars], vec![Fr::zero(); n_vars], vec![Fr::zero(); n_vars]);
    let mut n_coefs = n_pub + 1;
    r1cs.for_each_term(|m, row, sig, val| {
        let v = Fr::from_bigint(le_bigint(val)).unwrap_or_else(|| Fr::from_le_bytes_mod_order(val)) * lag[row];
        match m {
            0 => {
                a[sig] += v;
                n_coefs += 1;
            }
            1 => {
                b[sig] += v;
                n_coefs += 1;
            }
            _ => c[sig] += v,
        }
    })?;
    for s in 0..=n_pub {
        a[s] += lag[n_c + s];
    }
    drop(lag);

    let g1 = G1Projective::generator();
    let g2 = G2Projective::generator();
    let t1 = BatchMulPreprocessing::new(g1, n_vars.max(domain_size));
    let t2 = BatchMulPreprocessing::new(g2, n_vars);
    let gamma_inv = gamma.inverse().expect("nonzero");
    let delta_inv = delta.inverse().expect("nonzero");
    let k = |i: usize| beta * a[i] + alpha * b[i] + c[i];

    let out = File::create(zkey_path).map_err(io)?;
    let mut w = BufWriter::with_capacity(1 << 22, out);
    let mut run = || -> std::io::Result<()> {
        w.write_all(b"zkey")?;
        w.write_all(&1u32.to_le_bytes())?;
        w.write_all(&9u32.to_le_bytes())?;
        section(&mut w, 1, 4)?;
        w.write_all(&1u32.to_le_bytes())?;
        section(&mut w, 2, 4 + 32 + 4 + 32 + 12 + 64 * 3 + 128 * 3)?;
        w.write_all(&32u32.to_le_bytes())?;
        for l in Fq::MODULUS.0 {
            w.write_all(&l.to_le_bytes())?;
        }
        w.write_all(&32u32.to_le_bytes())?;
        for l in Fr::MODULUS.0 {
            w.write_all(&l.to_le_bytes())?;
        }
        w.write_all(&(n_vars as u32).to_le_bytes())?;
        w.write_all(&(n_pub as u32).to_le_bytes())?;
        w.write_all(&(domain_size as u32).to_le_bytes())?;
        let pts1 = t1.batch_mul(&[alpha, beta, delta]);
        let pts2 = t2.batch_mul(&[beta, gamma, delta]);
        put_g1(&mut w, &pts1[0])?;
        put_g1(&mut w, &pts1[1])?;
        put_g2(&mut w, &pts2[0])?;
        put_g2(&mut w, &pts2[1])?;
        put_g1(&mut w, &pts1[2])?;
        put_g2(&mut w, &pts2[2])?;

        // 3: IC.
        section(&mut w, 3, (n_pub + 1) * 64)?;
        let ic: Vec<Fr> = (0..=n_pub).map(|i| k(i) * gamma_inv).collect();
        write_g1s(&mut w, &t1, &ic)?;

        // 4: coefficients of A and B (value stored as coef·R² mod r, little endian).
        section(&mut w, 4, 4 + n_coefs * 44)?;
        w.write_all(&(n_coefs as u32).to_le_bytes())?;
        let mut err = None;
        let mut put = |m: u32, row: usize, sig: usize, coef: Fr| {
            let stored = Fr::from_bigint(coef.0).expect("Montgomery limbs < r");
            let mut rec = [0u8; 44];
            rec[0..4].copy_from_slice(&m.to_le_bytes());
            rec[4..8].copy_from_slice(&(row as u32).to_le_bytes());
            rec[8..12].copy_from_slice(&(sig as u32).to_le_bytes());
            for (i, l) in stored.0 .0.iter().enumerate() {
                rec[12 + 8 * i..20 + 8 * i].copy_from_slice(&l.to_le_bytes());
            }
            if let Err(e) = w.write_all(&rec) {
                err.get_or_insert(e);
            }
        };
        r1cs.for_each_term(|m, row, sig, val| {
            if m < 2 {
                put(m, row, sig, Fr::from_bigint(le_bigint(val)).unwrap_or_else(|| Fr::from_le_bytes_mod_order(val)));
            }
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        for s in 0..=n_pub {
            put(0, n_c + s, s, Fr::ONE);
        }
        if let Some(e) = err {
            return Err(e);
        }

        // 5, 6, 7: A, B1, B2.
        section(&mut w, 5, n_vars * 64)?;
        write_g1s(&mut w, &t1, &a)?;
        section(&mut w, 6, n_vars * 64)?;
        write_g1s(&mut w, &t1, &b)?;
        section(&mut w, 7, n_vars * 128)?;
        for chunk in b.chunks(CHUNK) {
            for p in t2.batch_mul(chunk) {
                put_g2(&mut w, &p)?;
            }
        }
        // 8: C (= L) for private wires.
        section(&mut w, 8, (n_vars - n_pub - 1) * 64)?;
        for start in (n_pub + 1..n_vars).step_by(CHUNK) {
            let end = (start + CHUNK).min(n_vars);
            let s: Vec<Fr> = (start..end).into_par_iter().map(|i| k(i) * delta_inv).collect();
            write_g1s(&mut w, &t1, &s)?;
        }
        // 9: H — Lagrange basis of the 2n-th roots at odd indices, / delta.
        section(&mut w, 9, domain_size * 64)?;
        for start in (0..domain_size).step_by(CHUNK) {
            let end = (start + CHUNK).min(domain_size);
            let mut s = lagrange_at(2 * domain_size, tau, (start..end).into_par_iter().map(|j| 2 * j + 1))
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            s.par_iter_mut().for_each(|v| *v *= delta_inv);
            write_g1s(&mut w, &t1, &s)?;
        }
        w.flush()
    };
    run().map_err(io)?;
    Ok(DevSetupInfo { n_vars, n_public: n_pub, n_constraints: n_c, domain_size, n_coefs })
}
