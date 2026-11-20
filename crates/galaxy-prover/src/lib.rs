//! Proving backends: witness generation and Groth16 proving for desktop and iOS (spec §9).
//!
//! Backend (ADR 0080): iden3 circom-witnesscalc graph witness generation +
//! arkworks Groth16 over a memory-mapped snarkjs `.zkey` (BN254), pure Rust,
//! cross-compiled for `aarch64-apple-ios` on Linux. Proofs are always
//! self-verified against the zkey's verifying key before being returned.
//!
//! ```no_run
//! use galaxy_prover::{Prover, ProveControl};
//! let prover = Prover::open("circuit.graph.bin".as_ref(), "circuit.zkey".as_ref())?;
//! let out = prover.prove(r#"{"a": "1"}"#, &ProveControl::default())?;
//! assert!(galaxy_prover::verify(&prover.vk_bytes()?, &out.public_inputs, &out.proof)?);
//! # Ok::<(), galaxy_prover::ProverError>(())
//! ```

pub mod codec;
pub mod dev_setup;
mod groth16;
pub mod witness;
pub mod zkey;

#[cfg(feature = "uniffi")]
pub mod ffi;
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!("galaxy_prover");

use std::path::Path;
use std::sync::atomic::AtomicBool;

use ark_bn254::{Bn254, Fr};
use ark_groth16::{Proof, VerifyingKey};

pub use groth16::ProverConfig;
use sha2::{Digest, Sha256};

use crate::groth16::Phases;
use crate::witness::WitnessGraph;
use crate::zkey::Zkey;

/// Prover errors.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error), uniffi(flat_error))]
pub enum ProverError {
    /// File system error while opening artifacts.
    #[error("io: {0}")]
    Io(String),
    /// Malformed or unsupported proving artifact.
    #[error("invalid artifact: {0}")]
    InvalidArtifact(String),
    /// Witness generation failed (bad inputs, unsatisfied assertion, graph error).
    #[error("witness: {0}")]
    Witness(String),
    /// Malformed proof, VK or public input encoding.
    #[error("invalid encoding: {0}")]
    InvalidEncoding(String),
    /// OS randomness unavailable.
    #[error("rng: {0}")]
    Rng(String),
    /// The produced proof does not verify (corrupt proving key or unsatisfied constraints).
    #[error("self-check failed: {0}")]
    SelfCheck(String),
    /// The caller's cancellation flag was set.
    #[error("cancelled")]
    Cancelled,
}

/// Proving phases, in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Witness generation from the graph.
    Witness,
    /// A·w, B·w, iFFT/coset FFT, H evaluations.
    WitnessMap,
    /// MSM over the H query.
    MsmH,
    /// MSM over the L (C) query.
    MsmL,
    /// MSM over the A query.
    MsmA,
    /// MSM over the B query in G1.
    MsmB1,
    /// MSM over the B query in G2.
    MsmB2,
    /// Proof assembly.
    Finalize,
    /// Verification of the proof against the zkey verifying key.
    SelfVerify,
}

impl Phase {
    /// All phases in order.
    pub const ALL: [Phase; 9] = [
        Phase::Witness,
        Phase::WitnessMap,
        Phase::MsmH,
        Phase::MsmL,
        Phase::MsmA,
        Phase::MsmB1,
        Phase::MsmB2,
        Phase::Finalize,
        Phase::SelfVerify,
    ];

    /// Stable snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Witness => "witness",
            Phase::WitnessMap => "witness_map",
            Phase::MsmH => "msm_h",
            Phase::MsmL => "msm_l",
            Phase::MsmA => "msm_a",
            Phase::MsmB1 => "msm_b1",
            Phase::MsmB2 => "msm_b2",
            Phase::Finalize => "finalize",
            Phase::SelfVerify => "self_verify",
        }
    }

    fn index(self) -> usize {
        Phase::ALL
            .iter()
            .position(|p| *p == self)
            .expect("phase listed")
    }
}

/// Seconds spent per phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timings {
    secs: [f64; 9],
}

impl Timings {
    fn add(&mut self, p: Phase, s: f64) {
        self.secs[p.index()] += s;
    }

    /// Seconds spent in `p`.
    pub fn get(&self, p: Phase) -> f64 {
        self.secs[p.index()]
    }

    /// Witness generation seconds.
    pub fn witness(&self) -> f64 {
        self.get(Phase::Witness)
    }

    /// Groth16 proving seconds (witness map through finalize, without self-verify).
    pub fn prove(&self) -> f64 {
        Phase::ALL[1..8].iter().map(|p| self.get(*p)).sum()
    }

    /// Sum over all phases.
    pub fn total(&self) -> f64 {
        self.secs.iter().sum()
    }
}

/// Receives phase notifications. Called on the proving thread.
pub trait Progress: Send + Sync {
    /// A phase starts.
    fn phase_started(&self, phase: Phase);
    /// A phase ended after `seconds`.
    fn phase_finished(&self, phase: Phase, seconds: f64);
}

/// Progress sink and cancellation flag for one proof. The flag is checked
/// before every phase and between MSM chunks.
#[derive(Clone, Copy, Default)]
pub struct ProveControl<'a> {
    /// Optional progress sink.
    pub progress: Option<&'a dyn Progress>,
    /// Optional cancellation flag; setting it aborts with [`ProverError::Cancelled`].
    pub cancel: Option<&'a AtomicBool>,
}

/// A produced proof.
#[derive(Clone, Debug)]
pub struct ProofOutput {
    /// 256-byte proof (spec §9).
    pub proof: [u8; codec::PROOF_BYTES],
    /// Public inputs in circuit order, 32-byte big-endian each.
    pub public_inputs: Vec<[u8; 32]>,
    /// Phase timings.
    pub timings: Timings,
}

