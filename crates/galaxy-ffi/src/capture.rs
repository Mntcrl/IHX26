use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use galaxy_crypto::{
    field::{fr_from_be_canonical, fr_to_be},
    public::public_inputs_v1,
    sampling::Challenge,
    Fr,
};
use galaxy_geometry::{build_view, try_evaluate, GeometryView, PolicyReport};
use galaxy_protocol::{
    ids,
    network::{NetworkConfig, NetworkId},
    policy::Policy,
    wire::{CaptureCore, CaptureMemo, MemoKind, ProofMemo, MAX_BUNDLE_LEN},
    xmp::{self, GalaxyXmp},
    Hash32,
};
use galaxy_prover::{
    self as prover, ArtifactDigests, Phase, Progress, ProveControl, Prover, ProverConfig,
    ProverError,
};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::{
    exact, guarded, hash_hex, network_id, CaptureEvidence, GalaxyError, Memo, Result,
    ValidatedLedger,
};

const BUNDLED_POLICY: &[u8] = include_bytes!(env!("GALAXY_BUNDLED_POLICY"));
const BUNDLED_NETWORK: &str = include_str!(env!("GALAXY_BUNDLED_NETWORK"));
const PUBLIC_COUNT: usize = 22;

#[derive(Deserialize)]
struct Descriptor {
    version: u32,
    profile: String,
    policy_id: String,
    witness_graph_sha256: String,
    zkey_sha256: String,
    vk_digest: String,
    n_public: usize,
    public_layout: String,
    ceremony: String,
}

/// Operator-selected trust anchor, not metadata read from an XRPL response.
#[derive(uniffi::Object)]
pub struct CaptureCircuit {
    network: NetworkConfig,
    circuit_id: Hash32,
    policy_id: Hash32,
    circuit_registration_tx: Hash32,
    vk_digest: Hash32,
    digests: ArtifactDigests,
    ceremony: String,
    policy: Policy,
}

#[derive(uniffi::Record)]
pub struct CapturePolicyLimits {
    pub capture_window: u32,
    pub selection_offset: u32,
    pub proof_window: u32,
    pub baseline_min_um: i64,
    pub rotation_max_q30: i64,
    pub depth_min_mm: i64,
    pub depth_max_mm: i64,
    pub depth_conf_min: i64,
}

#[derive(uniffi::Record)]
pub struct CaptureNetwork {
    pub rpc_url: String,
    pub xrpl: crate::XrplNetwork,
    pub accepted_app_ids: Vec<String>,
    pub circuit_registration_tx: String,
}

#[derive(uniffi::Record)]
pub struct CaptureParameters {
    pub network: u32,
    pub registration_tx: String,
    pub preledger: ValidatedLedger,
    pub nonce: Vec<u8>,
    pub claimed_time_ms: u64,
    pub flags: u16,
    pub account_id: Vec<u8>,
}

#[derive(uniffi::Record)]
pub struct CaptureStatement {
    pub core: Vec<u8>,
    pub capture_id: Vec<u8>,
    pub assertion_client_data_hash: Vec<u8>,
    pub capture_deadline: u32,
}

fn add_window(index: u32, offset: u32) -> Result<u32> {
    index
        .checked_add(offset)
        .filter(|_| index != 0)
        .ok_or(GalaxyError::LedgerWindow)
}

fn checked_core(core: &[u8]) -> Result<CaptureCore> {
    let core = CaptureCore::decode(core).map_err(|_| GalaxyError::CaptureBinding)?;
    if usize::from(core.width) != galaxy_image::WIDTH
        || usize::from(core.height) != galaxy_image::HEIGHT
    {
        return Err(GalaxyError::CaptureBinding);
    }
    Ok(core)
}

#[uniffi::export]
pub fn build_capture_memo(core: Vec<u8>, assertion: Vec<u8>) -> Result<Memo> {
    guarded(|| {
        let core = checked_core(&core)?;
        // Structural check only. XRPL verifiers still authenticate the signature/counter.
        galaxy_attestation::assertion_counter_hint(&assertion)
            .map_err(|_| GalaxyError::Attestation)?;
        Ok(Memo::new(
            MemoKind::Capture,
            CaptureMemo { core, assertion }.encode(),
        ))
    })
}

#[uniffi::export]
impl CaptureCircuit {
    /// Only entries in the build-time network allowlist can authorize this descriptor.
    #[uniffi::constructor]
    pub fn configure(descriptor: Vec<u8>) -> Result<Arc<Self>> {
        guarded(|| Self::from_pins(&descriptor, BUNDLED_POLICY, BUNDLED_NETWORK).map(Arc::new))
    }

