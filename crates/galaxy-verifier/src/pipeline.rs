use crate::{
    circuit::{self, Circuit},
    Candidate, CaptureReport, Check, ImageReport, LookupResult, LookupSource, ProofReport, Refs,
    Report, State, Status, VerifyContext, MAX_LOOKUP_CANDIDATES, MAX_LOOKUP_NODES, POLICY_CHECKS,
};
use galaxy_attestation::{
    verify_capture_assertion, verify_counter, verify_registration, AcceptedAttestation, AttestError,
    AttestationClass, CounterRecord, VerifiedCredential,
};
use galaxy_crypto::{field, groth16, public::public_inputs_v1, sampling, Fr};
use galaxy_image::{decode_with_xmp_status, photo_id, rgb_root_from_raster, Container, ImageError, ImageProfile};
use galaxy_protocol::{
    ids,
    wire::{CaptureCore, CaptureMemo, DeviceBundle, MemoKind, ProofMemo, RevokeMemo},
    AccountId, Hash32,
};
use galaxy_xrpl::{
    codec,
    keys::{classic_address, decode_address, tx_hash},
    ledger::{LedgerHeader, LedgerTx, ServerInfo},
    registry::{covers, fetch_bundle_from_tx, Manifest},
    transaction::{account_txn_id, decode_galaxy_memo, decode_galaxy_object, GalaxyMemo},
    Error as XrplError, LedgerSource,
};
use std::collections::{BTreeMap, HashSet};

/// XRPL account_tx request page size; the page count is configurable per scan.
const PAGE_LIMIT: u16 = 200;
const MAX_LOOKUP_RANGES: usize = 64;
/// PROOF candidates evaluated per capture (each costs one pairing check).
const MAX_PROOFS: usize = 32;
const DEV_LABEL: &str = "development unattested credential";

pub(crate) struct Stop {
    pub state: State,
    check: &'static str,
    detail: String,
}

impl Stop {
    fn new(state: State, check: &'static str, detail: impl Into<String>) -> Self {
        Self { state, check, detail: detail.into() }
    }

    pub fn invalid(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(State::Invalid, check, detail)
    }

    pub fn unsupported(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(State::Unsupported, check, detail)
    }

    fn unavailable(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(State::Unavailable, check, detail)
    }

    fn pending(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(State::Pending, check, detail)
    }

    /// `invalid_data`: the bytes come from hash-bound ledger records, so a
    /// decoding failure is a property of the record rather than of the RPC.
    pub fn from_rpc(check: &'static str, err: &XrplError, invalid_data: bool) -> Self {
        match err {
            XrplError::Network if !invalid_data => Self::unavailable(check, "RPC endpoint does not serve the configured network"),
            XrplError::Network => Self::invalid(check, "Record does not match the configured network"),
            XrplError::Invalid(_) | XrplError::Codec(_) | XrplError::Wire(_) if invalid_data => {
                Self::invalid(check, err.to_string())
            }
            XrplError::HistoryUnavailable(detail) => Self::unavailable(check, format!("XRPL history unavailable: {detail}")),
            // Static parser messages only; server error bodies and URLs are not copied into reports.
            XrplError::Invalid(detail) => Self::unavailable(check, format!("RPC response rejected: {detail}")),
            _ => Self::unavailable(check, "XRPL data unavailable or RPC response invalid"),
        }
    }

