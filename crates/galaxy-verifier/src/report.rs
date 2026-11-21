use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Verified,
    Pending,
    Invalid,
    Inconclusive,
    NotFound,
    Unavailable,
    Unsupported,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::Pending => "PENDING",
            Self::Invalid => "INVALID",
            Self::Inconclusive => "INCONCLUSIVE",
            Self::NotFound => "NOT_FOUND",
            Self::Unavailable => "UNAVAILABLE",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// `galaxy` CLI exit code for this state.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Verified => 0,
            Self::Pending => 10,
            Self::Invalid => 20,
            Self::Inconclusive => 30,
            Self::NotFound => 40,
            Self::Unavailable => 50,
            Self::Unsupported => 60,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Unavailable,
    NotApplicable,
    Skipped,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unavailable => "unavailable",
            Self::NotApplicable => "not_applicable",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenBy {
    Cryptographic,
    ZkCircuit,
    CommittedOnly,
    LedgerRpc,
    Attestation,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub status: Status,
    pub proven_by: ProvenBy,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LedgerRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsultedRange {
    pub purpose: &'static str,
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RpcTrust {
    #[default]
    Remote,
    Local,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TrustReport {
    pub rpc_endpoints: Vec<String>,
    pub rpc_trust: RpcTrust,
    pub lookup_endpoints: Vec<String>,
    pub ledger_ranges: Vec<ConsultedRange>,
    pub latest_validated: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImageReport {
    pub format: &'static str,
    pub profile: galaxy_image::ImageProfile,
    pub photo_id: String,
    pub rgb_root: String,
    pub xmp: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaptureReport {
    pub manifest_tx: String,
    #[serde(rename = "L0")]
    pub l0: u32,
    #[serde(rename = "Lpre")]
    pub lpre: u32,
    pub capture_id: String,
    pub account: String,
    pub registration_tx: String,
    pub claimed_time_ms: u64,
    pub ledger_close_time: u64,
    pub attestation: Option<&'static str>,
    pub app_id: Option<String>,
    pub counter: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProofReport {
    pub proof_tx: Option<String>,
    #[serde(rename = "Lsel")]
    pub lsel: Option<u32>,
    #[serde(rename = "Lproof")]
    pub lproof: Option<u32>,
    pub deadline: Option<u32>,
    pub selection_hash: Option<String>,
    pub circuit_id: String,
    pub policy_id: String,
    pub policy_name: Option<String>,
    pub ceremony: Option<String>,
    pub seed: Option<String>,
    pub sampled_tiles: Option<[u8; 16]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub manifest_tx: String,
    pub proof_tx: Option<String>,
    pub state: State,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<Candidate>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub state: State,
    pub summary: String,
    pub network: String,
    pub mode: &'static str,
    pub image: Option<ImageReport>,
    pub capture: Option<CaptureReport>,
    pub proof: Option<ProofReport>,
    pub checks: Vec<Check>,
    pub candidates: Vec<Candidate>,
    pub limitations: Vec<&'static str>,
    pub trust: TrustReport,
}

pub(crate) const POLICY_CHECKS: &[&str] = &[
    "policy.sensor_quality",
    "policy.lidar_geometry",
    "policy.rgb_lidar",
    "policy.temporal",
    "policy.motion_imu",
];

/// Every check in report order with the mechanism that establishes it.
pub const CHECKS: &[(&str, ProvenBy)] = {
    use ProvenBy::*;
    &[
        ("image.decode", Cryptographic),
        ("image.profile", Cryptographic),
        ("image.photo_id", Cryptographic),
        ("image.rgb_root", Cryptographic),
        ("refs.metadata", Cryptographic),
        ("ledger.network", LedgerRpc),
        ("capture.found", LedgerRpc),
        ("capture.validated_success", LedgerRpc),
        ("capture.decode", Cryptographic),
        ("capture.network", Cryptographic),
        ("capture.txn_chain", Cryptographic),
        ("capture.photo_binding", Cryptographic),
        ("capture.rgb_binding", Cryptographic),
        ("capture.account_binding", Cryptographic),
        ("circuit.accepted", Cryptographic),
        ("circuit.registration", Cryptographic),
        ("registration.resolve", LedgerRpc),
        ("registration.txn_chain", Cryptographic),
        ("registration.bundle", Cryptographic),
        ("registration.attestation", Attestation),
        ("capture.assertion", Attestation),
        ("capture.counter", Attestation),
        ("history.complete", LedgerRpc),
        ("capture.unique", LedgerRpc),
        ("registration.revocation", LedgerRpc),
        ("ledger.preledger_window", LedgerRpc),
        ("ledger.selection", LedgerRpc),
        ("ledger.challenge", Cryptographic),
        ("proof.found", LedgerRpc),
        ("proof.validated_success", LedgerRpc),
        ("proof.account", Cryptographic),
        ("proof.binding", Cryptographic),
        ("proof.window", LedgerRpc),
        ("proof.groth16", Cryptographic),
        ("policy.sensor_quality", ZkCircuit),
        ("policy.lidar_geometry", ZkCircuit),
        ("policy.rgb_lidar", ZkCircuit),
        ("policy.temporal", ZkCircuit),
        ("policy.motion_imu", ZkCircuit),
        ("evidence.bluetooth", CommittedOnly),
        ("evidence.noise", CommittedOnly),
    ]
};

impl Check {
    pub(crate) fn new(id: &'static str, status: Status, detail: impl Into<String>) -> Self {
        let proven_by = CHECKS.iter().find(|(name, _)| *name == id).map_or(ProvenBy::Cryptographic, |(_, by)| *by);
        Self { id, status, proven_by, detail: detail.into() }
    }
}

impl Report {
    pub(crate) fn new(network: String, mode: &'static str, trust: TrustReport) -> Self {
        let checks = CHECKS.iter().map(|&(id, _)| Check::new(id, Status::Skipped, "Not evaluated")).collect();
        let mut report = Self {
            version: "galaxy-report/v1", state: State::Unavailable, summary: String::new(), network,
            mode, image: None, capture: None, proof: None, checks, candidates: Vec::new(), trust,
            limitations: vec![
                "Physical consistency under a policy is not proof that a scene or its depiction is true.",
                "Coherent synthetic sensor data may satisfy the circuit if the acquisition pipeline is compromised.",
                "The proof tests sampled commitments and analysis pixels, not every full-resolution pixel.",
                "Ledger validation, order and completeness are RPC claims; use your own synchronized rippled to avoid trusting a remote provider.",
                "Groth16 soundness depends on the trusted setup; a development single-party ceremony is not a production setup.",
                "Apple App Attest concerns the app credential; it does not attest individual sensor measurements or sensor provenance.",
                "Discovery nodes can omit records or correlate photo-id queries; they cannot grant verification.",
            ],
        };
        report.set("evidence.bluetooth", Status::NotApplicable, "Committed only; nearby devices are not authenticated");
        report.set("evidence.noise", Status::NotApplicable, "Committed only; no sensor fingerprint or noise authenticity is verified");
        report
    }

    /// A report for a verification that could not run to completion (for
    /// example a caller-imposed time limit). Never claims anything about a photo.
    pub fn unavailable(network: String, mode: &'static str, trust: TrustReport, summary: impl Into<String>) -> Self {
        let mut report = Self::new(network, mode, trust);
        report.summary = summary.into();
        report
    }

    pub fn check(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.id == id)
    }

    pub fn exit_code(&self) -> u8 {
        self.state.exit_code()
    }

    /// A different endpoint may have the missing data. Restart the entire
    /// operation rather than retaining any reports from this attempt.
    pub fn needs_rpc_retry(&self) -> bool {
        fn unavailable(candidate: &Candidate) -> bool {
            candidate.state == State::Unavailable || candidate.proofs.iter().any(unavailable)
        }
        self.state == State::Unavailable || self.candidates.iter().any(unavailable)
    }

    pub(crate) fn set_check(&mut self, check: Check) {
        if let Some(slot) = self.checks.iter_mut().find(|slot| slot.id == check.id) {
            *slot = check;
        }
    }

    pub(crate) fn set(&mut self, id: &'static str, status: Status, detail: impl Into<String>) {
        if let Some(check) = self.checks.iter_mut().find(|check| check.id == id) {
            check.status = status;
            check.detail = detail.into();
        }
    }

    pub(crate) fn pass(&mut self, id: &'static str, detail: impl Into<String>) {
        self.set(id, Status::Pass, detail);
    }

    pub(crate) fn note_range(&mut self, purpose: &'static str, min: u32, max: u32) {
        self.trust.ledger_ranges.push(ConsultedRange { purpose, min, max });
    }

    pub fn human(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}: {}", self.state.as_str(), self.summary)?;
        writeln!(f, "Network: {} ({})", self.network, self.mode)?;
        if let Some(image) = &self.image {
            writeln!(f, "Photo ID: {}\nRGB root: {}", image.photo_id, image.rgb_root)?;
        }
        if let Some(capture) = &self.capture {
            writeln!(f, "CAPTURE: {} (ledger {})", capture.manifest_tx, capture.l0)?;
            if let Some(attestation) = capture.attestation {
                writeln!(f, "Attestation: {attestation}")?;
            }
        }
        if let Some(proof) = &self.proof {
            if let Some(tx) = &proof.proof_tx { writeln!(f, "PROOF: {tx}")?; }
            writeln!(f, "Circuit: {}\nPolicy ID: {}", proof.circuit_id, proof.policy_id)?;
            if let Some(ceremony) = &proof.ceremony { writeln!(f, "Ceremony: {ceremony}")?; }
        }
        for check in &self.checks {
            if check.status != Status::Skipped {
                writeln!(f, "  {} [{}]: {}", check.id, check.status.as_str(), check.detail)?;
            }
        }
        fn candidates(f: &mut fmt::Formatter<'_>, list: &[Candidate], indent: &str) -> fmt::Result {
            for candidate in list {
                writeln!(f, "{indent}{} {}: {} — {}", candidate.state.as_str(), candidate.manifest_tx,
                    candidate.proof_tx.as_deref().unwrap_or("–"), candidate.summary)?;
                candidates(f, &candidate.proofs, "    ")?;
            }
            Ok(())
        }
        if !self.candidates.is_empty() {
            writeln!(f, "Candidates:")?;
            candidates(f, &self.candidates, "  ")?;
        }
        writeln!(f, "RPC trust: {:?}; endpoints: {}", self.trust.rpc_trust, self.trust.rpc_endpoints.join(", "))?;
        for range in &self.trust.ledger_ranges {
            writeln!(f, "  {}: {}..{}", range.purpose, range.min, range.max)?;
        }
        for limitation in &self.limitations { writeln!(f, "- {limitation}")?; }
        Ok(())
    }
}