    pub fn network(&self) -> CaptureNetwork {
        CaptureNetwork {
            rpc_url: self.network.json_rpc.clone(),
            xrpl: crate::XrplNetwork {
                network_id: self.network.network_id.as_u32(),
                allow_mainnet: self.network.allow_mainnet,
                max_fee_drops: self.network.max_fee_drops,
            },
            accepted_app_ids: self.network.accepted_app_ids.clone(),
            circuit_registration_tx: hex::encode_upper(self.circuit_registration_tx),
        }
    }

    pub fn ceremony(&self) -> String {
        self.ceremony.clone()
    }

    pub fn policy_limits(&self) -> CapturePolicyLimits {
        CapturePolicyLimits {
            capture_window: self.policy.windows.w_cap,
            selection_offset: self.policy.windows.sel_offset,
            proof_window: self.policy.windows.w_proof,
            baseline_min_um: self.policy.motion.baseline_min_um,
            rotation_max_q30: self.policy.motion.rot_max_q30,
            depth_min_mm: self.policy.lidar.d_min_mm,
            depth_max_mm: self.policy.lidar.d_max_mm,
            depth_conf_min: self.policy.lidar.conf_min,
        }
    }

    pub fn build_capture_statement(
        &self,
        evidence: Arc<CaptureEvidence>,
        input: CaptureParameters,
    ) -> Result<CaptureStatement> {
        guarded(|| {
            if input.network != self.network.network_id.as_u32() {
                return Err(GalaxyError::Network);
            }
            let core = CaptureCore {
                network: input.network,
                registration_tx: hash_hex(&input.registration_tx)?,
                preledger_hash: input.preledger.checked_hash()?,
                nonce: exact(&input.nonce)?,
                preledger_index: input.preledger.index,
                claimed_time_ms: input.claimed_time_ms,
                width: galaxy_image::WIDTH as u16,
                height: galaxy_image::HEIGHT as u16,
                flags: input.flags,
                photo_id: evidence.photo_id,
                rgb_root: fr_to_be(&evidence.commitments.rgb_root),
                evidence_root: fr_to_be(&evidence.commitments.evidence_root),
                policy_id: self.policy_id,
                circuit_id: self.circuit_id,
                account_id: exact(&input.account_id)?,
            };
            if core.flags != u16::from(evidence.profile().code()) {
                return Err(GalaxyError::CaptureBinding);
            }
            let capture_deadline = add_window(core.preledger_index, self.policy.windows.w_cap)?;
            if !evidence.statement_available.swap(false, Ordering::AcqRel) {
                return Err(GalaxyError::CaptureBinding);
            }
            let core = core.encode();
            Ok(CaptureStatement {
                capture_id: ids::capture_id(&core).to_vec(),
                assertion_client_data_hash: ids::assertion_client_data_hash(&core).to_vec(),
                core,
                capture_deadline,
            })
        })
    }

    /// `capture_ledger` is L0, from the matching validated tesSUCCESS CAPTURE.
    pub fn selection_ledger_index(&self, core: Vec<u8>, capture_ledger: u32) -> Result<u32> {
        guarded(|| self.selection_index(&self.checked_core(&core)?, capture_ledger))
    }

    /// Reconstitute the only challenge from the journaled core and validated ledgers.
    pub fn derive_challenge(
        self: Arc<Self>,
        core: Vec<u8>,
        capture_tx: String,
        capture_ledger: u32,
        selection_ledger: ValidatedLedger,
    ) -> Result<Arc<CaptureChallenge>> {
        guarded(|| {
            let statement = self.checked_core(&core)?;
            if selection_ledger.index != self.selection_index(&statement, capture_ledger)? {
                return Err(GalaxyError::LedgerWindow);
            }
            let selection_hash = selection_ledger.checked_hash()?;
            let proof_deadline = add_window(selection_ledger.index, self.policy.windows.w_proof)?;
            let capture_tx = hash_hex(&capture_tx)?;
            let capture_id = ids::capture_id(&core);
            let challenge = Challenge::derive(
                statement.network,
                &capture_id,
                selection_ledger.index,
                &selection_hash,
            );
            let rgb_root = fr_from_be_canonical(&statement.rgb_root)
                .map_err(|_| GalaxyError::CaptureBinding)?;
            let evidence_root = fr_from_be_canonical(&statement.evidence_root)
                .map_err(|_| GalaxyError::CaptureBinding)?;
            let public_inputs = public_inputs_v1(
                &rgb_root,
                &evidence_root,
                &capture_id,
                &challenge.seed,
                &challenge.tiles,
            );
            Ok(Arc::new(CaptureChallenge {
                circuit: self,
                statement,
                capture_tx,
                capture_id,
                selection_index: selection_ledger.index,
                selection_hash,
                proof_deadline,
                challenge,
                public_inputs,
            }))
        })
    }

