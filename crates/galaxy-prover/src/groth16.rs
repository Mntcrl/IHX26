//! Groth16 prover over a memory-mapped snarkjs zkey, with phase progress and
//! cancellation.
//!
//! The algorithm is the snarkjs / arkworks Groth16 prover with the circom
//! R1CS-to-QAP reduction (as in ark-circom's `CircomReduction`): `H` is
//! evaluated on the odd coset of the 2n-th roots of unity because snarkjs
//! stores the `H` query in that Lagrange basis. All curve and field arithmetic
//! is arkworks (MSM, FFT); this module only orders the phases so that the
//! proving key is streamed from the mapping in bounded chunks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ark_bn254::{Bn254, Fr, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{BigInt, Field, PrimeField, Zero};
use ark_groth16::Proof;
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use rayon::prelude::*;

use crate::zkey::{Query, Zkey};
use crate::{Phase, ProveControl, ProverError, Timings};

/// Prover tuning knobs.
#[derive(Clone, Copy, Debug)]
pub struct ProverConfig {
    /// Maximum number of bases decoded and multiplied per MSM call. Smaller
    /// values lower peak memory (each G1 base is 72 B, each G2 base 136 B in
    /// memory) at the cost of MSM efficiency.
    pub msm_chunk: usize,
}

impl Default for ProverConfig {
    fn default() -> Self {
        Self { msm_chunk: 1 << 20 }
    }
}

pub(crate) struct Phases<'a, 'b> {
    pub(crate) ctl: &'a ProveControl<'b>,
    pub(crate) timings: Timings,
    current: Option<(Phase, Instant)>,
}

impl<'a, 'b> Phases<'a, 'b> {
    pub(crate) fn new(ctl: &'a ProveControl<'b>) -> Self {
        Self {
            ctl,
            timings: Timings::default(),
            current: None,
        }
    }

    /// Ends the running phase, checks cancellation and starts `phase`.
    pub(crate) fn start(&mut self, phase: Phase) -> Result<(), ProverError> {
        self.finish();
        if self.ctl.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(ProverError::Cancelled);
        }
        if let Some(p) = self.ctl.progress {
            p.phase_started(phase);
        }
        self.current = Some((phase, Instant::now()));
        Ok(())
    }

    pub(crate) fn finish(&mut self) {
        if let Some((phase, t0)) = self.current.take() {
            let secs = t0.elapsed().as_secs_f64();
            self.timings.add(phase, secs);
            if let Some(p) = self.ctl.progress {
                p.phase_finished(phase, secs);
            }
        }
    }
}

fn cancelled(flag: Option<&AtomicBool>) -> Result<(), ProverError> {
    if flag.is_some_and(|c| c.load(Ordering::Relaxed)) {
        Err(ProverError::Cancelled)
    } else {
        Ok(())
    }
}

/// Uniform scalar from 64 bytes of OS randomness (statistical distance 2^-258).
fn random_scalar() -> Result<Fr, ProverError> {
    let mut buf = [0u8; 64];
    getrandom::fill(&mut buf).map_err(|e| ProverError::Rng(e.to_string()))?;
    let v = Fr::from_le_bytes_mod_order(&buf);
    if v.is_zero() {
        // Probability 2^-254; refuse rather than produce a non-hiding proof.
        return Err(ProverError::Rng("sampled zero blinding scalar".into()));
    }
    Ok(v)
}

fn msm_g1(
    zkey: &Zkey,
    q: Query,
    scalars: &[BigInt<4>],
    cfg: &ProverConfig,
    cancel: Option<&AtomicBool>,
) -> Result<G1Projective, ProverError> {
    debug_assert_eq!(zkey.query_len(q), scalars.len());
    let mut acc = G1Projective::zero();
    let mut bases: Vec<G1Affine> = Vec::new();
    for (i, chunk) in scalars.chunks(cfg.msm_chunk.max(1)).enumerate() {
        cancelled(cancel)?;
        zkey.read_g1(q, i * cfg.msm_chunk.max(1), &mut bases, chunk.len());
        acc += G1Projective::msm_bigint(&bases, chunk);
    }
    Ok(acc)
}

fn msm_g2(
    zkey: &Zkey,
    scalars: &[BigInt<4>],
    cfg: &ProverConfig,
    cancel: Option<&AtomicBool>,
) -> Result<G2Projective, ProverError> {
    let mut acc = G2Projective::zero();
    let mut bases: Vec<G2Affine> = Vec::new();
    for (i, chunk) in scalars.chunks(cfg.msm_chunk.max(1)).enumerate() {
        cancelled(cancel)?;
        zkey.read_g2(i * cfg.msm_chunk.max(1), &mut bases, chunk.len());
        acc += G2Projective::msm_bigint(&bases, chunk);
    }
    Ok(acc)
}

