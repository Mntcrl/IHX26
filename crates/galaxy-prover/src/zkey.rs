//! Zero-copy reader for snarkjs Groth16 `.zkey` files (BN254).
//!
//! The file is memory-mapped; nothing but the header and the verifying key is
//! decoded at open time. Point sections are decoded chunk by chunk while the
//! prover runs, so the proving key never has to be resident as a whole: the
//! mapping is clean, file-backed memory that the OS can evict (on iOS it does
//! not count towards the app's `phys_footprint`).
//!
//! Layout (snarkjs `zkey_utils.js`, "zkey" v1): magic `zkey`, u32 version,
//! u32 n_sections, then sections `(u32 id, u64 size, data)`:
//! 1 header (u32 protocol, 1 = groth16), 2 Groth16 header, 3 IC, 4 coefficients
//! of A and B, 5 A points, 6 B1 points, 7 B2 points, 8 C (= L) points,
//! 9 H points, 10 contributions. Field elements are little-endian Montgomery
//! representations (R = 2^256); the point at infinity is all zero bytes.
//! Coefficients are stored as Montgomery representations multiplied by R once
//! more.

use std::fs::File;
use std::path::Path;

use ark_bn254::{Bn254, Fq, Fq2, Fr, G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::{BigInt, PrimeField};
use ark_groth16::VerifyingKey;
use memmap2::Mmap;
use rayon::prelude::*;

use crate::ProverError;

const G1_BYTES: usize = 64;
const G2_BYTES: usize = 128;
const FIELD_BYTES: usize = 32;
const COEF_BYTES: usize = 12 + FIELD_BYTES;

/// Byte range of one zkey section inside the mapping.
#[derive(Clone, Copy, Debug)]
struct Section {
    offset: usize,
    len: usize,
}

/// Header values of a Groth16 zkey.
#[derive(Clone, Debug)]
pub struct ZkeyHeader {
    /// Number of wires including the constant 1 wire.
    pub n_vars: usize,
    /// Number of public signals (outputs + public inputs).
    pub n_public: usize,
    /// FFT domain size (power of two).
    pub domain_size: usize,
    /// Number of (A, B) coefficients in section 4.
    pub n_coefs: usize,
}

/// A memory-mapped snarkjs Groth16 proving key.
pub struct Zkey {
    map: Mmap,
    header: ZkeyHeader,
    vk: VerifyingKey<Bn254>,
    beta_g1: G1Affine,
    delta_g1: G1Affine,
    coefs: Section,
    a: Section,
    b1: Section,
    b2: Section,
    c: Section,
    h: Section,
}

impl Zkey {
    /// Maps and validates a `.zkey` file. Validation is fail-closed: unknown
    /// protocol, wrong curve, missing/duplicate sections or section sizes that
    /// do not match the header are errors. Verifying-key points are checked
    /// on-curve and in the prime-order subgroup.
    pub fn open(path: &Path) -> Result<Self, ProverError> {
        let file = File::open(path)
            .map_err(|e| ProverError::Io(format!("open {}: {e}", path.display())))?;
        // SAFETY: the artifact is opened read-only and treated as immutable for
        // the lifetime of the mapping; callers must not modify artifact files
        // while a prover holds them (documented in docs/protocol/prover.md).
        let map = unsafe { Mmap::map(&file) }
            .map_err(|e| ProverError::Io(format!("mmap {}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            // Point sections are read sequentially, once per proof.
            let _ = map.advise(memmap2::Advice::Sequential);
        }
        Self::from_map(map)
    }

    fn from_map(map: Mmap) -> Result<Self, ProverError> {
        let bytes: &[u8] = &map;
        let mut r = Cursor { buf: bytes, pos: 0 };
        if r.take(4)? != b"zkey" {
            return Err(bad("magic is not 'zkey'"));
        }
        let version = r.u32()?;
        if version != 1 {
            return Err(bad(format!("unsupported zkey version {version}")));
        }
        let n_sections = r.u32()? as usize;
        let mut sections: [Option<Section>; 11] = [None; 11];
        for _ in 0..n_sections {
            let id = r.u32()? as usize;
            let len = usize::try_from(r.u64()?).map_err(|_| bad("section too large"))?;
            let offset = r.pos;
            r.take(len)?;
            if id == 0 || id > 10 {
                return Err(bad(format!("unknown section id {id}")));
            }
            if sections[id].is_some() {
                return Err(bad(format!("duplicate section {id}")));
            }
            sections[id] = Some(Section { offset, len });
        }
        if r.pos != bytes.len() {
            return Err(bad("trailing bytes after the last section"));
        }
        let sec = |id: usize| sections[id].ok_or_else(|| bad(format!("missing section {id}")));

        // Section 1: protocol.
        let s1 = sec(1)?;
        let mut r1 = Cursor::at(bytes, s1);
        if r1.u32()? != 1 || s1.len != 4 {
            return Err(bad("zkey protocol is not groth16"));
        }

        // Section 2: Groth16 header.
        let s2 = sec(2)?;
        let mut h = Cursor::at(bytes, s2);
        let n8q = h.u32()? as usize;
        if n8q != FIELD_BYTES || h.take(n8q)? != le_modulus::<Fq>().as_slice() {
            return Err(bad("base field is not BN254 Fq"));
        }
        let n8r = h.u32()? as usize;
        if n8r != FIELD_BYTES || h.take(n8r)? != le_modulus::<Fr>().as_slice() {
            return Err(bad("scalar field is not BN254 Fr"));
        }
        let n_vars = h.u32()? as usize;
        let n_public = h.u32()? as usize;
        let domain_size = h.u32()? as usize;
        let alpha_g1 = checked_g1(h.take(G1_BYTES)?)?;
        let beta_g1 = checked_g1(h.take(G1_BYTES)?)?;
        let beta_g2 = checked_g2(h.take(G2_BYTES)?)?;
        let gamma_g2 = checked_g2(h.take(G2_BYTES)?)?;
        let delta_g1 = checked_g1(h.take(G1_BYTES)?)?;
        let delta_g2 = checked_g2(h.take(G2_BYTES)?)?;
        if h.pos != s2.offset + s2.len {
            return Err(bad("Groth16 header has trailing bytes"));
        }
        if n_public + 1 > n_vars {
            return Err(bad("n_public exceeds n_vars"));
        }
        if !domain_size.is_power_of_two() || domain_size < 2 {
            return Err(bad("domain size is not a power of two"));
        }

        // Section 3: IC.
        let s3 = sec(3)?;
        expect_len(s3, (n_public + 1) * G1_BYTES, 3)?;
        let gamma_abc_g1 = bytes[s3.offset..s3.offset + s3.len]
            .chunks_exact(G1_BYTES)
            .map(checked_g1)
            .collect::<Result<Vec<_>, _>>()?;

        // Section 4: coefficients.
        let s4 = sec(4)?;
        let n_coefs = Cursor::at(bytes, s4).u32()? as usize;
        expect_len(s4, 4 + n_coefs * COEF_BYTES, 4)?;

        let a = sec(5)?;
        expect_len(a, n_vars * G1_BYTES, 5)?;
        let b1 = sec(6)?;
        expect_len(b1, n_vars * G1_BYTES, 6)?;
        let b2 = sec(7)?;
        expect_len(b2, n_vars * G2_BYTES, 7)?;
        let c = sec(8)?;
        expect_len(c, (n_vars - n_public - 1) * G1_BYTES, 8)?;
        let hs = sec(9)?;
        expect_len(hs, domain_size * G1_BYTES, 9)?;

        let header = ZkeyHeader {
            n_vars,
            n_public,
            domain_size,
            n_coefs,
        };
        let vk = VerifyingKey {
            alpha_g1,
            beta_g2,
            gamma_g2,
            delta_g2,
            gamma_abc_g1,
        };
        let coefs = Section {
            offset: s4.offset + 4,
            len: s4.len - 4,
        };
        let zkey = Self {
            map,
            header,
            vk,
            beta_g1,
            delta_g1,
            coefs,
            a,
            b1,
            b2,
            c,
            h: hs,
        };
        zkey.check_coefficients()?;
        Ok(zkey)
    }

    /// The mapped file bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    /// Header values.
    pub fn header(&self) -> &ZkeyHeader {
        &self.header
    }

    /// Verifying key embedded in the zkey.
    pub fn verifying_key(&self) -> &VerifyingKey<Bn254> {
        &self.vk
    }

    pub(crate) fn beta_g1(&self) -> G1Affine {
        self.beta_g1
    }

    pub(crate) fn delta_g1(&self) -> G1Affine {
        self.delta_g1
    }

    /// Validates every coefficient record's matrix id, constraint and signal
    /// index once at open time so the proving loop can index without checks.
    fn check_coefficients(&self) -> Result<(), ProverError> {
        let n_vars = self.header.n_vars;
        let domain = self.header.domain_size;
        let raw = &self.map[self.coefs.offset..self.coefs.offset + self.coefs.len];
        let ok = raw.par_chunks_exact(COEF_BYTES).all(|rec| {
            let matrix = u32le(&rec[0..4]);
            let constraint = u32le(&rec[4..8]) as usize;
            let signal = u32le(&rec[8..12]) as usize;
            matrix <= 1 && constraint < domain && signal < n_vars
        });
        if ok {
            Ok(())
        } else {
            Err(bad("coefficient record out of range"))
        }
    }

    /// Accumulates `A·w` (matrix 0) or `B·w` (matrix 1) into `out`
    /// (`out.len() == domain_size`). Includes the public-input rows that
    /// snarkjs appends after the circuit constraints.
    pub(crate) fn accumulate_matrix(&self, matrix: u32, w: &[Fr], out: &mut [Fr]) {
        let raw = &self.map[self.coefs.offset..self.coefs.offset + self.coefs.len];
        for rec in raw.chunks_exact(COEF_BYTES) {
            if u32le(&rec[0..4]) != matrix {
                continue;
            }
            let constraint = u32le(&rec[4..8]) as usize;
            let signal = u32le(&rec[8..12]) as usize;
            // Stored value is (coef · R) · R; one Montgomery reduction yields the
            // internal representation of coef.
            let value = Fr::new_unchecked(Fr::new_unchecked(bigint_le(&rec[12..44])).into_bigint());
            out[constraint] += value * w[signal];
        }
    }

    /// Point counts of the five query sections.
    pub(crate) fn query_len(&self, q: Query) -> usize {
        let s = self.section(q);
        s.len / q.point_bytes()
    }

    fn section(&self, q: Query) -> Section {
        match q {
            Query::A => self.a,
            Query::B1 => self.b1,
            Query::B2 => self.b2,
            Query::L => self.c,
            Query::H => self.h,
        }
    }

    /// Decodes G1 points `[start, start + out.len())` of a G1 query section.
    pub(crate) fn read_g1(&self, q: Query, start: usize, out: &mut Vec<G1Affine>, n: usize) {
        debug_assert!(q != Query::B2);
        let s = self.section(q);
        let raw = &self.map[s.offset + start * G1_BYTES..s.offset + (start + n) * G1_BYTES];
        out.clear();
        raw.par_chunks_exact(G1_BYTES)
            .map(unchecked_g1)
            .collect_into_vec(out);
    }

    /// Decodes G2 points `[start, start + n)` of the B2 section.
    pub(crate) fn read_g2(&self, start: usize, out: &mut Vec<G2Affine>, n: usize) {
        let s = self.b2;
        let raw = &self.map[s.offset + start * G2_BYTES..s.offset + (start + n) * G2_BYTES];
        out.clear();
        raw.par_chunks_exact(G2_BYTES)
            .map(unchecked_g2)
            .collect_into_vec(out);
    }
}

/// Query sections of the proving key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Query {
    A,
    B1,
    B2,
    L,
    H,
}

impl Query {
    fn point_bytes(self) -> usize {
        if self == Query::B2 {
            G2_BYTES
        } else {
            G1_BYTES
        }
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn at(buf: &'a [u8], s: Section) -> Self {
        Self {
            buf: &buf[..s.offset + s.len],
            pos: s.offset,
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProverError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let end = end.ok_or_else(|| bad("truncated zkey"))?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, ProverError> {
        Ok(u32le(self.take(4)?))
    }

    fn u64(&mut self) -> Result<u64, ProverError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
    }
}

fn bad(msg: impl Into<String>) -> ProverError {
    ProverError::InvalidArtifact(format!("zkey: {}", msg.into()))
}

fn expect_len(s: Section, want: usize, id: u32) -> Result<(), ProverError> {
    if s.len == want {
        Ok(())
    } else {
        Err(bad(format!(
            "section {id} has {} bytes, expected {want}",
            s.len
        )))
    }
}

#[inline]
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().expect("4 bytes"))
}

#[inline]
fn bigint_le(b: &[u8]) -> BigInt<4> {
    let mut limbs = [0u64; 4];
    for (i, limb) in limbs.iter_mut().enumerate() {
        *limb = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
    }
    BigInt::new(limbs)
}

fn le_modulus<F: PrimeField<BigInt = BigInt<4>>>() -> Vec<u8> {
    F::MODULUS.0.iter().flat_map(|l| l.to_le_bytes()).collect()
}

/// Montgomery-form Fq from 32 LE bytes, rejecting values >= q.
fn checked_fq(b: &[u8]) -> Result<Fq, ProverError> {
    let v = bigint_le(b);
    if v >= Fq::MODULUS {
        return Err(bad("non-canonical base field element"));
    }
    Ok(Fq::new_unchecked(v))
}

fn checked_g1(b: &[u8]) -> Result<G1Affine, ProverError> {
    let x = checked_fq(&b[..32])?;
    let y = checked_fq(&b[32..64])?;
    if b.iter().all(|&v| v == 0) {
        return Err(bad("verifying-key point at infinity"));
    }
    let p = G1Affine::new_unchecked(x, y);
    if !p.is_on_curve() || !p.is_in_correct_subgroup_assuming_on_curve() {
        return Err(bad("G1 point not on curve"));
    }
    Ok(p)
}

fn checked_g2(b: &[u8]) -> Result<G2Affine, ProverError> {
    let x = Fq2::new(checked_fq(&b[..32])?, checked_fq(&b[32..64])?);
    let y = Fq2::new(checked_fq(&b[64..96])?, checked_fq(&b[96..128])?);
    if b.iter().all(|&v| v == 0) {
        return Err(bad("verifying-key point at infinity"));
    }
    let p = G2Affine::new_unchecked(x, y);
    if !p.is_on_curve() || !p.is_in_correct_subgroup_assuming_on_curve() {
        return Err(bad(
            "G2 point not on curve or not in the prime-order subgroup",
        ));
    }
    Ok(p)
}

/// Bulk query points are decoded without curve checks (performance); a
/// corrupted proving key yields a proof that fails the mandatory self-check.
#[inline]
pub(crate) fn unchecked_g1(b: &[u8]) -> G1Affine {
    if b.iter().all(|&v| v == 0) {
        return G1Affine::zero();
    }
    G1Affine::new_unchecked(
        Fq::new_unchecked(bigint_le(&b[..32])),
        Fq::new_unchecked(bigint_le(&b[32..64])),
    )
}

#[inline]
pub(crate) fn unchecked_g2(b: &[u8]) -> G2Affine {
    if b.iter().all(|&v| v == 0) {
        return G2Affine::zero();
    }
    let x = Fq2::new(
        Fq::new_unchecked(bigint_le(&b[..32])),
        Fq::new_unchecked(bigint_le(&b[32..64])),
    );
    let y = Fq2::new(
        Fq::new_unchecked(bigint_le(&b[64..96])),
        Fq::new_unchecked(bigint_le(&b[96..128])),
    );
    G2Affine::new_unchecked(x, y)
}