    /// Mapped files must not be changed for the entire lifetime of the prover.
    pub fn open_prover(&self, graph_path: String, zkey_path: String) -> Result<Arc<CaptureProver>> {
        guarded(|| {
            let prover = Prover::open_pinned(
                Path::new(&graph_path),
                Path::new(&zkey_path),
                &self.digests,
                ProverConfig::default(),
            )
            .map_err(|_| GalaxyError::Circuit)?;
            let vk = prover.vk_bytes().map_err(|_| GalaxyError::Circuit)?;
            if prover.n_public() != PUBLIC_COUNT || ids::vk_digest(&vk) != self.vk_digest {
                return Err(GalaxyError::Circuit);
            }
            Ok(Arc::new(CaptureProver {
                prover,
                circuit_id: self.circuit_id,
            }))
        })
    }
}

impl CaptureCircuit {
    fn from_pins(descriptor: &[u8], policy_json: &[u8], network_toml: &str) -> Result<Self> {
        if descriptor.len() > MAX_BUNDLE_LEN {
            return Err(GalaxyError::Circuit);
        }
        let network = NetworkConfig::from_toml(network_toml).map_err(|_| GalaxyError::Network)?;
        let circuit_id = ids::circuit_id(descriptor).map_err(|_| GalaxyError::Circuit)?;
        let accepted = network
            .accepted_circuit(&circuit_id)
            .ok_or(GalaxyError::Circuit)?;
        let circuit_registration_tx = accepted.registration_tx;
        let d: Descriptor = serde_json::from_slice(descriptor).map_err(|_| GalaxyError::Circuit)?;
        let policy: Policy =
            serde_json::from_slice(policy_json).map_err(|_| GalaxyError::Circuit)?;
        let policy_id = policy.id().map_err(|_| GalaxyError::Circuit)?;
        let digest = |s: &str| hash_hex(s).map_err(|_| GalaxyError::Circuit);
        let vk_digest = digest(&d.vk_digest)?;
        let ceremony_allowed = match d.ceremony.as_str() {
            "development-single-party" => network.network_id != NetworkId::Mainnet,
            "groth16-phase2-multi-party" => true,
            _ => false,
        };
        if d.version != 1
            || d.profile != policy.profile
            || d.n_public != PUBLIC_COUNT
            || d.public_layout != "galaxy-pc-v1-public-22"
            || !ceremony_allowed
            || digest(&d.policy_id)? != policy_id
            || policy_id != accepted.policy_id
            || vk_digest != accepted.vk_digest
        {
            return Err(GalaxyError::Circuit);
        }
        let digests = ArtifactDigests {
            witness_graph_sha256: digest(&d.witness_graph_sha256)?,
            zkey_sha256: digest(&d.zkey_sha256)?,
        };
        Ok(Self {
            network,
            circuit_id,
            policy_id,
            circuit_registration_tx,
            vk_digest,
            digests,
            ceremony: d.ceremony,
            policy,
        })
    }
}

impl CaptureCircuit {
    fn checked_core(&self, core: &[u8]) -> Result<CaptureCore> {
        let core = checked_core(core)?;
        if core.network != self.network.network_id.as_u32() {
            return Err(GalaxyError::Network);
        }
        if core.circuit_id != self.circuit_id || core.policy_id != self.policy_id {
            return Err(GalaxyError::CaptureBinding);
        }
        Ok(core)
    }

    fn selection_index(&self, core: &CaptureCore, capture_ledger: u32) -> Result<u32> {
        let deadline = add_window(core.preledger_index, self.policy.windows.w_cap)?;
        if capture_ledger <= core.preledger_index || capture_ledger > deadline {
            return Err(GalaxyError::LedgerWindow);
        }
        add_window(capture_ledger, self.policy.windows.sel_offset)
    }
}