/// Computes the `H` evaluations (snarkjs layout) from the witness.
fn witness_map(zkey: &Zkey, w: &[Fr]) -> Result<Vec<Fr>, ProverError> {
    let n = zkey.header().domain_size;
    let domain = Radix2EvaluationDomain::<Fr>::new(n)
        .filter(|d| d.size() == n)
        .ok_or_else(|| ProverError::InvalidArtifact("unsupported domain size".into()))?;
    let mut a = vec![Fr::zero(); n];
    let mut b = vec![Fr::zero(); n];
    rayon::join(
        || zkey.accumulate_matrix(0, w, &mut a),
        || zkey.accumulate_matrix(1, w, &mut b),
    );
    // C·w = A·w ∘ B·w for a satisfying witness (snarkjs does not store C).
    let mut c: Vec<Fr> = a
        .par_iter()
        .zip(b.par_iter())
        .map(|(x, y)| *x * y)
        .collect();

    // Shift to the odd coset: g = primitive 2n-th root of unity.
    let g = Radix2EvaluationDomain::<Fr>::new(2 * n)
        .ok_or_else(|| ProverError::InvalidArtifact("domain too large".into()))?
        .element(1);
    for v in [&mut a, &mut b, &mut c] {
        domain.ifft_in_place(v);
        Radix2EvaluationDomain::<Fr>::distribute_powers_and_mul_by_const(v, g, Fr::ONE);
        domain.fft_in_place(v);
    }
    a.par_iter_mut()
        .zip(b.par_iter())
        .zip(c.par_iter())
        .for_each(|((x, y), z)| {
            *x = *x * y - z;
        });
    Ok(a)
}

/// Runs the Groth16 prover phases after witness generation. Returns the proof.
pub(crate) fn prove_with_witness(
    zkey: &Zkey,
    w: &[Fr],
    cfg: &ProverConfig,
    phases: &mut Phases<'_, '_>,
) -> Result<Proof<Bn254>, ProverError> {
    let hdr = zkey.header();
    if w.len() != hdr.n_vars {
        return Err(ProverError::Witness(format!(
            "witness has {} wires, proving key expects {}",
            w.len(),
            hdr.n_vars
        )));
    }
    let cancel = phases.ctl.cancel;
    let r = random_scalar()?;
    let s = random_scalar()?;

    phases.start(Phase::WitnessMap)?;
    let h = witness_map(zkey, w)?;

    phases.start(Phase::MsmH)?;
    let h_big: Vec<BigInt<4>> = h.par_iter().map(|v| v.into_bigint()).collect();
    drop(h);
    let h_acc = msm_g1(zkey, Query::H, &h_big, cfg, cancel)?;
    drop(h_big);

    phases.start(Phase::MsmL)?;
    let w_big: Vec<BigInt<4>> = w.par_iter().map(|v| v.into_bigint()).collect();
    let l_acc = msm_g1(zkey, Query::L, &w_big[hdr.n_public + 1..], cfg, cancel)?;

    phases.start(Phase::MsmA)?;
    let vk = zkey.verifying_key();
    let delta_g1 = zkey.delta_g1();
    let g_a = msm_g1(zkey, Query::A, &w_big, cfg, cancel)? + vk.alpha_g1 + delta_g1 * r;

    phases.start(Phase::MsmB1)?;
    let g1_b = msm_g1(zkey, Query::B1, &w_big, cfg, cancel)? + zkey.beta_g1() + delta_g1 * s;

    phases.start(Phase::MsmB2)?;
    let g2_b = msm_g2(zkey, &w_big, cfg, cancel)? + vk.beta_g2 + vk.delta_g2 * s;
    drop(w_big);

    phases.start(Phase::Finalize)?;
    let g_c = g_a * s + g1_b * r - delta_g1 * (r * s) + l_acc + h_acc;
    let proof: Proof<Bn254> = Proof {
        a: g_a.into_affine(),
        b: g2_b.into_affine(),
        c: g_c.into_affine(),
    };
    if proof.a.xy().is_none() || proof.b.xy().is_none() || proof.c.xy().is_none() {
        return Err(ProverError::SelfCheck("proof element at infinity".into()));
    }
    Ok(proof)
}

/// Verifies a proof with arkworks (prepared VK), public inputs without the
/// leading 1.
pub(crate) fn verify_proof(
    vk: &ark_groth16::VerifyingKey<Bn254>,
    public: &[Fr],
    proof: &Proof<Bn254>,
) -> Result<bool, ProverError> {
    if public.len() + 1 != vk.gamma_abc_g1.len() {
        return Err(ProverError::InvalidEncoding(format!(
            "expected {} public inputs, got {}",
            vk.gamma_abc_g1.len() - 1,
            public.len()
        )));
    }
    let pvk = ark_groth16::prepare_verifying_key(vk);
    ark_groth16::Groth16::<Bn254>::verify_proof(&pvk, proof, public)
        .map_err(|e| ProverError::InvalidEncoding(e.to_string()))
}
