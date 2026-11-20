//! UniFFI surface (feature `uniffi`) used by apps/prover-bench-ios and
//! re-exported by galaxy-ffi.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{codec, Phase, ProveControl, Progress, Prover, ProverConfig, ProverError};

/// Phase notifications delivered to Swift (on the proving thread).
#[uniffi::export(with_foreign)]
pub trait ProgressListener: Send + Sync {
    /// Phase `phase` (snake_case name) starts.
    fn on_phase_started(&self, phase: String);
    /// Phase `phase` ended after `seconds`.
    fn on_phase_finished(&self, phase: String, seconds: f64);
}

struct ListenerAdapter(Arc<dyn ProgressListener>);

impl Progress for ListenerAdapter {
    fn phase_started(&self, phase: Phase) {
        self.0.on_phase_started(phase.name().to_owned());
    }
    fn phase_finished(&self, phase: Phase, seconds: f64) {
        self.0.on_phase_finished(phase.name().to_owned(), seconds);
    }
}

/// Cooperative cancellation flag, checked between proving phases and MSM chunks.
#[derive(uniffi::Object, Default)]
pub struct CancelToken {
    flag: AtomicBool,
}

#[uniffi::export]
impl CancelToken {
    /// New, not cancelled.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Time spent in one phase.
#[derive(uniffi::Record, Clone, Debug)]
pub struct PhaseTiming {
    /// snake_case phase name.
    pub phase: String,
    /// Seconds.
    pub seconds: f64,
}

/// A proof produced on this device.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ProofResult {
    /// 256-byte proof (spec §9).
    pub proof: Vec<u8>,
    /// Public inputs, 32-byte big-endian each.
    pub public_inputs: Vec<Vec<u8>>,
    /// Public inputs as decimal strings.
    pub public_inputs_decimal: Vec<String>,
    /// Per-phase timings.
    pub phases: Vec<PhaseTiming>,
    /// Witness generation seconds.
    pub witness_seconds: f64,
    /// Groth16 proving seconds (without witness and self-verify).
    pub prove_seconds: f64,
    /// snarkjs-compatible proof.json (development cross-check).
    pub snarkjs_proof_json: String,
    /// snarkjs-compatible public.json.
    pub snarkjs_public_json: String,
}

/// Memory-mapped proving artifacts.
#[derive(uniffi::Object)]
pub struct GalaxyProver {
    inner: Prover,
}

#[uniffi::export]
impl GalaxyProver {
    /// Opens a witness graph and a snarkjs zkey. `msm_chunk` = 0 selects the default.
    #[uniffi::constructor]
    pub fn open(graph_path: String, zkey_path: String, msm_chunk: u32) -> Result<Arc<Self>, ProverError> {
        let mut cfg = ProverConfig::default();
        if msm_chunk > 0 {
            cfg.msm_chunk = msm_chunk as usize;
        }
        let inner = Prover::open_with_config(Path::new(&graph_path), Path::new(&zkey_path), cfg)?;
        Ok(Arc::new(Self { inner }))
    }

    /// Number of public inputs.
    pub fn n_public(&self) -> u32 {
        self.inner.n_public() as u32
    }

    /// Number of wires in the proving key.
    pub fn n_vars(&self) -> u64 {
        self.inner.zkey_header().n_vars as u64
    }

    /// FFT domain size of the proving key.
    pub fn domain_size(&self) -> u64 {
        self.inner.zkey_header().domain_size as u64
    }

    /// Spec §9 VK bytes.
    pub fn vk_bytes(&self) -> Result<Vec<u8>, ProverError> {
        self.inner.vk_bytes()
    }

    /// Generates witness + Groth16 proof and self-verifies it.
    pub fn prove(
        &self,
        inputs_json: String,
        listener: Option<Arc<dyn ProgressListener>>,
        cancel: Option<Arc<CancelToken>>,
    ) -> Result<ProofResult, ProverError> {
        let adapter = listener.map(ListenerAdapter);
        let ctl = ProveControl {
            progress: adapter.as_ref().map(|a| a as &dyn Progress),
            cancel: cancel.as_deref().map(|c| &c.flag),
        };
        let (proof, public, timings) = self.inner.prove_ark(&inputs_json, &ctl)?;
        Ok(ProofResult {
            proof: codec::encode_proof(&proof)?.to_vec(),
            public_inputs: public.iter().map(|v| codec::encode_scalar(v).to_vec()).collect(),
            public_inputs_decimal: public.iter().map(codec::to_decimal).collect(),
            phases: Phase::ALL
                .iter()
                .map(|p| PhaseTiming { phase: p.name().to_owned(), seconds: timings.get(*p) })
                .collect(),
            witness_seconds: timings.witness(),
            prove_seconds: timings.prove(),
            snarkjs_proof_json: codec::proof_to_snarkjs_json(&proof)?,
            snarkjs_public_json: codec::public_to_snarkjs_json(&public),
        })
    }
}

/// Verifies a 256-byte proof against spec §9 VK bytes and 32-byte public inputs.
#[uniffi::export]
pub fn verify_proof(vk_bytes: Vec<u8>, public_inputs: Vec<Vec<u8>>, proof: Vec<u8>) -> Result<bool, ProverError> {
    let public = public_inputs
        .iter()
        .map(|b| <[u8; 32]>::try_from(b.as_slice()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ProverError::InvalidEncoding("public input must be 32 bytes".into()))?;
    crate::verify(&vk_bytes, &public, &proof)
}