#[derive(uniffi::Object)]
pub struct CaptureChallenge {
    circuit: Arc<CaptureCircuit>,
    statement: CaptureCore,
    capture_tx: Hash32,
    capture_id: Hash32,
    selection_index: u32,
    selection_hash: Hash32,
    proof_deadline: u32,
    challenge: Challenge,
    public_inputs: [Fr; PUBLIC_COUNT],
}

#[derive(uniffi::Record)]
pub struct ChallengeInfo {
    pub seed: Vec<u8>,
    pub sampled_tiles: Vec<u8>,
    pub proof_deadline: u32,
}

#[uniffi::export]
impl CaptureChallenge {
    pub fn info(&self) -> ChallengeInfo {
        ChallengeInfo {
            seed: self.challenge.seed.to_vec(),
            sampled_tiles: self.challenge.tiles.to_vec(),
            proof_deadline: self.proof_deadline,
        }
    }

    pub fn public_inputs(&self) -> Vec<Vec<u8>> {
        self.public_inputs
            .iter()
            .map(|scalar| fr_to_be(scalar).to_vec())
            .collect()
    }

    pub fn evaluate(
        self: Arc<Self>,
        evidence: Arc<CaptureEvidence>,
    ) -> Result<Arc<CaptureEvaluation>> {
        guarded(|| {
            evidence.check_core(&self.statement)?;
            let view = build_view(&evidence.bundle.evidence, &self.challenge)
                .map_err(|_| GalaxyError::Evidence)?;
            let verdict =
                try_evaluate(&view, &self.circuit.policy).map_err(|_| GalaxyError::Evidence)?;
            Ok(Arc::new(CaptureEvaluation {
                challenge: self,
                evidence,
                view,
                verdict,
            }))
        })
    }

    /// Export only after the matching PROOF transaction has validated with tesSUCCESS.
    #[allow(clippy::needless_pass_by_value)]
    pub fn export_image(
        &self,
        evidence: Arc<CaptureEvidence>,
        proof_tx: String,
    ) -> Result<Vec<u8>> {
        guarded(|| {
            evidence.check_core(&self.statement)?;
            let xmp = GalaxyXmp {
                protocol: xmp::PROTOCOL.into(),
                network: network_id(self.statement.network)?.name().into(),
                image_profile: xmp::IMAGE_PROFILES[usize::from(evidence.profile().code())].into(),
                photo_id: self.statement.photo_id,
                capture_id: self.capture_id,
                manifest_tx: self.capture_tx,
                proof_tx: hash_hex(&proof_tx)?,
            };
            galaxy_image::embed_xmp(&evidence.bundle.evidence.image.encoded, &xmp)
                .map_err(|_| GalaxyError::Image)
        })
    }
}

#[derive(Clone, Copy, uniffi::Record)]
pub struct CaptureVerdict {
    pub quality_ok: bool,
    pub geometry_ok: bool,
    pub rgb_depth_ok: bool,
    pub temporal_ok: bool,
    pub motion_ok: bool,
    pub pass: bool,
}

#[derive(uniffi::Object)]
pub struct CaptureEvaluation {
    challenge: Arc<CaptureChallenge>,
    evidence: Arc<CaptureEvidence>,
    view: GeometryView,
    verdict: PolicyReport,
}

#[uniffi::export]
impl CaptureEvaluation {
    pub fn verdict(&self) -> CaptureVerdict {
        let v = &self.verdict;
        CaptureVerdict {
            quality_ok: v.quality_ok,
            geometry_ok: v.geometry_ok,
            rgb_depth_ok: v.rgb_depth_ok,
            temporal_ok: v.temporal_ok,
            motion_ok: v.motion_ok,
            pass: v.pass,
        }
    }

    pub fn build_witness(&self) -> Result<Arc<CaptureWitness>> {
        guarded(|| {
            if !self.verdict.pass {
                return Err(GalaxyError::Inconclusive);
            }
            let evidence = &self.evidence;
            let c = &self.challenge;
            let json = galaxy_witness::build_input(
                &evidence.bundle.evidence,
                &evidence.bundle.salts,
                &evidence.commitments,
                &c.challenge,
                &c.capture_id,
                &self.view,
            )
            .map_err(|_| GalaxyError::Witness)?;
            let json = serde_json::to_string(&json).map_err(|_| GalaxyError::Witness)?;
            Ok(Arc::new(CaptureWitness {
                json: Zeroizing::new(json),
                challenge: Arc::clone(c),
            }))
        })
    }
}

/// A private witness input handle, not a string to be transported to Swift or persisted.
#[derive(uniffi::Object)]
pub struct CaptureWitness {
    json: Zeroizing<String>,
    challenge: Arc<CaptureChallenge>,
}