/// Pinned artifact digests from the circuit descriptor (spec §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactDigests {
    /// SHA-256 of the witness graph file (`witness_graph_sha256`).
    pub witness_graph_sha256: [u8; 32],
    /// SHA-256 of the proving key file (`zkey_sha256`).
    pub zkey_sha256: [u8; 32],
}

/// Loaded proving artifacts: witness graph + proving key (both memory-mapped).
pub struct Prover {
    graph: WitnessGraph,
    zkey: Zkey,
    config: ProverConfig,
}

impl Prover {
    /// Opens a witness graph and a snarkjs Groth16 zkey with the default config.
    pub fn open(graph_path: &Path, zkey_path: &Path) -> Result<Self, ProverError> {
        Self::open_with_config(graph_path, zkey_path, ProverConfig::default())
    }

    /// Opens artifacts with an explicit config.
    pub fn open_with_config(
        graph_path: &Path,
        zkey_path: &Path,
        config: ProverConfig,
    ) -> Result<Self, ProverError> {
        let graph = WitnessGraph::open(graph_path)?;
        let zkey = Zkey::open(zkey_path)?;
        Ok(Self {
            graph,
            zkey,
            config,
        })
    }

    /// Opens artifacts and checks them against the circuit descriptor's
    /// `witness_graph_sha256` and `zkey_sha256` (spec §9). The digests are computed
    /// over the mapped bytes the prover then uses; a mismatch is `InvalidArtifact`.
    pub fn open_pinned(
        graph_path: &Path,
        zkey_path: &Path,
        digests: &ArtifactDigests,
        config: ProverConfig,
    ) -> Result<Self, ProverError> {
        let prover = Self::open_with_config(graph_path, zkey_path, config)?;
        for (what, bytes, want) in [
            (
                "witness graph",
                prover.graph.bytes(),
                &digests.witness_graph_sha256,
            ),
            ("zkey", prover.zkey.bytes(), &digests.zkey_sha256),
        ] {
            let got: [u8; 32] = Sha256::digest(bytes).into();
            if &got != want {
                return Err(ProverError::InvalidArtifact(format!(
                    "{what}: SHA-256 does not match the circuit descriptor"
                )));
            }
        }
        Ok(prover)
    }

    /// Number of public inputs of the circuit.
    pub fn n_public(&self) -> usize {
        self.zkey.header().n_public
    }

    /// Wire count, constraint-domain size and coefficient count of the proving key.
    pub fn zkey_header(&self) -> &zkey::ZkeyHeader {
        self.zkey.header()
    }

    /// Verifying key of the loaded proving key.
    pub fn verifying_key(&self) -> &VerifyingKey<Bn254> {
        self.zkey.verifying_key()
    }

    /// Spec §9 VK bytes of the loaded proving key; callers compare its digest
    /// with the accepted circuit's `vk_digest` before publishing proofs.
    pub fn vk_bytes(&self) -> Result<Vec<u8>, ProverError> {
        codec::encode_vk(self.zkey.verifying_key())
    }

    /// Generates the witness only (benchmarks / diagnostics).
    pub fn witness(&self, inputs_json: &str) -> Result<Vec<Fr>, ProverError> {
        self.graph.calculate(inputs_json)
    }

    /// Full proof: witness → Groth16 → self-verification.
    pub fn prove(
        &self,
        inputs_json: &str,
        ctl: &ProveControl<'_>,
    ) -> Result<ProofOutput, ProverError> {
        let (proof, public, timings) = self.prove_ark(inputs_json, ctl)?;
        Ok(ProofOutput {
            proof: codec::encode_proof(&proof)?,
            public_inputs: public.iter().map(codec::encode_scalar).collect(),
            timings,
        })
    }

    /// Like [`Prover::prove`], returning arkworks types.
    pub fn prove_ark(
        &self,
        inputs_json: &str,
        ctl: &ProveControl<'_>,
    ) -> Result<(Proof<Bn254>, Vec<Fr>, Timings), ProverError> {
        let mut phases = Phases::new(ctl);
        phases.start(Phase::Witness)?;
        let w = self.graph.calculate(inputs_json)?;
        let proof = groth16::prove_with_witness(&self.zkey, &w, &self.config, &mut phases)?;
        let public = w[1..=self.n_public()].to_vec();
        drop(w);
        phases.start(Phase::SelfVerify)?;
        let ok = groth16::verify_proof(self.zkey.verifying_key(), &public, &proof)?;
        phases.finish();
        if !ok {
            return Err(ProverError::SelfCheck(
                "proof does not verify against the zkey verifying key".into(),
            ));
        }
        Ok((proof, public, phases.timings))
    }
}

/// Verifies a 256-byte proof against spec §9 VK bytes and 32-byte big-endian
/// public inputs. Malformed encodings are errors; a well-formed proof that
/// does not verify returns `Ok(false)`.
pub fn verify(
    vk_bytes: &[u8],
    public_inputs: &[[u8; 32]],
    proof: &[u8],
) -> Result<bool, ProverError> {
    let vk = codec::decode_vk(vk_bytes)?;
    let public = public_inputs
        .iter()
        .map(|b| codec::decode_scalar(b))
        .collect::<Result<Vec<_>, _>>()?;
    let proof = codec::decode_proof(proof)?;
    groth16::verify_proof(&vk, &public, &proof)
}

/// Verifies arkworks values directly.
pub fn verify_ark(
    vk: &VerifyingKey<Bn254>,
    public: &[Fr],
    proof: &Proof<Bn254>,
) -> Result<bool, ProverError> {
    groth16::verify_proof(vk, public, proof)
}
