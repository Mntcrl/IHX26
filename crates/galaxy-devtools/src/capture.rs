use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use clap::{Args, ValueEnum};
use galaxy_attestation::verify_capture_assertion;
use galaxy_crypto::{field::fr_to_be, sampling::Challenge};
use galaxy_evidence::{bundle, ingest, try_commit, Salts};
use galaxy_geometry::{build_view, synthetic::{self, Scenario}, try_evaluate};
use galaxy_image::{ImageProfile, HEIGHT, MAX_IMAGE_BYTES, WIDTH};
use galaxy_protocol::{
    ids,
    policy::Policy,
    wire::{CaptureCore, CaptureMemo, CircuitBundle, MemoKind, ProofMemo},
    xmp::GalaxyXmp,
    Hash32,
};
use galaxy_prover::{Phase, ProveControl, Prover};
use galaxy_xrpl::{
    ledger::{LedgerHeader, LedgerTx, RpcTransport},
    registry::{fetch_bundle, Manifest},
    submission::{PendingTx, TxState},
    transaction::Memo,
    LedgerSource,
};
use serde_json::{json, Value};

use crate::{delay, error, hash, ledger, output, registry, storage, Profile, Result, Session, DEVELOPMENT, NETWORK};

#[derive(Args)]
pub(crate) struct CaptureArgs {
    #[arg(long, value_parser = hash)]
    device_tx: Hash32,
    #[arg(long, default_value = "dev-device")]
    device: String,
    #[arg(long, value_parser = hash)]
    circuit_tx: Hash32,
    #[arg(long, default_value = "circuits/artifacts/v1m/main_v1m.graph.bin")]
    graph: PathBuf,
    #[arg(long, default_value = "circuits/artifacts/v1m/main_v1m.zkey")]
    zkey: PathBuf,
    #[arg(long)]
    raw: Option<PathBuf>,
    #[arg(long, conflicts_with = "raw")]
    synthetic_seed: Option<u64>,
    #[arg(long, value_enum, default_value = "png-v1")]
    profile: Profile,
    /// Mode-0700 local directory. Do not publish contents except the explicit image and run record.
    #[arg(long)]
    run_dir: PathBuf,
    /// Development transaction experiments. These never write an exported image for an expired proof.
    #[arg(long, value_enum, default_value = "normal")]
    case: Case,
    /// Submit CAPTURE, then exit without observing validation. Rerun without this flag to recover.
    #[arg(long, conflicts_with = "duplicate_capture")]
    interrupt_after_capture: bool,
    /// During the original CAPTURE submission, submit the same signed blob a second time.
    #[arg(long)]
    duplicate_capture: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Case {
    Normal,
    InvalidProof,
    ExpiredProof,
}

struct Submission {
    tx: LedgerTx,
    submit_to_validated_ms: Option<u128>,
    engine_result: Option<String>,
    duplicate_engine_result: Option<String>,
}

async fn submit(session: &Session<'_>, pending: &PendingTx, duplicate: bool) -> Result<Submission> {
    let mut started = None;
    let mut engine_result = None;
    let mut duplicate_engine_result = None;
    let state = session.submitter.state(pending).await?;
    let state = if matches!(state, TxState::Pending) {
        if session.submitter.validated_ledger().await? < pending.last_ledger {
            started = Some(Instant::now());
            engine_result = Some(session.submitter.submit_saved(pending).await?.code);
            if duplicate {
                duplicate_engine_result = Some(session.submitter.submit_saved(pending).await?.code);
            }
        }
        session.submitter.poll(pending, delay).await?
    } else {
        state
    };
    if !state.succeeded() {
        return Err(error(&match state {
            TxState::Expired => format!("{} expired without validation", hex::encode_upper(pending.hash)),
            TxState::Validated(tx) => format!("{} validated with {}", hex::encode_upper(pending.hash), tx.transaction_result),
            TxState::Pending => "submission did not reach a final state".into(),
        }));
    }
    let TxState::Validated(tx) = state else { unreachable!("success is validated") };
    Ok(Submission { tx, submit_to_validated_ms: started.map(|t| t.elapsed().as_millis()), engine_result, duplicate_engine_result })
}

async fn wait_for(session: &Session<'_>, index: u32) -> Result<LedgerHeader> {
    loop {
        if session.submitter.validated_ledger().await? >= index {
            return ledger(session.rpc, index).await;
        }
        delay().await;
    }
}

async fn check_history(session: &Session<'_>, first: &LedgerTx, second: &LedgerTx) -> Result<usize> {
    let mut marker = None;
    let mut seen_markers = HashSet::new();
    let mut capture_seen = 0;
    let mut proof_seen = 0;
    loop {
        let page = session.rpc.account_tx(&session.wallet.account_id(), first.ledger_index, second.ledger_index, marker, 1).await?;
        for tx in page.transactions {
            capture_seen += usize::from(tx == *first || (tx.hash == first.hash && tx.tx_blob == first.tx_blob && tx.transaction_result == "tesSUCCESS"));
            proof_seen += usize::from(tx == *second || (tx.hash == second.hash && tx.tx_blob == second.tx_blob && tx.transaction_result == "tesSUCCESS"));
        }
        marker = page.marker;
        let Some(next) = &marker else { break };
        if !seen_markers.insert(serde_json::to_string(next)?) || seen_markers.len() >= 1024 {
            return Err(error("account_tx pagination did not terminate"));
        }
    }
    if (capture_seen, proof_seen) != (1, 1) {
        return Err(error("account_tx did not find exactly one CAPTURE and PROOF"));
    }
    Ok(seen_markers.len() + 1)
}

pub(crate) async fn run(session: &Session<'_>, args: CaptureArgs) -> Result<()> {
    let dir = storage::PrivateDir::open(&args.run_dir)?;
    let _lock = dir.lock()?;
    let profile: ImageProfile = args.profile.into();
    let key = registry::device_key(session, &args.device, false)?;
    let credential = registry::credential(session, &args.device_tx, &key).await?;
    let registered = fetch_bundle(session.rpc, &args.circuit_tx, NETWORK, false).await?;
    let Manifest::Circuit(manifest) = registered.manifest else { return Err(error("registration is not a CIRCUIT transaction")) };
    let circuit = CircuitBundle::decode(&registered.bytes)?;
    let policy: Policy = serde_json::from_slice(&circuit.policy_json)?;
    policy.validate()?;

    let intent = json!({
        "mode": DEVELOPMENT, "network": NETWORK,
        "account": session.wallet.classic_address(), "device": args.device,
        "device_tx": hex::encode_upper(args.device_tx), "circuit_tx": hex::encode_upper(args.circuit_tx),
        "profile": profile, "case": args.case,
    });
    let source_mode = match dir.read("intent.json", 4096)? {
        Some(bytes) => {
            let saved: Value = serde_json::from_slice(&bytes)?;
            if saved["capture"] != intent {
                return Err(error("saved development run uses different capture arguments"));
            }
            match saved["source"].as_str() {
                Some("raw") if args.synthetic_seed.is_none() => "raw",
                Some("synthetic") if args.raw.is_none() && (args.synthetic_seed.is_none() || saved["synthetic_seed"].as_u64() == args.synthetic_seed) => "synthetic",
                _ => return Err(error("saved development run uses another evidence source")),
            }
        }
        None => {
            let source = if args.raw.is_some() { "raw" } else if args.synthetic_seed.is_some() { "synthetic" } else {
                return Err(error("new runs require exactly one of --raw or --synthetic-seed"));
            };
            let saved = json!({"capture": intent, "source": source, "synthetic_seed": args.synthetic_seed});
            dir.write("intent.json", &serde_json::to_vec(&saved)?)?;
            source
        }
    };

    let start = Instant::now();
    let (evidence, salts) = match dir.read("capture.gxev", storage::EVIDENCE_CAP)? {
        Some(bytes) => {
            let saved = bundle::decode(&bytes).map_err(|_| error("could not decode private evidence journal"))?;
            (saved.evidence, saved.salts)
        }
        None => {
            let evidence = if let Some(raw) = &args.raw {
                let bytes = storage::read(raw, storage::EVIDENCE_CAP)?;
                ingest::from_gxrw(&bytes, profile).map_err(|_| error("GXRW ingest rejected the private sensor evidence"))?
            } else if let Some(seed) = args.synthetic_seed {
                synthetic::synthesize_with_profile(Scenario::Room, seed, &policy, profile)
                    .map_err(|_| error("synthetic room generation failed"))?
            } else {
                return Err(error("capture was interrupted before GXEV persistence; supply the original evidence source"));
            };
            let salts = Salts::random().map_err(|_| error("could not generate private salts"))?;
            let bytes = bundle::encode(&evidence, &salts).map_err(|_| error("could not encode private evidence journal"))?;
            dir.write("capture.gxev", &bytes)?;
            (evidence, salts)
        }
    };
    if evidence.image.profile != profile {
        return Err(error("saved development evidence profile mismatch"));
    }
    let evidence_ms = start.elapsed().as_millis();
    let start = Instant::now();
    let commitments = try_commit(&evidence, &salts).map_err(|_| error("could not commit to private evidence"))?;
    let photo_id = galaxy_image::photo_id(&evidence.image.rgb)?;
    let rgb_root = fr_to_be(&commitments.rgb_root);
    let evidence_root = fr_to_be(&commitments.evidence_root);
    let commitment_ms = start.elapsed().as_millis();

    // Check that the desktop proving key matches the published circuit before
    // spending a CAPTURE fee. A proof from another graph must also pass the
    // registered verification key with exactly the derived public inputs.
    let prover = Prover::open(&args.graph, &args.zkey).map_err(|_| error("could not open desktop witness graph or zkey"))?;
    if prover.n_public() != 22 || prover.vk_bytes()? != circuit.vk_bytes {
        return Err(error("desktop proving key does not match the registered verifying key/public layout"));
    }
    let capture_bytes = match dir.read("capture.memo", 901)? {
        Some(bytes) => bytes,
        None => {
            let head = session.submitter.validated_ledger().await?;
            let pre = ledger(session.rpc, head).await?;
            let core = CaptureCore {
                network: NETWORK, registration_tx: args.device_tx, preledger_hash: pre.hash,
                nonce: storage::random()?, preledger_index: pre.ledger_index,
                claimed_time_ms: u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?,
                width: WIDTH as u16, height: HEIGHT as u16, flags: u16::from(profile.code()),
                photo_id, rgb_root, evidence_root, policy_id: manifest.policy_id,
                circuit_id: manifest.circuit_id, account_id: session.wallet.account_id(),
            };
            let bytes = CaptureMemo { assertion: registry::assertion(&core.encode(), &key), core }.encode();
            dir.write("capture.memo", &bytes)?;
            bytes
        }
    };
    let CaptureMemo { core, assertion } = CaptureMemo::decode(&capture_bytes)?;
    if core.network != NETWORK || core.registration_tx != args.device_tx || core.account_id != session.wallet.account_id()
        || core.circuit_id != manifest.circuit_id || core.policy_id != manifest.policy_id
        || core.photo_id != photo_id || core.rgb_root != rgb_root || core.evidence_root != evidence_root
        || core.flags != u16::from(profile.code())
    {
        return Err(error("saved CAPTURE is bound to different evidence, device or circuit"));
    }
    let core_bytes = core.encode();
    verify_capture_assertion(&credential, &core_bytes, &assertion)?;
    let capture_id = ids::capture_id(&core_bytes);
    let lpre = ledger(session.rpc, core.preledger_index).await?;
    if lpre.hash != core.preledger_hash {
        return Err(error("saved Lpre no longer matches XRPL"));
    }
    let capture_deadline = core.preledger_index.checked_add(policy.windows.w_cap).ok_or_else(|| error("capture deadline overflow"))?;
    let purpose = format!("development/{}/capture", hex::encode(capture_id));
    let pending = session.submitter.prepare(&purpose, session.wallet, Memo { kind: MemoKind::Capture.as_str(), data: &capture_bytes }, Some(capture_deadline)).await?;
    if args.interrupt_after_capture {
        let submit = session.submitter.submit_saved(&pending).await?;
        output(json!({"interrupted": "after-capture-submit", "capture_tx": hex::encode_upper(pending.hash), "engine_result": submit.code}));
        return Ok(());
    }
    let capture = submit(session, &pending, args.duplicate_capture).await?;
    let l0 = ledger(session.rpc, capture.tx.ledger_index).await?;
    if !(core.preledger_index < l0.ledger_index && l0.ledger_index <= capture_deadline) {
        return Err(error("validated CAPTURE falls outside the policy window"));
    }
    let lsel_index = l0.ledger_index.checked_add(policy.windows.sel_offset).ok_or_else(|| error("selection ledger overflow"))?;
    let proof_deadline = lsel_index.checked_add(policy.windows.w_proof).ok_or_else(|| error("proof deadline overflow"))?;
    let lsel = wait_for(session, lsel_index).await?;
    let challenge = Challenge::derive(NETWORK, &capture_id, lsel_index, &lsel.hash);
    let public = galaxy_witness::public_inputs(&commitments, &capture_id, &challenge).map(|f| fr_to_be(&f));
    let mut proof_timings = Value::Null;
    let proof_bytes = match dir.read("proof.memo", 361)? {
        Some(bytes) => bytes,
        None => {
            if session.submitter.validated_ledger().await? >= proof_deadline {
                return Err(error("proof window closed before proof generation"));
            }
            let start = Instant::now();
            let view = build_view(&evidence, &challenge).map_err(|_| error("private geometry view rejected"))?;
            if !try_evaluate(&view, &policy).map_err(|_| error("reference geometry evaluation failed"))?.pass {
                return Err(error("development capture INCONCLUSIVE: reference policy did not pass for the committed ledger challenge"));
            }
            let input = galaxy_witness::build_input(&evidence, &salts, &commitments, &challenge, &capture_id, &view)
                .map_err(|_| error("private circuit input construction failed"))?;
            let input = serde_json::to_string(&input).map_err(|_| error("private circuit input JSON encoding failed"))?;
            let input_ms = start.elapsed().as_millis();
            let desktop = prover.prove(&input, &ProveControl::default()).map_err(|_| error("desktop witness/proof or prover self-check failed"))?;
            if desktop.public_inputs != public || !galaxy_prover::verify(&circuit.vk_bytes, &public, &desktop.proof)? {
                return Err(error("desktop proof did not verify for the exact registered key and ledger-derived public inputs"));
            }
            proof_timings = json!({"build_input_ms": input_ms, "witness_seconds": desktop.timings.witness(),
                "prove_seconds": desktop.timings.prove(), "self_verify_seconds": desktop.timings.get(Phase::SelfVerify)});
            dir.write("prover-timings.json", &serde_json::to_vec(&proof_timings)?)?;
            let mut proof = desktop.proof;
            if args.case == Case::InvalidProof {
                // Replace C with A (a canonical on-curve point); this tests the
                // pairing check, not only malformed proof decoding.
                proof.copy_within(..64, 192);
                if galaxy_prover::verify(&circuit.vk_bytes, &public, &proof)? {
                    return Err(error("bad-proof experiment unexpectedly retained a valid proof"));
                }
            }
            let proof = ProofMemo { network: NETWORK, manifest_tx: capture.tx.hash, selection_ledger_index: lsel_index,
                selection_ledger_hash: lsel.hash, circuit_id: manifest.circuit_id, proof };
            let bytes = proof.encode();
            dir.write("proof.memo", &bytes)?;
            bytes
        }
    };
    if proof_timings.is_null() {
        if let Some(bytes) = dir.read("prover-timings.json", 4096)? {
            proof_timings = serde_json::from_slice(&bytes)?;
        }
    }
    let proof = ProofMemo::decode(&proof_bytes)?;
    if proof.network != NETWORK || proof.manifest_tx != capture.tx.hash || proof.selection_ledger_index != lsel_index
        || proof.selection_ledger_hash != lsel.hash || proof.circuit_id != manifest.circuit_id
        || galaxy_prover::verify(&circuit.vk_bytes, &public, &proof.proof)? != (args.case != Case::InvalidProof)
    {
        return Err(error("saved PROOF does not match the exact CAPTURE, selection ledger or experiment"));
    }
    let purpose = format!("development/{}/proof", hex::encode(capture_id));
    let pending = session.submitter.prepare(&purpose, session.wallet, Memo { kind: MemoKind::Proof.as_str(), data: &proof_bytes }, Some(proof_deadline)).await?;
    let mut report = json!({
        "mode": DEVELOPMENT, "format": "galaxy-development-run/v1", "source": source_mode,
        "case": args.case, "profile": profile, "credential": "development unattested credential",
        "photo_id": hex::encode(photo_id), "capture_id": hex::encode(capture_id),
        "device_registration_tx": hex::encode_upper(args.device_tx), "circuit_registration_tx": hex::encode_upper(args.circuit_tx),
        "Lpre": lpre.ledger_index, "L0": l0.ledger_index, "Lsel": lsel.ledger_index,
        "selection_ledger_hash": hex::encode_upper(lsel.hash), "seed": hex::encode(challenge.seed), "sampled_tiles": challenge.tiles,
        "capture": registry::observe(&capture.tx, session.config.max_fee_drops)?,
        "capture_engine_result": capture.engine_result, "duplicate_capture_engine_result": capture.duplicate_engine_result,
        "proof_deadline": proof_deadline, "capture_to_selection_ledger_seconds": lsel.close_time.checked_sub(l0.close_time),
        "capture_submit_to_validated_ms": capture.submit_to_validated_ms,
        "load_or_ingest_evidence_ms": evidence_ms, "commitment_ms": commitment_ms, "desktop_prover": proof_timings,
    });
    if args.case == Case::ExpiredProof {
        let expired = expired_experiment(session, &pending, proof_deadline).await?;
        report["transaction_count"] = json!(1);
        report["expired_proof"] = expired;
    } else {
        let submitted = submit(session, &pending, false).await?;
        let lproof = ledger(session.rpc, submitted.tx.ledger_index).await?;
        if !(lsel_index < lproof.ledger_index && lproof.ledger_index <= proof_deadline) {
            return Err(error("validated PROOF falls outside the policy window"));
        }
        let history_pages = check_history(session, &capture.tx, &submitted.tx).await?;
        report["transaction_count"] = json!(2);
        report["account_tx_pages"] = json!(history_pages);
        report["Lproof"] = json!(lproof.ledger_index);
        report["proof"] = json!(registry::observe(&submitted.tx, session.config.max_fee_drops)?);
        report["proof_engine_result"] = json!(submitted.engine_result);
        report["proof_submit_to_validated_ms"] = json!(submitted.submit_to_validated_ms);
        report["selection_to_proof_ledger_seconds"] = json!(lproof.close_time.checked_sub(lsel.close_time));
        let xmp = GalaxyXmp { protocol: "galaxy/v1".into(), network: "xrpl-testnet".into(),
            image_profile: match profile { ImageProfile::PngV1 => "png-v1", ImageProfile::JpegV1 => "jpeg-v1" }.into(),
            photo_id, capture_id, manifest_tx: capture.tx.hash, proof_tx: submitted.tx.hash };
        let packet = xmp.to_packet()?;
        let image = &evidence.image;
        let name = match profile { ImageProfile::PngV1 => "development.png", ImageProfile::JpegV1 => "development.jpg" };
        write_export(&dir, name, &image.encoded, &packet, &photo_id)?;
        if profile == ImageProfile::JpegV1 {
            let lossless = galaxy_image::encode(ImageProfile::PngV1, &image.rgb)?;
            write_export(&dir, "development-jpeg-raster.png", &lossless, &packet, &photo_id)?;
        }
        report["export"] = json!(name);
    }
    dir.write("run.json", &serde_json::to_vec_pretty(&report)?)?;
    output(report);
    Ok(())
}

async fn expired_experiment(session: &Session<'_>, proof: &PendingTx, deadline: u32) -> Result<Value> {
    let end = deadline.checked_add(1).ok_or_else(|| error("expired proof ledger overflow"))?;
    wait_for(session, end).await?;
    if !matches!(session.submitter.state(proof).await?, TxState::Expired) {
        return Err(error("expired test blob was not proven absent from its signed validation window"));
    }
    // Bypass the regular submit guard only for an explicitly requested
    // Testnet experiment using a persisted, already expired signed blob.
    let response = session.rpc.request("submit", json!({"tx_blob": hex::encode_upper(&proof.blob)})).await?;
    if response["engine_result"] != "tefMAX_LEDGER" {
        return Err(error("expected tefMAX_LEDGER for a PROOF submitted after its deadline"));
    }
    if session.rpc.tx(&proof.hash).await?.is_some_and(|tx| tx.validated) {
        return Err(error("expired PROOF unexpectedly validated"));
    }
    Ok(json!({"tx": hex::encode_upper(proof.hash), "engine_result": "tefMAX_LEDGER", "last_ledger_sequence": proof.last_ledger, "attempted_after_ledger": deadline}))
}

fn write_export(dir: &storage::PrivateDir, filename: &str, encoded: &[u8], packet: &[u8], expected_photo_id: &Hash32) -> Result<()> {
    let output = galaxy_image::embed_xmp_packet(encoded, packet)?;
    if output.len() > MAX_IMAGE_BYTES || galaxy_image::decode_any(&output)?.photo_id != *expected_photo_id {
        return Err(error("XMP export changed the committed raster"));
    }
    dir.write(filename, &output)
}