/// Phase notifications are delivered on the proving thread.
#[uniffi::export(with_foreign)]
pub trait CaptureProgress: Send + Sync {
    fn phase_started(&self, phase: String);
    fn phase_finished(&self, phase: String, seconds: f64);
}

struct ProgressAdapter(Arc<dyn CaptureProgress>);
impl Progress for ProgressAdapter {
    fn phase_started(&self, phase: Phase) {
        self.0.phase_started(phase.name().into());
    }
    fn phase_finished(&self, phase: Phase, seconds: f64) {
        self.0.phase_finished(phase.name().into(), seconds);
    }
}

#[derive(Default, uniffi::Object)]
pub struct ProvingCancellation {
    flag: AtomicBool,
}

#[uniffi::export]
impl ProvingCancellation {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }
}

#[derive(uniffi::Record)]
pub struct CaptureProof {
    pub proof: Vec<u8>,
    pub witness_seconds: f64,
    pub prove_seconds: f64,
}

#[derive(uniffi::Object)]
pub struct CaptureProver {
    prover: Prover,
    circuit_id: Hash32,
}

#[uniffi::export]
impl CaptureProver {
    /// Synchronous, expensive computation. Invoke it on a dedicated worker, not the main actor.
    pub fn prove(
        &self,
        witness: Arc<CaptureWitness>,
        progress: Option<Arc<dyn CaptureProgress>>,
        cancellation: Option<Arc<ProvingCancellation>>,
    ) -> Result<CaptureProof> {
        guarded(|| {
            self.check_challenge(&witness.challenge)?;
            let progress = progress.map(ProgressAdapter);
            let ctrl = ProveControl {
                progress: progress.as_ref().map(|p| p as &dyn Progress),
                cancel: cancellation.as_ref().map(|c| &c.flag),
            };
            let (proof, public_inputs, timings) = self
                .prover
                .prove_ark(&witness.json, &ctrl)
                .map_err(|err| match err {
                    ProverError::Cancelled => GalaxyError::Cancelled,
                    ProverError::Witness(_) => GalaxyError::Witness,
                    ProverError::Rng(_) => GalaxyError::Randomness,
                    ProverError::SelfCheck(_) => GalaxyError::ProofVerification,
                    _ => GalaxyError::Proving,
                })?;
            if public_inputs != witness.challenge.public_inputs {
                return Err(GalaxyError::ProofVerification);
            }
            let proof = prover::codec::encode_proof(&proof).map_err(|_| GalaxyError::Proving)?;
            Ok(CaptureProof {
                proof: proof.to_vec(),
                witness_seconds: timings.witness(),
                prove_seconds: timings.prove(),
            })
        })
    }

    /// Checks against the statement reconstructed from the CAPTURE and Lsel, never witness-supplied inputs.
    pub fn verify_proof(&self, challenge: Arc<CaptureChallenge>, proof: Vec<u8>) -> Result<bool> {
        guarded(|| self.verify(&challenge, &proof))
    }

    /// Refuses to build the PROOF memo unless the local check passes.
    pub fn build_proof_memo(
        &self,
        challenge: Arc<CaptureChallenge>,
        proof: Vec<u8>,
    ) -> Result<Memo> {
        guarded(|| {
            if !self.verify(&challenge, &proof)? {
                return Err(GalaxyError::ProofVerification);
            }
            let memo = ProofMemo {
                network: challenge.statement.network,
                manifest_tx: challenge.capture_tx,
                selection_ledger_index: challenge.selection_index,
                selection_ledger_hash: challenge.selection_hash,
                circuit_id: self.circuit_id,
                proof: exact(&proof)?,
            };
            Ok(Memo::new(MemoKind::Proof, memo.encode()))
        })
    }
}

impl CaptureProver {
    fn check_challenge(&self, c: &CaptureChallenge) -> Result<()> {
        if c.circuit.circuit_id != self.circuit_id {
            return Err(GalaxyError::CaptureBinding);
        }
        Ok(())
    }

    fn verify(&self, c: &CaptureChallenge, proof: &[u8]) -> Result<bool> {
        self.check_challenge(c)?;
        let proof =
            prover::codec::decode_proof(proof).map_err(|_| GalaxyError::ProofVerification)?;
        prover::verify_ark(self.prover.verifying_key(), &c.public_inputs, &proof)
            .map_err(|_| GalaxyError::ProofVerification)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