    pub fn apply(self, report: &mut Report) {
        let status = match self.state {
            State::Invalid | State::Unsupported | State::Inconclusive => Status::Fail,
            State::NotFound => Status::NotApplicable,
            _ => Status::Unavailable,
        };
        report.set(self.check, status, &self.detail);
        report.state = self.state;
        report.summary = self.detail;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhotoIdentity {
    pub photo_id: Hash32,
    pub rgb_root: Hash32,
    pub profile: ImageProfile,
}

/// Local image identity (no network): canonical raster photo_id and RGB root.
pub fn photo_identity(image_bytes: &[u8]) -> Result<PhotoIdentity, ImageError> {
    let (decoded, _) = decode_with_xmp_status(image_bytes)?;
    Ok(PhotoIdentity {
        photo_id: photo_id(&decoded.raster)?,
        rgb_root: field::fr_to_be(&rgb_root_from_raster(&decoded.raster)?),
        profile: decoded.profile_hint,
    })
}

fn prepare_image(bytes: &[u8], report: &mut Report) -> Result<(PhotoIdentity, Option<Refs>), Stop> {
    let image_err = |err: ImageError| match err {
        ImageError::Unsupported(_) => Stop::unsupported("image.profile", err.to_string()),
        _ => Stop::invalid("image.decode", err.to_string()),
    };
    if bytes.len() > galaxy_image::MAX_IMAGE_BYTES {
        return Err(Stop::unsupported("image.profile", "Image file exceeds the size limit"));
    }
    let (decoded, xmp) = decode_with_xmp_status(bytes).map_err(image_err)?;
    report.pass("image.decode", "Decoded canonical 1024×768 RGB8 raster");
    report.pass("image.profile", "Supported image container and pixel profile");
    let photo_id = photo_id(&decoded.raster).map_err(image_err)?;
    let rgb_root = field::fr_to_be(&rgb_root_from_raster(&decoded.raster).map_err(image_err)?);
    let identity = PhotoIdentity { photo_id, rgb_root, profile: decoded.profile_hint };
    let (metadata, refs) = match xmp {
        Ok(Some(xmp)) => {
            let model = xmp.model;
            report.pass("refs.metadata", "Found Galaxy XMP; its references are untrusted hints");
            (Some(serde_json::json!({
                "protocol": model.protocol, "network": model.network, "image_profile": model.image_profile,
                "photo_id": hex::encode(model.photo_id), "capture_id": hex::encode(model.capture_id),
                "manifest_tx": hex::encode_upper(model.manifest_tx), "proof_tx": hex::encode_upper(model.proof_tx),
            })), Some(Refs { manifest_tx: model.manifest_tx, proof_tx: Some(model.proof_tx) }))
        }
        Ok(None) => {
            report.set("refs.metadata", Status::NotApplicable, "No Galaxy XMP; supply a CAPTURE hash or use photo-id lookup");
            (None, None)
        }
        Err(err) => {
            report.set("refs.metadata", Status::NotApplicable, format!("Unusable XMP ignored ({err}); supply a CAPTURE hash or use photo-id lookup"));
            (None, None)
        }
    };
    report.image = Some(ImageReport {
        format: match decoded.container { Container::Png => "png", Container::Jpeg => "jpeg" },
        profile: identity.profile, photo_id: hex::encode(photo_id), rgb_root: hex::encode(rgb_root), xmp: metadata,
    });
    report.pass("image.photo_id", "Recomputed SHA-256 photo ID from the presented pixels");
    report.pass("image.rgb_root", "Recomputed public Poseidon root from the analysis raster");
    Ok((identity, refs))
}

fn mark_no_image(report: &mut Report) {
    for check in ["image.decode", "image.profile", "image.photo_id", "image.rgb_root", "refs.metadata", "capture.photo_binding", "capture.rgb_binding"] {
        report.set(check, Status::NotApplicable, "Image binding not checked (proof-only mode)");
    }
    report.limitations.push("Image binding not checked: this report does not certify any presented image.");
}

/// Direct mode (references from the image XMP) or manual mode (caller references).
pub async fn verify_photo<L: LedgerSource>(
    image_bytes: &[u8], refs: Option<Refs>, ctx: &VerifyContext<'_, L>,
) -> Report {
    let mut report = ctx.report(if refs.is_some() { "manual" } else { "direct" });
    let (identity, embedded) = match prepare_image(image_bytes, &mut report) {
        Ok(image) => image,
        Err(stop) => { stop.apply(&mut report); return report; }
    };
    let Some(refs) = refs.or(embedded) else {
        report.state = State::NotFound;
        report.summary = "No Galaxy references in this image and none supplied; no capture was consulted. Supply a CAPTURE hash or use photo-id lookup.".into();
        return report;
    };
    evaluate(ctx, report, Some(identity), refs).await
}

/// Manual mode by CAPTURE hash; without an image this is proof-only mode.
pub async fn verify_manifest<L: LedgerSource>(
    image_bytes: Option<&[u8]>, manifest_tx: Hash32, ctx: &VerifyContext<'_, L>,
) -> Report {
    let refs = Refs { manifest_tx, proof_tx: None };
    if let Some(bytes) = image_bytes {
        verify_photo(bytes, Some(refs), ctx).await
    } else {
        let mut report = ctx.report("proof-only");
        mark_no_image(&mut report);
        evaluate(ctx, report, None, refs).await
    }
}

/// Capture and proof verification without an image (nodes, `galaxy inspect`).
pub async fn verify_proof_only<L: LedgerSource>(manifest_tx: Hash32, ctx: &VerifyContext<'_, L>) -> Report {
    verify_manifest(None, manifest_tx, ctx).await
}

/// Inspect one exact PROOF transaction without a photo. A different valid
/// sibling never makes this transaction pass.
pub async fn verify_proof_transaction<L: LedgerSource>(proof_tx: Hash32, ctx: &VerifyContext<'_, L>) -> Report {
    let mut report = ctx.report("proof-only");
    mark_no_image(&mut report);
    let mut pipeline = Pipeline::new(ctx, report);
    let result = pipeline.exact_proof(proof_tx).await;
    pipeline.finish(result)
}

/// Lookup mode: untrusted nodes supply CAPTURE hints for the photo ID.
/// The earliest authenticated, image-bound hint in XRPL order decides.
pub async fn verify_by_photo_id<L: LedgerSource, N: LookupSource>(
    image_bytes: &[u8], nodes: &[N], ctx: &VerifyContext<'_, L>,
) -> Report {
    let mut report = ctx.report("lookup");
    let identity = match prepare_image(image_bytes, &mut report) {
        Ok((identity, _)) => identity,
        Err(stop) => { stop.apply(&mut report); return report; }
    };
    report.limitations.push("Lookup completeness is limited to the consulted nodes; an earlier CAPTURE may be missing.");
    let mut hints = BTreeMap::new();
    let mut incomplete = nodes.is_empty() || nodes.len() > MAX_LOOKUP_NODES;
    for node in nodes.iter().take(MAX_LOOKUP_NODES) {
        report.trust.lookup_endpoints.push(node.endpoint().into());
        match node.lookup(&identity.photo_id, ctx.config.network_id.as_u32()).await {
            Err(_) => incomplete = true,
            Ok(LookupResult { candidates, ranges, incomplete: partial }) => {
                incomplete |= partial || ranges.is_empty();
                for range in ranges.iter().take(MAX_LOOKUP_RANGES) {
                    if range.min == 0 || range.max < range.min { incomplete = true; continue; }
                    report.note_range("lookup index (untrusted)", range.min, range.max);
                }
                incomplete |= ranges.len() > MAX_LOOKUP_RANGES || candidates.len() > MAX_LOOKUP_CANDIDATES;
                for refs in candidates.into_iter().take(MAX_LOOKUP_CANDIDATES) {
                    if hints.len() >= MAX_LOOKUP_CANDIDATES && !hints.contains_key(&refs.manifest_tx) {
                        incomplete = true;
                        continue;
                    }
                    // Only CAPTURE hints matter; PROOFs come from XRPL account history.
                    hints.entry(refs.manifest_tx).or_insert(refs.proof_tx);
                }
            }
        }
    }
    // Establish order from XRPL, not from the lookup node or from hash order.
    let mut lookup = Pipeline::new(ctx, report);
    let info = if hints.is_empty() {
        None
    } else {
        match lookup.server_info().await {
            Ok(info) => Some(info),
            Err(stop) => return lookup.finish(Err(stop)),
        }
    };
    let mut ordered = Vec::with_capacity(hints.len());
    for (hash, advertised_proof) in hints {
        let tx = lookup.tx(&hash, "capture.found").await;
        let key = match &tx {
            Ok(Some(tx)) => lookup_capture_key(tx, identity, lookup.network(), info.as_ref().expect("lookup tip").validated_ledger),
            _ => None,
        };
        ordered.push((key, hash, advertised_proof, tx));
    }
    ordered.sort_unstable_by_key(|(key, hash, _, _)| (key.is_none(), *key, *hash));
    let mut report = lookup.report;
    let mut other_captures = 0;
    let mut candidates = Vec::new();
    let mut earliest: Option<Report> = None;
    let mut ranges = std::mem::take(&mut report.trust.ledger_ranges);
    for (key, manifest_tx, advertised_proof, tx) in ordered {
        if key.is_some() && earliest.is_some() {
            other_captures += 1;
            candidates.push(Candidate {
                manifest_tx: hex::encode_upper(manifest_tx),
                proof_tx: advertised_proof.map(hex::encode_upper),
                state: State::Invalid,
                summary: "Not evaluated: an earlier CAPTURE in XRPL order binds to this photo".into(),
                checks: vec![Check::new("capture.unique", Status::Fail, "Later CAPTURE matches the same photo as the earliest hint")],
                proofs: Vec::new(),
            });
            continue;
        }
        let refs = Refs { manifest_tx, proof_tx: None };
        let mut pipeline = Pipeline::new(ctx, report.clone());
        let result = match tx {
            Ok(tx) => pipeline.capture(refs, Some(identity), info.as_ref().expect("lookup tip"), None, tx).await,
            Err(stop) => Err(stop),
        };
        let mut candidate = pipeline.finish(result);
        ranges.append(&mut candidate.trust.ledger_ranges);
        // Non-binding hints are discarded; an untrusted node must not make an unrelated photo look invalid.
        let state = if key.is_some() || candidate.state == State::Unavailable {
            candidate.state
        } else {
            State::NotFound
        };
        incomplete |= state == State::Unavailable;
        candidates.push(Candidate {
            manifest_tx: hex::encode_upper(manifest_tx),
            proof_tx: candidate.proof.as_ref().and_then(|proof| proof.proof_tx.clone())
                .or_else(|| advertised_proof.map(hex::encode_upper)),
            state,
            summary: if state == State::NotFound {
                format!("Rejected hint: no usable CAPTURE bound to this image ({})", candidate.summary)
            } else {
                candidate.summary.clone()
            },
            checks: candidate.checks.clone(), proofs: std::mem::take(&mut candidate.candidates),
        });
        if key.is_some() {
            earliest = Some(candidate);
        }
    }
    if let Some(candidate) = earliest.filter(|candidate| !incomplete ||
        matches!(candidate.state, State::Verified | State::Pending | State::Unsupported | State::Inconclusive))
    {
        report = candidate;
    } else if incomplete {
        report.state = State::Unavailable;
        report.summary = "Lookup or required ledger history is incomplete; absence of a capture is not established.".into();
    } else {
        report.state = State::NotFound;
        report.summary = "No capture bound to this image was found in the consulted index ranges and nodes.".into();
    }
    if other_captures > 0 {
        report.summary.push_str(&format!(" {other_captures} other CAPTUREs of this photo."));
    }
    report.trust.ledger_ranges = ranges;
    report.candidates = candidates;
    report
}

fn lookup_capture_key(tx: &LedgerTx, image: PhotoIdentity, network: u32, tip: u32) -> Option<(u32, u32)> {
    if !tx.validated || tx.ledger_index > tip || tx.transaction_result != "tesSUCCESS" { return None; }
    let envelope = decode_kind(tx, MemoKind::Capture, "capture.decode").ok()?;
    let core = CaptureMemo::decode(&envelope.data).ok()?.core;
    (core.network == network && network_field(&envelope, network)
        && core.account_id == envelope.account && image.photo_id == core.photo_id && image.rgb_root == core.rgb_root)
        .then_some((tx.ledger_index, tx.transaction_index))
}

async fn evaluate<L: LedgerSource>(
    ctx: &VerifyContext<'_, L>, report: Report, image: Option<PhotoIdentity>, refs: Refs,
) -> Report {
    let mut pipeline = Pipeline::new(ctx, report);
    let result = async {
        let info = pipeline.server_info().await?;
        let tx = pipeline.tx(&refs.manifest_tx, "capture.found").await?;
        pipeline.capture(refs, image, &info, None, tx).await
    }.await;
    pipeline.finish(result)
}

/// The values every PROOF for one capture must bind to.
struct Statement {
    account: AccountId,
    manifest_tx: Hash32,
    circuit_id: Hash32,
    lsel: u32,
    lsel_hash: Hash32,
    deadline: u32,
    public: [Fr; 22],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Verdict {
    /// Accepted proof.
    Valid,
    /// Not a publication for this capture's window (other account or capture,
    /// failed or unvalidated transaction, outside the window): reported only.
    Rejected,
    /// A validated in-window PROOF from the capture account for this CAPTURE that fails verification.
    Invalid,
}

struct ProofOutcome {
    verdict: Verdict,
    checks: Vec<Check>,
    summary: String,
    unvalidated: bool,
}

impl ProofOutcome {
    fn pass(&mut self, id: &'static str, detail: impl Into<String>) {
        self.checks.push(Check::new(id, Status::Pass, detail));
    }

    fn end(mut self, verdict: Verdict, id: &'static str, status: Status, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.summary = detail.clone();
        self.checks.push(Check::new(id, status, detail));
        self.verdict = verdict;
        self
    }
}

#[derive(Clone, Copy)]
enum HistoryAnchor {
    Capture { predecessor: Hash32, before: (u32, u32), registration: Hash32 },
    Tip { capture: Hash32, at: (u32, u32), lsel: u32, proof_max: u32 },
}

impl HistoryAnchor {
    fn accepts(self, kind: MemoKind, at: (u32, u32)) -> bool {
        match self {
            Self::Capture { before, .. } => kind == MemoKind::Revoke || (kind == MemoKind::Capture && at < before),
            Self::Tip { at: capture, lsel, proof_max, .. } => {
                (kind == MemoKind::Revoke && at > capture)
                    || (kind == MemoKind::Proof && at.0 > lsel && at.0 <= proof_max)
            }
        }
    }
}

#[derive(Default)]
struct HistoryScan {
    records: Vec<(LedgerTx, GalaxyMemo)>,
    pages: usize,
    rows: usize,
    ignored: usize,
    walked: u32,
    rpc_links: usize,
}

impl HistoryScan {
    fn detail(&self, name: &str) -> String {
        format!("{name}: {} account_tx pages, {} rows, {} non-records ignored, {} AccountTxnID steps ({} metadata links)",
            self.pages, self.rows, self.ignored, self.walked, self.rpc_links)
    }
}

struct Pipeline<'ctx, 'data, L: LedgerSource> {
    ctx: &'ctx VerifyContext<'data, L>,
    report: Report,
    dev_credential: bool,
}

impl<'ctx, 'data, L: LedgerSource> Pipeline<'ctx, 'data, L> {
    fn new(ctx: &'ctx VerifyContext<'data, L>, report: Report) -> Self {
        Self { ctx, report, dev_credential: false }
    }

    fn network(&self) -> u32 { self.ctx.config.network_id.as_u32() }

    /// Apply the final state. A dev-unattested credential can never reach VERIFIED
    /// (nor a PENDING/INCONCLUSIVE that suggests it could): it is UNSUPPORTED unless INVALID.
    fn finish(mut self, result: Result<(), Stop>) -> Report {
        if let Err(stop) = result { stop.apply(&mut self.report); }
        if self.dev_credential && matches!(self.report.state, State::Verified | State::Pending | State::Inconclusive) {
            let previous = self.report.state;
            self.report.state = State::Unsupported;
            self.report.summary = format!(
                "UNSUPPORTED: {DEV_LABEL}. Proof status {}: {} A development credential proves key possession only and can never yield VERIFIED.",
                previous.as_str(), self.report.summary,
            );
        }
        self.report
    }

    async fn server_info(&mut self) -> Result<ServerInfo, Stop> {
        let check = "ledger.network";
        if self.network() == 0 && !self.ctx.config.allow_mainnet {
            return Err(Stop::unsupported(check, "Mainnet is disabled in this network config"));
        }
        let info = self.ctx.ledger.server_info().await.map_err(|err| match err {
            // The endpoint serves another network: a property of the RPC, not of any record.
            XrplError::Network => Stop::unavailable(check, "RPC endpoint does not serve the configured network"),
            err => Stop::from_rpc(check, &err, false),
        })?;
        if info.network_id != self.network() {
            return Err(Stop::unavailable(check, format!(
                "RPC endpoint serves NetworkID {}, not the configured {} (NetworkID {})",
                info.network_id, self.ctx.config.name, self.network(),
            )));
        }
        if info.validated_ledger == 0 {
            return Err(Stop::unavailable(check, "RPC did not provide a latest validated ledger"));
        }
        self.report.trust.latest_validated = Some(info.validated_ledger);
        self.report.pass(check, format!(
            "RPC serves {} (NetworkID {}); latest validated ledger {}", self.ctx.config.name, self.network(), info.validated_ledger,
        ));
        Ok(info)
    }

    async fn header(&mut self, index: u32, check: &'static str) -> Result<LedgerHeader, Stop> {
        self.report.note_range(check, index, index);
        let header = self.ctx.ledger.ledger_header(index).await.map_err(|err| Stop::from_rpc(check, &err, false))?;
        if header.ledger_index != index || !header.validated {
            return Err(Stop::unavailable(check, format!("Validated header of ledger {index} unavailable")));
        }
        Ok(header)
    }

    /// Fetch by hash. The returned blob must hash to the request; anything else
    /// is an unusable RPC answer (UNAVAILABLE), never evidence about the capture.
    async fn tx(&self, hash: &Hash32, check: &'static str) -> Result<Option<LedgerTx>, Stop> {
        let tx = self.ctx.ledger.tx(hash).await.map_err(|err| Stop::from_rpc(check, &err, false))?;
        if tx.as_ref().is_some_and(|tx| tx.hash != *hash || tx_hash(&tx.tx_blob) != *hash) {
            return Err(Stop::unavailable(check, "RPC returned a transaction whose blob does not hash to the requested id"));
        }
        Ok(tx)
    }

    async fn exact_proof(&mut self, hash: Hash32) -> Result<(), Stop> {
        let info = self.server_info().await?;
        let inception = self.ctx.config.inception_ledger;
        let Some(tx) = self.tx(&hash, "proof.found").await? else {
            self.report.note_range("PROOF search coverage", inception, info.validated_ledger);
            return Err(missing_ref(inception, &info, "proof.found", "PROOF"));
        };
        let envelope = decode_kind(&tx, MemoKind::Proof, "proof.binding")?;
        let proof = ProofMemo::decode(&envelope.data).map_err(|err| Stop::invalid("proof.binding", err.to_string()))?;
        let capture = self.tx(&proof.manifest_tx, "capture.found").await?;
        self.capture(Refs { manifest_tx: proof.manifest_tx, proof_tx: Some(hash) }, None, &info, Some(tx), capture).await
    }

    async fn capture(&mut self, refs: Refs, image: Option<PhotoIdentity>, info: &ServerInfo, exact: Option<LedgerTx>, tx: Option<LedgerTx>) -> Result<(), Stop> {
        let inception = self.ctx.config.inception_ledger;
        let tx = match tx {
            Some(tx) => tx,
            None => {
                self.report.note_range("CAPTURE search coverage", inception, info.validated_ledger);
                return Err(missing_ref(inception, info, "capture.found", "CAPTURE"));
            }
        };
        self.report.pass("capture.found", "Retrieved CAPTURE; its blob hashes to the requested id");
        let envelope = decode_kind(&tx, MemoKind::Capture, "capture.decode")?;
        let capture = CaptureMemo::decode(&envelope.data).map_err(|err| Stop::invalid("capture.decode", err.to_string()))?;
        let core = &capture.core;
        if (core.width, core.height) != (galaxy_image::WIDTH as u16, galaxy_image::HEIGHT as u16) {
            return Err(Stop::unsupported("capture.decode", "CAPTURE raster size is not 1024×768"));
        }
        self.report.pass("capture.decode", "Strict Galaxy v1 CAPTURE decoding succeeded");
        if let Some(image) = image {
            let photo_matches = core.photo_id == image.photo_id;
            let rgb_matches = core.rgb_root == image.rgb_root;
            self.report.set("capture.photo_binding", if photo_matches { Status::Pass } else { Status::Fail },
                if photo_matches { "CAPTURE photo ID equals the presented raster's ID" } else { "CAPTURE photo ID differs from the presented raster" });
            self.report.set("capture.rgb_binding", if rgb_matches { Status::Pass } else { Status::Fail },
                if rgb_matches { "CAPTURE RGB root equals the presented raster's root" } else { "CAPTURE RGB root differs from the presented raster" });
            if !photo_matches || !rgb_matches {
                return Err(Stop::invalid(if photo_matches { "capture.rgb_binding" } else { "capture.photo_binding" },
                    "The referenced CAPTURE does not bind to this image (pixels differ from the registered photo)"));
            }
        }
        check_success(&tx, info, "capture.validated_success", "CAPTURE")?;
        self.report.pass("capture.validated_success", format!("CAPTURE validated with tesSUCCESS in ledger {}", tx.ledger_index));
        if core.network != self.network() || !network_field(&envelope, self.network()) || tx.ledger_index < inception {
            return Err(Stop::invalid("capture.network", "CAPTURE network, NetworkID field or inception ledger does not match the config"));
        }
        self.report.pass("capture.network", "CAPTURE memo and AccountSet envelope match the configured network");
        if core.account_id != envelope.account {
            return Err(Stop::invalid("capture.account_binding", "CAPTURE AccountSet account differs from the account in the signed core"));
        }
        let predecessor = envelope.account_txn_id.filter(|hash| *hash != [0; 32])
            .ok_or_else(|| Stop::invalid("capture.txn_chain", "CAPTURE has no nonzero AccountTxnID predecessor"))?;
        self.report.pass("capture.txn_chain", "CAPTURE binds its AccountTxnID predecessor in the master-key signature");
        let l0 = tx.ledger_index;
        let lpre = core.preledger_index;
        let core299 = core.encode();
        let capture_id = ids::capture_id(&core299);
        let header = self.header(l0, "capture.validated_success").await?;
        self.report.capture = Some(CaptureReport {
            manifest_tx: hex::encode_upper(refs.manifest_tx), l0, lpre, capture_id: hex::encode(capture_id),
            account: classic_address(&envelope.account), registration_tx: hex::encode_upper(core.registration_tx),
            claimed_time_ms: core.claimed_time_ms, ledger_close_time: header.close_time,
            attestation: None, app_id: None, counter: None,
        });
        self.report.proof = Some(ProofReport {
            proof_tx: None, lsel: None, lproof: None, deadline: None, selection_hash: None,
            circuit_id: hex::encode(core.circuit_id), policy_id: hex::encode(core.policy_id),
            policy_name: None, ceremony: None, seed: None, sampled_tiles: None,
        });
        circuit::accepted(self.ctx, &core.circuit_id, Some(&core.policy_id))?;
        self.report.pass("circuit.accepted", "Circuit, policy and VK digest are explicitly pinned in the network config");
        let circuit = circuit::resolve(self.ctx, &core.circuit_id, Some(&core.policy_id), Some(info)).await?;
        if let Some((min, max)) = circuit.chunk_range { self.report.note_range("CIRCUIT chunks", min, max); }
        self.report.pass("circuit.registration", format!(
            "Circuit descriptor, policy and verifying-key digests match ({}; {} non-records ignored)",
            circuit.info.source, circuit.ignored_chunks,
        ));
        if let Some(proof) = &mut self.report.proof {
            proof.policy_name = Some(circuit.info.policy.name.clone());
            proof.ceremony = circuit.info.descriptor["ceremony"].as_str().map(str::to_owned);
        }

        let (credential, manifest_ledger) = self.registration(&capture, &tx, info).await?;
        let assertion = verify_capture_assertion(&credential, &core299, &capture.assertion).map_err(|err| match err {
            AttestError::CaptureBinding => Stop::invalid("capture.account_binding", err.to_string()),
            _ => Stop::invalid("capture.assertion", format!("{err} ({})", err.code())),
        })?;
        self.report.pass("capture.account_binding", "CAPTURE transaction, signed core and DEVICE registration use the same account");
        self.report.pass("capture.assertion", if self.dev_credential {
            "Ed25519 development signature over the CAPTURE core is valid (key possession only)"
        } else {
            "App Attest assertion over the CAPTURE core is valid"
        });
        let before = (l0, tx.transaction_index);
        let anchor = HistoryAnchor::Capture { predecessor, before, registration: core.registration_tx };
        let earlier = self.history("A: CAPTURE/REVOKE", &envelope.account, manifest_ledger, l0, anchor, info).await?;
        let mut counters = Vec::new();
        for (entry, memo) in &earlier.records {
            if memo.kind != MemoKind::Capture.as_str() { continue; }
            let Ok(other) = CaptureMemo::decode(&memo.data) else { continue };
            if same_capture(core, &other.core) {
                return Err(Stop::invalid("capture.unique", format!(
                    "An earlier CAPTURE of this account in ledger {} shares the photo ID, RGB root or evidence root", entry.ledger_index,
                )));
            }
            if other.core.registration_tx != core.registration_tx { continue; }
            let other_core = other.core.encode();
            if ids::capture_id(&other_core) == capture_id {
                return Err(Stop::invalid("capture.unique", format!(
                    "An earlier CAPTURE of this registration in ledger {} shares the capture ID", entry.ledger_index,
                )));
            }
            let here = (entry.ledger_index, entry.transaction_index);
            if let Ok(assertion) = verify_capture_assertion(&credential, &other_core, &other.assertion) {
                if let Some(counter) = assertion.counter {
                    counters.push(CounterRecord { ledger_index: here.0, tx_index: here.1, counter });
                }
            }
        }
        self.report.pass("capture.unique", "No earlier CAPTURE in window A from this account shares the capture ID, photo ID, RGB root or evidence root");
        match assertion.counter {
            Some(counter) => {
                verify_counter(CounterRecord { ledger_index: l0, tx_index: tx.transaction_index, counter }, &counters)
                    .map_err(|err| Stop::invalid("capture.counter", format!("{err} ({})", err.code())))?;
                if let Some(report) = &mut self.report.capture { report.counter = Some(counter); }
                self.report.pass("capture.counter", format!(
                    "Counter {counter} exceeds all {} earlier authenticated counters of this registration", counters.len(),
                ));
            }
            None => self.report.set("capture.counter", Status::NotApplicable,
                "Development credential has no authenticated counter; only the re-challenge check applies"),
        }
        let windows = circuit.info.policy.windows;
        let lsel = l0.checked_add(windows.sel_offset).ok_or_else(|| Stop::invalid("ledger.selection", "Selection ledger overflows"))?;
        let deadline = lsel.checked_add(windows.w_proof).ok_or_else(|| Stop::invalid("proof.window", "Proof deadline overflows"))?;
        let tip = info.validated_ledger;
        let proof_max = deadline.min(tip);
        self.report.note_range("C: REVOKE", manifest_ledger, tip);
        if lsel < proof_max { self.report.note_range("B: PROOF", lsel + 1, proof_max); }
        // Paging saw all of L0. A walk from the CAPTURE's predecessor
        // missed its same-ledger successors, so include that suffix in B/C.
        let recent = if earlier.walked == 0 && l0 == tip {
            HistoryScan::default()
        } else {
            let from = if earlier.walked == 0 { l0 + 1 } else { l0 };
            let anchor = HistoryAnchor::Tip { capture: tx.hash, at: before, lsel, proof_max };
            self.history("B/C: PROOF/REVOKE", &envelope.account, from, tip, anchor, info).await?
        };
        let mut revoked_from = None;
        let mut ignored_revokes = 0;
        for (entry, memo) in earlier.records.iter().chain(&recent.records) {
            if memo.kind != MemoKind::Revoke.as_str() { continue; }
            let Ok(revoke) = RevokeMemo::decode(&memo.data) else { ignored_revokes += 1; continue };
            if revoke.registration_tx != core.registration_tx { continue; }
            let effective = if revoke.effective_ledger == 0 { entry.ledger_index } else { revoke.effective_ledger };
            if revoke.network != self.network() || effective < manifest_ledger || effective > entry.ledger_index {
                ignored_revokes += 1;
                continue;
            }
            if revoked_from.is_none_or(|(current, _)| effective < current) { revoked_from = Some((effective, entry.hash)); }
        }
        drop(earlier);
        let ignored = if ignored_revokes > 0 {
            format!("; {ignored_revokes} REVOKE record(s) ignored as malformed, future-dated or out of range")
        } else {
            String::new()
        };
        if let Some((effective, revoke_tx)) = revoked_from {
            if l0 >= effective {
                return Err(Stop::invalid("registration.revocation", format!(
                    "Credential revoked from ledger {effective} by {} (CAPTURE ledger {l0}); credential state as of ledger {}{ignored}",
                    hex::encode_upper(revoke_tx), info.validated_ledger,
                )));
            }
        }
        self.report.pass("registration.revocation", format!(
            "No applicable REVOKE; credential state as of ledger {}{ignored}", info.validated_ledger,
        ));

        if lpre == 0 || lpre >= l0 || l0 - lpre > windows.w_cap {
            return Err(Stop::invalid("ledger.preledger_window", format!(
                "CAPTURE ledger {l0} is outside the window ({lpre}, {lpre} + {}]", windows.w_cap,
            )));
        }
        let pre = self.header(lpre, "ledger.preledger_window").await?;
        if pre.hash != core.preledger_hash {
            return Err(Stop::invalid("ledger.preledger_window", format!("CAPTURE Lpre hash differs from validated ledger {lpre}")));
        }
        self.report.pass("ledger.preledger_window", format!("Validated Lpre {lpre}; CAPTURE included at {l0}, within {} ledgers", windows.w_cap));
        if let Some(proof) = &mut self.report.proof { proof.lsel = Some(lsel); proof.deadline = Some(deadline); }
        if lsel > info.validated_ledger {
            return Err(Stop::pending("ledger.selection", format!(
                "Selection ledger {lsel} is not validated yet (latest {}); the proof window closes at ledger {deadline}", info.validated_ledger,
            )));
        }
        let selection = self.header(lsel, "ledger.selection").await?;
        self.report.pass("ledger.selection", format!("Selection ledger Lsel = L0 + {} = {lsel} is validated", windows.sel_offset));
        let rgb = field::fr_from_be_canonical(&core.rgb_root).map_err(|err| Stop::invalid("capture.decode", err.to_string()))?;
        let evidence = field::fr_from_be_canonical(&core.evidence_root).map_err(|err| Stop::invalid("capture.decode", err.to_string()))?;
        let seed = sampling::seed(self.network(), &capture_id, lsel, &selection.hash);
        let tiles = sampling::select_tiles(&seed);
        if let Some(proof) = &mut self.report.proof {
            proof.selection_hash = Some(hex::encode_upper(selection.hash));
            proof.seed = Some(hex::encode(seed));
            proof.sampled_tiles = Some(tiles);
        }
        self.report.pass("ledger.challenge", "Recomputed seed, 16 sampled tiles and the 22 public inputs from the Lsel hash");
        let statement = Statement {
            account: envelope.account, manifest_tx: refs.manifest_tx, circuit_id: core.circuit_id, lsel,
            lsel_hash: selection.hash, deadline, public: public_inputs_v1(&rgb, &evidence, &capture_id, &seed, &tiles),
        };
        let mut discovered = Vec::new();
        for (entry, memo) in recent.records {
            if memo.kind == MemoKind::Proof.as_str() && ProofMemo::decode(&memo.data).is_ok_and(|proof| proof.manifest_tx == tx.hash) {
                if discovered.len() >= MAX_PROOFS {
                    return Err(Stop::unavailable("proof.found", format!(
                        "More than {MAX_PROOFS} PROOF transactions reference this CAPTURE; not evaluated",
                    )));
                }
                discovered.push(entry);
            }
        }
        self.proofs(refs, &statement, &circuit, info, discovered, exact).await
    }

    /// DEVICE manifest, bundle and attestation of the registration named in the CAPTURE.
    async fn registration(&mut self, capture: &CaptureMemo, tx: &LedgerTx, info: &ServerInfo) -> Result<(VerifiedCredential, u32), Stop> {
        let core = &capture.core;
        let check = "registration.resolve";
        let Some(registration) = self.tx(&core.registration_tx, check).await? else {
            let stop = missing_ref(self.ctx.config.inception_ledger, info, check, "DEVICE registration");
            return Err(if stop.state == State::NotFound {
                Stop::invalid(check, "The CAPTURE names a DEVICE registration that does not exist in the consulted history")
            } else {
                stop
            });
        };
        check_success(&registration, info, check, "DEVICE registration")?;
        let envelope = decode_kind(&registration, MemoKind::Device, check)?;
        if envelope.account_txn_id.is_none_or(|hash| hash == [0; 32]) {
            return Err(Stop::invalid("registration.txn_chain", "DEVICE has no nonzero AccountTxnID predecessor"));
        }
        self.report.pass("registration.txn_chain", "DEVICE binds its AccountTxnID predecessor in the master-key signature");
        let fetched = fetch_bundle_from_tx(self.ctx.ledger, &core.registration_tx, &registration, info, self.network(), self.ctx.config.allow_mainnet)
            .await.map_err(|err| Stop::from_rpc("registration.bundle", &err, true))?;
        let Manifest::Device(manifest) = fetched.manifest else {
            return Err(Stop::invalid(check, "The CAPTURE registration is not a DEVICE manifest"));
        };
        if (fetched.manifest_ledger, fetched.manifest_tx_index) >= (tx.ledger_index, tx.transaction_index) {
            return Err(Stop::invalid(check, "DEVICE registration is not earlier than the CAPTURE"));
        }
        if fetched.account != core.account_id {
            return Err(Stop::invalid("capture.account_binding", "DEVICE registration account differs from the CAPTURE account"));
        }
        self.report.note_range("DEVICE chunks", manifest.first_chunk_ledger, manifest.last_chunk_ledger);
        let reg = self.header(manifest.reg_ledger_index, check).await?;
        if reg.hash != manifest.reg_ledger_hash {
            return Err(Stop::invalid(check, format!(
                "DEVICE registration names ledger {} with a hash that differs from the validated ledger", manifest.reg_ledger_index,
            )));
        }
        self.report.pass(check, format!(
            "DEVICE manifest validated in ledger {}; registration ledger {} hash matches", fetched.manifest_ledger, manifest.reg_ledger_index,
        ));
        let bundle = DeviceBundle::decode(&fetched.bytes).map_err(|err| Stop::invalid("registration.bundle", err.to_string()))?;
        self.report.pass("registration.bundle", format!(
            "Device bundle reassembled from valid chunks; bundle_id and manifest match ({} non-records ignored)", fetched.ignored_records,
        ));
        let close_time = self.header(fetched.manifest_ledger, "registration.attestation").await?.close_time;
        let accepted = AcceptedAttestation {
            app_ids: self.ctx.config.accepted_app_ids.clone(),
            environments: self.ctx.config.accepted_attestation_environments.clone(),
        };
        let credential = verify_registration(&bundle, &manifest, close_time, &accepted).map_err(|err| match err {
            AttestError::Environment | AttestError::AppId => Stop::unsupported("registration.attestation", format!(
                "{err}: app {} / environment {:?} is not accepted by this network config", bundle.app_id, bundle.env,
            )),
            _ => Stop::invalid("registration.attestation", format!("{err} ({})", err.code())),
        })?;
        let label = match credential.class() {
            AttestationClass::AppleDevelopment => "apple-development",
            AttestationClass::AppleProduction => "apple-production",
            AttestationClass::DevUnattested => "dev-unattested",
            _ => return Err(Stop::unsupported("registration.attestation", "Unknown credential class")),
        };
        if let Some(report) = &mut self.report.capture {
            report.attestation = Some(label);
            report.app_id = Some(credential.app_id().to_owned());
        }
        if credential.class() == AttestationClass::DevUnattested {
            self.dev_credential = true;
            self.report.set("registration.attestation", Status::Fail, format!(
                "{DEV_LABEL}: Ed25519 key without App Attest; can never yield VERIFIED",
            ));
        } else {
            self.report.pass("registration.attestation", format!(
                "App Attest chain to the pinned Apple root valid at the DEVICE ledger close time ({label}, app {})", credential.app_id(),
            ));
        }
        Ok((credential, fetched.manifest_ledger))
    }

    async fn history(
        &mut self, name: &'static str, account: &AccountId, from: u32, to: u32,
        anchor: HistoryAnchor, info: &ServerInfo,
    ) -> Result<HistoryScan, Stop> {
        const CHECK: &str = "history.complete";
        self.report.note_range(name, from, to);
        if !covers(&info.complete_ledgers, from, to) {
            return Err(Stop::unavailable(CHECK, format!("{name} requires complete history in ledgers {from}..{to}")));
        }
        let mut scan = HistoryScan::default();
        let mut marker = None;
        let mut markers = HashSet::new();
        for _ in 0..self.ctx.options.history_pages {
            let page = self.ctx.ledger.account_tx(account, from, to, marker, PAGE_LIMIT).await
                .map_err(|err| Stop::unavailable(CHECK, format!("{}; {}", scan.detail(name), Stop::from_rpc(CHECK, &err, false).detail)))?;
            scan.pages += 1;
            scan.rows += page.transactions.len();
            if page.transactions.len() > usize::from(PAGE_LIMIT) {
                return Err(Stop::unavailable(CHECK, format!("{}; page exceeded its row limit", scan.detail(name))));
            }
            for entry in page.transactions {
                if !(from..=to).contains(&entry.ledger_index) {
                    return Err(Stop::unavailable(CHECK, format!("{}; transaction outside requested window", scan.detail(name))));
                }
                let envelope = if entry.validated && entry.transaction_result == "tesSUCCESS" {
                    decode_galaxy_memo(&entry.tx_blob).ok()
                } else { None };
                self.collect_record(&mut scan, account, anchor, entry, envelope);
            }
            marker = page.marker;
            let Some(next) = &marker else {
                self.finish_history(name, &mut scan)?;
                return Ok(scan);
            };
            if !markers.insert(next.to_string()) {
                return Err(Stop::unavailable(CHECK, format!("{}; account_tx pagination did not advance", scan.detail(name))));
            }
        }
        // Partial pagination must not contribute candidates to a fallback result.
        scan.records.clear();
        if let Err(reason) = self.walk_history(account, from, to, anchor, &mut scan).await {
            return Err(Stop::unavailable(CHECK, format!("{}; {reason}", scan.detail(name))));
        }
        self.finish_history(name, &mut scan)?;
        Ok(scan)
    }

    fn collect_record(&self, scan: &mut HistoryScan, account: &AccountId, anchor: HistoryAnchor, entry: LedgerTx, envelope: Option<GalaxyMemo>) {
        let Some((kind_found, memo)) = envelope.and_then(|memo| {
            if memo.account != *account || !network_field(&memo, self.network()) {
                return None;
            }
            let kind = MemoKind::from_memo_type(&memo.kind)?;
            if kind == MemoKind::Proof && memo.account_txn_id.is_none_or(|hash| hash == [0; 32]) {
                return None;
            }
            Some((kind, memo))
        }) else {
            scan.ignored += 1;
            return;
        };
        if anchor.accepts(kind_found, (entry.ledger_index, entry.transaction_index)) {
            scan.records.push((entry, memo));
        }
    }

    fn finish_history(&mut self, name: &str, scan: &mut HistoryScan) -> Result<(), Stop> {
        const CHECK: &str = "history.complete";
        scan.records.sort_unstable_by_key(|(tx, _)| (tx.ledger_index, tx.transaction_index));
        if scan.records.windows(2).any(|pair| {
            let (a, b) = (&pair[0].0, &pair[1].0);
            (a.ledger_index, a.transaction_index) == (b.ledger_index, b.transaction_index) && a.hash != b.hash
        }) {
            return Err(Stop::unavailable(CHECK, format!("{}; conflicting transaction positions", scan.detail(name))));
        }
        scan.records.dedup_by_key(|(tx, _)| (tx.ledger_index, tx.transaction_index));
        let detail = scan.detail(name);
        let detail = match self.report.check(CHECK).filter(|check| check.status == Status::Pass) {
            Some(check) => format!("{}; {detail}", check.detail),
            None => detail,
        };
        self.report.pass(CHECK, detail);
        Ok(())
    }

    async fn walk_history(
        &self, account: &AccountId, from: u32, to: u32, anchor: HistoryAnchor, scan: &mut HistoryScan,
    ) -> Result<(), &'static str> {
        let (mut hash, mut before) = match anchor {
            HistoryAnchor::Capture { predecessor, before, .. } => (predecessor, before),
            HistoryAnchor::Tip { .. } => {
                let root = self.ctx.ledger.account_root(account, to).await.map_err(|_| "validated AccountRoot unavailable")?;
                let hash = root.account_txn_id.filter(|hash| *hash != [0; 32]).ok_or("AccountRoot has no nonzero AccountTxnID")?;
                (hash, (to, u32::MAX))
            }
        };
        let mut reached_registration = false;
        loop {
            if let HistoryAnchor::Tip { capture, at, .. } = anchor {
                // The boundary CAPTURE was already fetched and checked.
                if hash == capture {
                    return if at < before { Ok(()) } else { Err("AccountTxnID transaction has invalid status or order") };
                }
            }
            if scan.walked >= self.ctx.options.history_walk {
                return Err("AccountTxnID walk budget exhausted");
            }
            let tx = self.tx(&hash, "history.complete").await
                .map_err(|_| "AccountTxnID transaction unavailable or inconsistent")?
                .ok_or("AccountTxnID transaction missing")?;
            scan.walked += 1;
            let here = (tx.ledger_index, tx.transaction_index);
            if !tx.validated || tx.transaction_result != "tesSUCCESS" || here >= before || here.0 == 0 {
                return Err("AccountTxnID transaction has invalid status or order");
            }
            let object = codec::decode(&tx.tx_blob).map_err(|_| "AccountTxnID transaction decoding failed")?;
            let signer = object["Account"].as_str().and_then(|s| decode_address(s).ok());
            if signer.as_ref() != Some(account) {
                return Err("AccountTxnID link does not name an own transaction");
            }
            match anchor {
                HistoryAnchor::Capture { registration, .. } => {
                    if here.0 < from {
                        return if reached_registration { Ok(()) } else { Err("AccountTxnID chain skipped the DEVICE manifest") };
                    }
                    reached_registration |= here.0 == from && hash == registration;
                }
                HistoryAnchor::Tip { at, .. } => {
                    if here <= at { return Err("AccountTxnID chain skipped the CAPTURE"); }
                    if here.0 < from { return Ok(()); }
                }
            }
            let previous = match account_txn_id(&object).map_err(|_| "invalid signed AccountTxnID")? {
                Some(previous) => previous,
                None => {
                    let previous = tx.previous_account_txn_id(account).map_err(|_| "invalid AccountRoot AccountTxnID metadata")?
                        .ok_or("missing AccountRoot PreviousFields.AccountTxnID")?;
                    scan.rpc_links += 1;
                    previous
                }
            };
            let envelope = decode_galaxy_object(&object).ok();
            self.collect_record(scan, account, anchor, tx, envelope);
            if previous == [0; 32] {
                return if matches!(anchor, HistoryAnchor::Capture { .. }) && here.0 == from && reached_registration {
                    Ok(())
                } else { Err("AccountTxnID chain ends before completing the requested window") };
            }
            before = here;
            hash = previous;
        }
    }

    fn evaluate_proof(&self, tx: &LedgerTx, statement: &Statement, circuit: &Circuit, latest: u32) -> ProofOutcome {
        let mut out = ProofOutcome { verdict: Verdict::Rejected, checks: Vec::new(), summary: String::new(), unvalidated: false };
        out.pass("proof.found", "PROOF transaction retrieved; blob hash recomputed");
        if !tx.validated || tx.ledger_index > latest {
            out.unvalidated = true;
            return out.end(Verdict::Rejected, "proof.validated_success", Status::Unavailable, "PROOF transaction is not validated yet");
        }
        if tx.transaction_result != "tesSUCCESS" {
            return out.end(Verdict::Rejected, "proof.validated_success", Status::Fail,
                format!("PROOF transaction failed with {}", tx.transaction_result));
        }
        out.pass("proof.validated_success", format!("Validated with tesSUCCESS in ledger {}", tx.ledger_index));
        let Ok(envelope) = decode_galaxy_memo(&tx.tx_blob) else {
            return out.end(Verdict::Rejected, "proof.binding", Status::Fail, "Not a Galaxy AccountSet envelope");
        };
        if envelope.kind != MemoKind::Proof.as_str() {
            return out.end(Verdict::Rejected, "proof.binding", Status::Fail, "Not a Galaxy PROOF memo");
        }
        if envelope.account != statement.account {
            return out.end(Verdict::Rejected, "proof.account", Status::Fail, "PROOF is not sent by the CAPTURE account");
        }
        if envelope.account_txn_id.is_none_or(|hash| hash == [0; 32]) {
            return out.end(Verdict::Rejected, "proof.account", Status::Fail, "PROOF has no nonzero AccountTxnID predecessor");
        }
        out.pass("proof.account", "PROOF is sent by the CAPTURE account");
        let memo = match ProofMemo::decode(&envelope.data) {
            Ok(memo) => memo,
            Err(err) => return out.end(Verdict::Rejected, "proof.binding", Status::Fail, format!("Malformed PROOF memo: {err}")),
        };
        if memo.manifest_tx != statement.manifest_tx {
            return out.end(Verdict::Rejected, "proof.binding", Status::Fail, "PROOF references a different CAPTURE");
        }
        if tx.ledger_index <= statement.lsel || tx.ledger_index > statement.deadline {
            return out.end(Verdict::Rejected, "proof.window", Status::Fail, format!(
                "PROOF ledger {} is outside the window ({}, {}]", tx.ledger_index, statement.lsel, statement.deadline,
            ));
        }
        out.pass("proof.window", format!("PROOF ledger {} within ({}, {}]", tx.ledger_index, statement.lsel, statement.deadline));
        if memo.network != self.network() || !network_field(&envelope, self.network())
            || memo.selection_ledger_index != statement.lsel || memo.selection_ledger_hash != statement.lsel_hash
            || memo.circuit_id != statement.circuit_id
        {
            return out.end(Verdict::Invalid, "proof.binding", Status::Fail,
                "PROOF network, selection ledger, selection hash or circuit differs from the CAPTURE challenge");
        }
        out.pass("proof.binding", "PROOF binds the CAPTURE, Lsel, its hash and the CAPTURE circuit");
        let proof = match groth16::Proof::from_bytes(&memo.proof) {
            Ok(proof) => proof,
            Err(err) => return out.end(Verdict::Invalid, "proof.groth16", Status::Fail, format!("Malformed Groth16 proof: {err}")),
        };
        match groth16::verify(&circuit.vk, &statement.public, &proof) {
            Ok(true) => {
                out.verdict = Verdict::Valid;
                out.summary = "Groth16 proof valid for the recomputed public inputs".into();
                out.pass("proof.groth16", "Groth16 proof verifies against the pinned verifying key and recomputed public inputs");
                out
            }
            Ok(false) => out.end(Verdict::Invalid, "proof.groth16", Status::Fail, "Groth16 pairing check failed for the recomputed public inputs"),
            Err(err) => out.end(Verdict::Invalid, "proof.groth16", Status::Fail, format!("Groth16 verification error: {err}")),
        }
    }

    async fn proofs(
        &mut self, refs: Refs, statement: &Statement, circuit: &Circuit, info: &ServerInfo,
        discovered: Vec<LedgerTx>, exact: Option<LedgerTx>,
    ) -> Result<(), Stop> {
        let latest = info.validated_ledger;
        let window_open = latest < statement.deadline;
        self.report.note_range("PROOF window", statement.lsel + 1, statement.deadline);
        let exact_mode = exact.is_some();
        let mut candidates: Vec<LedgerTx> = match exact {
            Some(tx) => vec![tx],
            None => discovered,
        };
        // An advertised (untrusted) PROOF is evaluated and listed, but cannot select itself:
        // only PROOFs found in the account history compete in ledger order.
        let mut advertised = None;
        if !exact_mode {
            if let Some(hash) = refs.proof_tx.filter(|hash| candidates.iter().all(|tx| tx.hash != *hash)) {
                advertised = Some(match self.tx(&hash, "proof.found").await {
                    Ok(Some(tx)) => {
                        let mut outcome = self.evaluate_proof(&tx, statement, circuit, latest);
                        if outcome.verdict == Verdict::Valid {
                            outcome.verdict = Verdict::Rejected;
                            outcome.summary = "Advertised PROOF verifies but was not returned by the account history; not used".into();
                        }
                        candidate(hash, refs.manifest_tx, outcome)
                    }
                    Ok(None) => Candidate {
                        manifest_tx: hex::encode_upper(refs.manifest_tx), proof_tx: Some(hex::encode_upper(hash)),
                        state: State::NotFound, summary: "Advertised PROOF transaction not found".into(),
                        checks: Vec::new(), proofs: Vec::new(),
                    },
                    Err(stop) => Candidate {
                        manifest_tx: hex::encode_upper(refs.manifest_tx), proof_tx: Some(hex::encode_upper(hash)),
                        state: stop.state, summary: stop.detail, checks: Vec::new(), proofs: Vec::new(),
                    },
                });
            }
        }
        candidates.truncate(MAX_PROOFS);
        let mut selected: Option<(LedgerTx, ProofOutcome)> = None;
        let mut first_invalid: Option<ProofOutcome> = None;
        let mut exact_rejection: Option<ProofOutcome> = None;
        let mut first_rejected: Option<ProofOutcome> = None;
        let mut listed = Vec::new();
        for tx in candidates {
            let outcome = self.evaluate_proof(&tx, statement, circuit, latest);
            match outcome.verdict {
                Verdict::Valid => {
                    listed.push(candidate(tx.hash, refs.manifest_tx, ProofOutcome { checks: Vec::new(), ..outcome_copy(&outcome) }));
                    selected = Some((tx, outcome));
                    break;
                }
                Verdict::Invalid => {
                    listed.push(candidate(tx.hash, refs.manifest_tx, outcome_copy(&outcome)));
                    if first_invalid.is_none() { first_invalid = Some(outcome); }
                }
                Verdict::Rejected => {
                    listed.push(candidate(tx.hash, refs.manifest_tx, outcome_copy(&outcome)));
                    if exact_mode {
                        exact_rejection = Some(outcome);
                    } else if first_rejected.is_none() {
                        first_rejected = Some(outcome);
                    }
                }
            }
        }
        listed.extend(advertised);
        self.report.candidates = listed;
        if let Some((tx, outcome)) = selected {
            for check in outcome.checks { self.report.set_check(check); }
            let name = circuit.info.policy.name.clone();
            for id in POLICY_CHECKS {
                self.report.pass(id, format!("Constrained by accepted circuit for policy {name}; holds because proof.groth16 passed"));
            }
            if let Some(proof) = &mut self.report.proof {
                proof.proof_tx = Some(hex::encode_upper(tx.hash));
                proof.lproof = Some(tx.ledger_index);
            }
            self.report.state = State::Verified;
            self.report.summary = format!(
                "Physical consistency verified under Galaxy policy {name} ({}).", circuit.info.policy_id,
            );
            return Ok(());
        }
        let failing = first_invalid.or(exact_rejection);
        if let Some(outcome) = failing {
            let pending = outcome.unvalidated;
            let summary = outcome.summary.clone();
            for check in outcome.checks { self.report.set_check(check); }
            if pending {
                self.report.state = State::Pending;
                self.report.summary = format!("The PROOF transaction is not validated yet; window closes at ledger {}", statement.deadline);
            } else if window_open && !exact_mode {
                self.report.state = State::Pending;
                self.report.summary = format!(
                    "No valid PROOF yet ({summary}); a valid PROOF may still be published until ledger {}", statement.deadline,
                );
            } else {
                self.report.state = State::Invalid;
                self.report.summary = format!("PROOF failed verification: {summary}");
            }
            return Ok(());
        }
        // Show why the earliest rejected candidate was not usable; the state stays PENDING/INCONCLUSIVE.
        if let Some(outcome) = first_rejected {
            for check in outcome.checks { self.report.set_check(check); }
        }
        if window_open {
            self.report.set("proof.found", Status::Unavailable, format!(
                "No PROOF yet in ledgers {}..{latest}; window closes at ledger {}", statement.lsel + 1, statement.deadline,
            ));
            self.report.state = State::Pending;
            self.report.summary = format!("Capture is valid; its PROOF window is open until ledger {}", statement.deadline);
        } else {
            self.report.set("proof.found", Status::Fail, format!(
                "No valid PROOF from the CAPTURE account in ledgers {}..{}", statement.lsel + 1, statement.deadline,
            ));
            self.report.state = State::Inconclusive;
            self.report.summary = "No accepted physical-consistency proof was published within the window.".into();
        }
        Ok(())
    }
}

fn outcome_copy(outcome: &ProofOutcome) -> ProofOutcome {
    ProofOutcome { verdict: outcome.verdict, checks: outcome.checks.clone(), summary: outcome.summary.clone(), unvalidated: outcome.unvalidated }
}

fn candidate(proof_tx: Hash32, manifest_tx: Hash32, outcome: ProofOutcome) -> Candidate {
    Candidate {
        manifest_tx: hex::encode_upper(manifest_tx),
        proof_tx: Some(hex::encode_upper(proof_tx)),
        state: match outcome.verdict {
            Verdict::Valid => State::Verified,
            Verdict::Invalid => State::Invalid,
            Verdict::Rejected if outcome.unvalidated => State::Pending,
            Verdict::Rejected => State::NotFound,
        },
        summary: outcome.summary,
        checks: outcome.checks,
        proofs: Vec::new(),
    }
}

/// Absent transaction: NOT_FOUND only when the RPC claims complete history since inception.
fn missing_ref(inception: u32, info: &ServerInfo, check: &'static str, what: &str) -> Stop {
    let (from, to) = (inception.max(1), info.validated_ledger);
    if covers(&info.complete_ledgers, from, to) {
        Stop::new(State::NotFound, check, format!("{what} transaction not found in ledgers {from}..{to}"))
    } else {
        Stop::unavailable(check, format!(
            "{what} transaction not found and the RPC history ({}) does not cover ledgers {from}..{to}", info.complete_ledgers,
        ))
    }
}

fn check_success(tx: &LedgerTx, info: &ServerInfo, check: &'static str, what: &str) -> Result<(), Stop> {
    if !tx.validated || tx.ledger_index > info.validated_ledger {
        return Err(Stop::pending(check, format!("{what} transaction is not validated yet")));
    }
    if tx.transaction_result != "tesSUCCESS" {
        return Err(Stop::invalid(check, format!("{what} transaction failed with {}", tx.transaction_result)));
    }
    Ok(())
}

fn decode_kind(tx: &LedgerTx, kind: MemoKind, check: &'static str) -> Result<GalaxyMemo, Stop> {
    let memo = decode_galaxy_memo(&tx.tx_blob).map_err(|err| Stop::invalid(check, err.to_string()))?;
    if memo.kind != kind.as_str() {
        return Err(Stop::invalid(check, format!("Transaction carries {} instead of {}", memo.kind, kind.as_str())));
    }
    Ok(memo)
}

/// §12: no NetworkID field for network ids ≤ 1024, mandatory above.
fn network_field(envelope: &GalaxyMemo, network: u32) -> bool {
    envelope.network_id == (network > 1024).then_some(network)
}

fn same_capture(a: &CaptureCore, b: &CaptureCore) -> bool {
    a.photo_id == b.photo_id || a.rgb_root == b.rgb_root || a.evidence_root == b.evidence_root
}
