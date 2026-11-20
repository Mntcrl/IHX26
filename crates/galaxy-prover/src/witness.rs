//! Witness generation from an iden3 circom-witnesscalc graph (pure Rust, no
//! WASM runtime, no per-circuit native code).
//!
//! The graph file is produced by `build-circuit` from the same circom sources
//! and circom version as the R1CS used for the Groth16 setup (see
//! `scripts/prover-build-bench.sh`); its wire order then matches the zkey.

use std::fs::File;
use std::path::Path;

use ark_bn254::Fr;
use ark_ff::{BigInt, PrimeField};
use memmap2::Mmap;

use crate::ProverError;

/// A memory-mapped witness graph (`.wcd` / graph `.bin`).
pub struct WitnessGraph {
    map: Mmap,
}

impl WitnessGraph {
    /// Maps a graph file. The format magic is checked by circom-witnesscalc on use.
    pub fn open(path: &Path) -> Result<Self, ProverError> {
        let file = File::open(path)
            .map_err(|e| ProverError::Io(format!("open {}: {e}", path.display())))?;
        // SAFETY: read-only mapping of an immutable artifact (see zkey.rs).
        let map = unsafe { Mmap::map(&file) }
            .map_err(|e| ProverError::Io(format!("mmap {}: {e}", path.display())))?;
        if map.len() < 16 {
            return Err(ProverError::InvalidArtifact(
                "witness graph: file too short".into(),
            ));
        }
        Ok(Self { map })
    }

    /// The mapped file bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    /// Size of the graph file in bytes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Always false for an opened graph (kept for clippy's `len_without_is_empty`).
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Computes the full witness (wire 0 = 1) for circuit inputs in circom
    /// input JSON form.
    pub fn calculate(&self, inputs_json: &str) -> Result<Vec<Fr>, ProverError> {
        let wtns = circom_witnesscalc::calc_witness(inputs_json, &self.map)
            .map_err(|e| ProverError::Witness(e.to_string()))?;
        parse_wtns(&wtns)
    }
}

/// Parses an iden3 `.wtns` (v2) byte string into field elements, rejecting a
/// wrong prime, malformed sections and non-canonical values.
pub fn parse_wtns(b: &[u8]) -> Result<Vec<Fr>, ProverError> {
    let bad = |m: &str| ProverError::Witness(format!("wtns: {m}"));
    let rd_u32 = |at: usize| -> Result<u32, ProverError> {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes(s.try_into().expect("4 bytes")))
            .ok_or_else(|| bad("truncated"))
    };
    if b.get(0..4) != Some(b"wtns") {
        return Err(bad("bad magic"));
    }
    let n_sections = rd_u32(8)?;
    let mut pos = 12usize;
    let mut header: Option<(usize, usize)> = None; // (n_witness, data offset of section 2 unknown yet)
    let mut data: Option<&[u8]> = None;
    for _ in 0..n_sections {
        let id = rd_u32(pos)?;
        let len = b
            .get(pos + 4..pos + 12)
            .map(|s| u64::from_le_bytes(s.try_into().expect("8 bytes")) as usize)
            .ok_or_else(|| bad("truncated"))?;
        let start = pos + 12;
        let body = b
            .get(start..start + len)
            .ok_or_else(|| bad("truncated section"))?;
        match id {
            1 => {
                let n8 = rd_u32(start)? as usize;
                if n8 != 32 || len != 4 + n8 + 4 {
                    return Err(bad("unexpected field size"));
                }
                let modulus: Vec<u8> = Fr::MODULUS.0.iter().flat_map(|l| l.to_le_bytes()).collect();
                if body[4..36] != modulus[..] {
                    return Err(bad("prime is not BN254 Fr"));
                }
                header = Some((rd_u32(start + 36)? as usize, n8));
            }
            2 => data = Some(body),
            _ => return Err(bad("unknown section")),
        }
        pos = start + len;
    }
    if pos != b.len() {
        return Err(bad("trailing bytes"));
    }
    let (n, n8) = header.ok_or_else(|| bad("missing header"))?;
    let data = data.ok_or_else(|| bad("missing data"))?;
    if data.len() != n * n8 {
        return Err(bad("data length mismatch"));
    }
    let out = data
        .chunks_exact(32)
        .map(|c| {
            let mut limbs = [0u64; 4];
            for (i, l) in limbs.iter_mut().enumerate() {
                *l = u64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
            }
            Fr::from_bigint(BigInt::new(limbs))
        })
        .collect::<Option<Vec<Fr>>>()
        .ok_or_else(|| bad("non-canonical witness value"))?;
    if out.first() != Some(&Fr::from(1u64)) {
        return Err(bad("wire 0 is not 1"));
    }
    Ok(out)
}
