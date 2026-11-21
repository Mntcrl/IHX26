//! End-to-end verifier cases over recorded XRPL answers built from real signed
//! transactions. Expected states follow spec §13; the credential is the
//! development unattested class, so a fully valid pipeline is UNSUPPORTED.

mod support;

use galaxy_image::ImageProfile;
use galaxy_protocol::{
    ids,
    wire::{AttestationEnv, DeviceBundle, DeviceKind, MemoKind},
};
use galaxy_verifier::{
    recorded::Recording, verify_by_photo_id, verify_manifest, verify_photo, verify_proof_only,
    verify_proof_transaction, Refs, Report, State, Status, CHECKS,
};
use galaxy_xrpl::keys::Wallet;
use serde_json::json;
use support::*;

fn status(report: &Report, id: &str) -> Status {
    report.check(id).unwrap_or_else(|| panic!("check {id} missing")).status
}

fn assert_check(report: &Report, id: &str, expected: Status) {
    assert_eq!(status(report, id), expected, "{id}: {}\n{report}", report.check(id).unwrap().detail);
}

/// Image with embedded XMP references to `capture` and `proof_tx`.
fn certified_png(world: &World, rgb: &[u8], capture: &CaptureInfo, proof_tx: [u8; 32]) -> Vec<u8> {
    galaxy_image::embed_xmp(&png(rgb), &world.xmp(capture, proof_tx)).expect("xmp")
}

struct Case {
    world: World,
    rgb: Vec<u8>,
    capture: CaptureInfo,
    proof_tx: [u8; 32],
}

fn valid_case() -> Case {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    let proof_tx = world.publish_proof(&capture, PROOF_LEDGER);
    Case { world, rgb, capture, proof_tx }
}

impl Case {
    async fn direct(&self) -> Report {
        let ledger = self.world.chain.ledger();
        let image = certified_png(&self.world, &self.rgb, &self.capture, self.proof_tx);
        verify_photo(&image, None, &self.world.ctx(&ledger)).await
    }
}

#[tokio::test]
async fn dev_unattested_full_pipeline_is_unsupported_with_every_other_check_passing() {
    let case = valid_case();
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert_eq!(report.exit_code(), 60);
    assert!(report.summary.contains("development unattested credential"), "{}", report.summary);
    assert!(report.summary.contains("Proof status VERIFIED"), "{}", report.summary);
    for (id, _) in CHECKS {
        let expected = match *id {
            "registration.attestation" => Status::Fail,
            "capture.counter" | "evidence.bluetooth" | "evidence.noise" => Status::NotApplicable,
            _ => Status::Pass,
        };
        assert_check(&report, id, expected);
    }
    let capture = report.capture.as_ref().unwrap();
    assert_eq!(capture.attestation, Some("dev-unattested"));
    assert_eq!((capture.l0, capture.lpre), (L0, LPRE));
    let proof = report.proof.as_ref().unwrap();
    assert_eq!(proof.proof_tx.as_deref(), Some(hex::encode_upper(case.proof_tx).as_str()));
    assert_eq!((proof.lsel, proof.lproof, proof.deadline), (Some(LSEL), Some(PROOF_LEDGER), Some(DEADLINE)));
    assert_eq!(proof.policy_name.as_deref(), Some("galaxy-pc-v1m"));
    assert_eq!(proof.ceremony.as_deref(), Some("development-single-party"));
    assert_eq!(report.trust.latest_validated, Some(TIP));

    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["version"], "galaxy-report/v1");
    assert_eq!(value["state"], "UNSUPPORTED");
    assert_eq!(value["capture"]["L0"], L0);
    assert_eq!(value["proof"]["Lsel"], LSEL);
    assert_eq!(value["trust"]["rpc_trust"], "remote");
    let proven: Vec<_> = value["checks"].as_array().unwrap().iter()
        .filter(|c| c["proven_by"] == "zk-circuit").map(|c| c["status"].clone()).collect();
    assert_eq!(proven, vec![json!("pass"); 5]);
    assert!(report.limitations.iter().any(|l| l.contains("not proof that a scene")));
    assert!(report.to_string().starts_with("UNSUPPORTED: "));
}

#[tokio::test]
async fn recording_round_trips_through_json_and_pagination() {
    let mut case = valid_case();
    let json = case.world.chain.recording().to_json();
    let replay = Recording::from_json(json.as_bytes()).unwrap().into_ledger();
    let image = certified_png(&case.world, &case.rgb, &case.capture, case.proof_tx);
    let report = verify_photo(&image, None, &case.world.ctx(&replay)).await;
    assert_eq!(report.state, State::Unsupported);
    assert_check(&report, "proof.groth16", Status::Pass);

    case.world.chain.page_size = Some(1);
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert_check(&report, "proof.groth16", Status::Pass);
}

#[tokio::test]
async fn local_circuit_copy_must_match_the_configured_digests() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let image = certified_png(&case.world, &case.rgb, &case.capture, case.proof_tx);
    let bundle = case.world.circuit.encode();
    let mut ctx = case.world.ctx(&ledger);
    ctx.local_circuit = Some(&bundle);
    let report = verify_photo(&image, None, &ctx).await;
    assert_check(&report, "proof.groth16", Status::Pass);
    assert!(report.check("circuit.registration").unwrap().detail.contains("local-digest-pinned"));

    let mut altered = case.world.circuit.clone();
    altered.vk_bytes[100] ^= 1;
    let altered = altered.encode();
    ctx.local_circuit = Some(&altered);
    let report = verify_photo(&image, None, &ctx).await;
    assert_eq!(report.state, State::Invalid);
    assert_check(&report, "circuit.registration", Status::Fail);
}

#[tokio::test]
async fn metadata_removed_requires_manual_reference_or_lookup() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);
    let bare = png(&case.rgb);
    let report = verify_photo(&bare, None, &ctx).await;
    assert_eq!(report.state, State::NotFound);
    assert_check(&report, "refs.metadata", Status::NotApplicable);
    assert!(report.trust.ledger_ranges.is_empty());

    let manual = verify_photo(&bare, Some(Refs { manifest_tx: case.capture.hash, proof_tx: None }), &ctx).await;
    assert_eq!((manual.mode, manual.state), ("manual", State::Unsupported));
    assert_check(&manual, "proof.groth16", Status::Pass);

    let node = StaticNode::new(vec![Refs { manifest_tx: case.capture.hash, proof_tx: Some(case.proof_tx) }], INCEPTION, TIP);
    let lookup = verify_by_photo_id(&bare, &[node], &ctx).await;
    assert_eq!((lookup.mode, lookup.state), ("lookup", State::Unsupported), "{lookup}");
    assert_eq!(lookup.candidates.len(), 1);
    assert_eq!(lookup.trust.lookup_endpoints, vec!["https://node.test".to_owned()]);
}

#[tokio::test]
async fn metadata_substituted_to_another_capture_is_invalid_in_direct_mode() {
    let mut case = valid_case();
    let other_rgb = raster(2);
    let other = case.world.publish_capture(case.world.core(&other_rgb, 2), L0 + 1);
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);
    let mut xmp = case.world.xmp(&case.capture, case.proof_tx);
    xmp.manifest_tx = other.hash;
    let image = galaxy_image::embed_xmp(&png(&case.rgb), &xmp).unwrap();
    let report = verify_photo(&image, None, &ctx).await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "capture.photo_binding", Status::Fail);
    assert_check(&report, "capture.rgb_binding", Status::Fail);

    // The same mismatching reference from an untrusted node is a rejected hint, never INVALID.
    let node = StaticNode::new(vec![Refs { manifest_tx: other.hash, proof_tx: None }], INCEPTION, TIP);
    let report = verify_by_photo_id(&png(&case.rgb), &[node], &ctx).await;
    assert_eq!(report.state, State::NotFound, "{report}");
    assert_eq!(report.candidates[0].state, State::NotFound);
    assert!(report.candidates[0].summary.starts_with("Rejected hint"));

    // A binding candidate next to a rejected hint decides the aggregate.
    let node = StaticNode::new(vec![
        Refs { manifest_tx: other.hash, proof_tx: None },
        Refs { manifest_tx: case.capture.hash, proof_tx: None },
    ], INCEPTION, TIP);
    let report = verify_by_photo_id(&png(&case.rgb), &[node], &ctx).await;
    assert_eq!(report.state, State::Unsupported);
    assert_eq!(report.candidates.len(), 2);
}

#[tokio::test]
async fn one_pixel_change_breaks_the_photo_binding() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let mut rgb = case.rgb.clone();
    rgb[3 * (1024 * 400 + 512)] ^= 1;
    let image = certified_png(&case.world, &rgb, &case.capture, case.proof_tx);
    let report = verify_photo(&image, None, &case.world.ctx(&ledger)).await;
    assert_eq!(report.state, State::Invalid);
    assert_check(&report, "capture.photo_binding", Status::Fail);
    assert_check(&report, "image.photo_id", Status::Pass);
}

#[tokio::test]
async fn crop_resize_and_recompression() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);

    // Cropped / resized: not the canonical 1024×768 profile.
    let mut cropped = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut cropped, 512, 384);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&case.rgb[..512 * 384 * 3]).unwrap();
    }
    let report = verify_photo(&cropped, Some(Refs { manifest_tx: case.capture.hash, proof_tx: None }), &ctx).await;
    assert_eq!(report.state, State::Unsupported);
    assert_check(&report, "image.profile", Status::Fail);
    assert!(report.capture.is_none());

    // Lossy recompression of the PNG capture into JPEG changes the pixels.
    let jpeg = galaxy_image::encode(ImageProfile::JpegV1, &case.rgb).unwrap();
    let mut xmp = case.world.xmp(&case.capture, case.proof_tx);
    xmp.image_profile = "jpeg-v1".into();
    let jpeg = galaxy_image::embed_xmp(&jpeg, &xmp).unwrap();
    let report = verify_photo(&jpeg, None, &ctx).await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "capture.photo_binding", Status::Fail);
    assert_eq!(report.image.as_ref().unwrap().format, "jpeg");
}

#[tokio::test]
async fn unregistered_image_is_not_found_never_invalid() {
    let mut case = valid_case();
    let unknown = [0xab; 32];
    case.world.chain.missing(unknown);
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);
    let image = png(&raster(9));
    let node = StaticNode::new(Vec::new(), INCEPTION, TIP);
    let report = verify_by_photo_id(&image, &[node], &ctx).await;
    assert_eq!(report.state, State::NotFound, "{report}");
    assert!(report.trust.ledger_ranges.iter().any(|r| r.min == INCEPTION && r.max == TIP));
    assert!(!report.to_string().to_lowercase().contains("fake"));

    let report = verify_photo(&image, Some(Refs { manifest_tx: unknown, proof_tx: None }), &ctx).await;
    assert_eq!(report.state, State::NotFound, "{report}");
    assert_check(&report, "capture.found", Status::NotApplicable);

    // Without complete history, absence is not established.
    case.world.chain.complete = Some(format!("{}-{}", INCEPTION + 50, TIP));
    let ledger = case.world.chain.ledger();
    let report = verify_photo(&image, Some(Refs { manifest_tx: unknown, proof_tx: None }), &case.world.ctx(&ledger)).await;
    assert_eq!(report.state, State::Unavailable, "{report}");

    // A failing or incomplete lookup is UNAVAILABLE, not NOT_FOUND.
    let ledger = case.world.chain.ledger();
    let failing = StaticNode { url: "https://down.test".into(), answer: Err("timeout".into()) };
    let report = verify_by_photo_id(&image, &[failing], &case.world.ctx(&ledger)).await;
    assert_eq!(report.state, State::Unavailable);
}

#[tokio::test]
async fn invalid_groth16_proof_is_invalid() {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    let mut wrong = world.public_inputs(&capture);
    wrong[6] += galaxy_crypto::Fr::from(1u8);
    let memo = world.proof_memo(&capture, prove(&wrong));
    let proof_tx = world.add_device_tx(MemoKind::Proof, &memo, PROOF_LEDGER);
    let case = Case { world, rgb, capture, proof_tx };
    let report = case.direct().await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_eq!(report.exit_code(), 20);
    assert_check(&report, "proof.groth16", Status::Fail);
    for id in ["policy.sensor_quality", "policy.motion_imu"] {
        assert_ne!(status(&report, id), Status::Pass);
    }
}

#[tokio::test]
async fn first_valid_proof_in_ledger_order_wins_over_later_invalid_and_earlier_rejected() {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    // Too early (at Lsel): rejected, then a valid one, then an invalid duplicate.
    let early = world.proof_memo(&capture, prove(&world.public_inputs(&capture)));
    world.add_device_tx(MemoKind::Proof, &early, LSEL);
    let valid = world.publish_proof(&capture, PROOF_LEDGER);
    let mut wrong = world.public_inputs(&capture);
    wrong[0] += galaxy_crypto::Fr::from(1u8);
    let memo = world.proof_memo(&capture, prove(&wrong));
    world.add_device_tx(MemoKind::Proof, &memo, PROOF_LEDGER + 1);
    let case = Case { world, rgb, capture, proof_tx: valid };
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert_check(&report, "proof.groth16", Status::Pass);
    assert_eq!(report.proof.as_ref().unwrap().lproof, Some(PROOF_LEDGER));
    assert_eq!(report.candidates.len(), 2, "evaluation stops at the first valid proof");
    assert_eq!(report.candidates[0].state, State::NotFound);
}

#[tokio::test]
async fn unrecognized_circuit_is_unsupported() {
    let mut case = valid_case();
    case.world.config.accepted_circuits.clear();
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported);
    assert_check(&report, "circuit.accepted", Status::Fail);
    assert_check(&report, "proof.groth16", Status::Skipped);
}

#[tokio::test]
async fn proof_outside_window_leaves_the_capture_without_accepted_proof() {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    let late = world.publish_proof(&capture, DEADLINE + 1);
    let case = Case { world, rgb, capture, proof_tx: late };
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert!(report.summary.contains("Proof status INCONCLUSIVE"), "{}", report.summary);
    assert_check(&report, "proof.window", Status::Fail);
    assert_check(&report, "proof.groth16", Status::Skipped);
}

#[tokio::test]
async fn proof_from_another_account_is_not_used() {
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    let memo = world.proof_memo(&capture, prove(&world.public_inputs(&capture)));
    let stranger = Wallet::from_entropy(&[0x55; 16]);
    let proof_tx = world.chain.add(&stranger, MemoKind::Proof.as_str(), &memo, PROOF_LEDGER);
    let case = Case { world, rgb, capture, proof_tx };
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported);
    assert!(report.summary.contains("Proof status INCONCLUSIVE"), "{}", report.summary);
    let advertised = &report.candidates[0];
    assert_eq!(advertised.proof_tx.as_deref(), Some(hex::encode_upper(proof_tx).as_str()));
    assert!(advertised.checks.iter().any(|c| c.id == "proof.account" && c.status == Status::Fail));
}

#[tokio::test]
async fn replayed_capture_id_is_invalid() {
    let mut world = World::new();
    let rgb = raster(1);
    let core = world.core(&rgb, 1);
    let first = world.publish_capture(core.clone(), L0);
    let replay = world.publish_capture(core, L0 + 1);
    let ledger = world.chain.ledger();
    let ctx = world.ctx(&ledger);
    let report = verify_photo(&png(&rgb), Some(Refs { manifest_tx: replay.hash, proof_tx: None }), &ctx).await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "capture.unique", Status::Fail);
    let report = verify_photo(&png(&rgb), Some(Refs { manifest_tx: first.hash, proof_tx: None }), &ctx).await;
    assert_check(&report, "capture.unique", Status::Pass);
}

#[tokio::test]
async fn revocation_rules() {
    async fn run(revoke_at: u32, effective: u32) -> Report {
        let mut case = valid_case();
        case.world.revoke(revoke_at, effective);
        case.direct().await
    }
    // Revoked before the capture.
    let report = run(L0 - 1, 0).await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "registration.revocation", Status::Fail);
    assert!(report.check("registration.revocation").unwrap().detail.contains(&format!("as of ledger {TIP}")));
    // Backdated revocation published later withdraws the capture.
    let report = run(1100, L0 - 2).await;
    assert_eq!(report.state, State::Invalid);
    // Revocation effective after the capture does not affect it.
    let report = run(1100, 0).await;
    assert_check(&report, "registration.revocation", Status::Pass);
    // Future-dated revocation is ignored and reported.
    let report = run(1100, 1200).await;
    assert_check(&report, "registration.revocation", Status::Pass);
    assert!(report.check("registration.revocation").unwrap().detail.contains("ignored"));
}

#[tokio::test]
async fn unvalidated_and_failed_captures() {
    let mut case = valid_case();
    case.world.chain.tx_mut(&case.capture.hash).validated = false;
    let report = case.direct().await;
    assert_eq!(report.state, State::Pending, "{report}");
    assert_eq!(report.exit_code(), 10);
    assert_check(&report, "capture.validated_success", Status::Unavailable);

    let mut case = valid_case();
    case.world.chain.tx_mut(&case.capture.hash).result = "tecNO_PERMISSION";
    let report = case.direct().await;
    assert_eq!(report.state, State::Invalid);
    assert_check(&report, "capture.validated_success", Status::Fail);

    // A failed PROOF transaction is listed and never accepted.
    let mut case = valid_case();
    case.world.chain.tx_mut(&case.proof_tx).result = "tecNO_PERMISSION";
    let report = case.direct().await;
    assert!(report.summary.contains("Proof status INCONCLUSIVE"), "{}", report.summary);
}

#[tokio::test]
async fn rpc_blob_hash_mismatch_is_unavailable() {
    let mut case = valid_case();
    let other = case.world.chain.txs.iter().find(|t| t.hash == case.proof_tx).unwrap().clone();
    let forged = json!({
        "hash": hex::encode_upper(case.capture.hash), "tx_blob": hex::encode_upper(&other.blob),
        "meta_blob": hex::encode_upper(galaxy_xrpl::codec::encode(&json!({
            "AffectedNodes": [], "TransactionIndex": 0, "TransactionResult": "tesSUCCESS",
        })).unwrap()),
        "ledger_index": L0, "validated": true,
    });
    case.world.chain.overrides.push((case.capture.hash, forged));
    let report = case.direct().await;
    assert_eq!(report.state, State::Unavailable, "{report}");
    assert_eq!(report.exit_code(), 50);
    assert_check(&report, "capture.found", Status::Unavailable);
}

#[tokio::test]
async fn network_mismatch() {
    let mut case = valid_case();
    case.world.chain.network_id = 2;
    let report = case.direct().await;
    assert_eq!(report.state, State::Unavailable);
    assert_check(&report, "ledger.network", Status::Unavailable);

    let mut world = World::new();
    let rgb = raster(1);
    let mut core = world.core(&rgb, 1);
    core.network = 2;
    let capture = world.publish_capture(core, L0);
    let case = Case { world, rgb, capture, proof_tx: [0; 32] };
    let report = case.direct().await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "capture.network", Status::Fail);
}

#[tokio::test]
async fn pending_while_the_window_is_open() {
    // Selection ledger not validated yet.
    let mut world = World::new();
    let rgb = raster(1);
    let capture = world.publish_capture(world.core(&rgb, 1), L0);
    world.chain.tip = LSEL - 1;
    let case = Case { world, rgb, capture, proof_tx: [0; 32] };
    let report = case.direct().await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert!(report.summary.contains("Proof status PENDING"), "{}", report.summary);
    assert_check(&report, "ledger.selection", Status::Unavailable);

    // Selection validated, no proof yet, window open.
    let mut case = case;
    case.world.chain.tip = LSEL + 10;
    let report = case.direct().await;
    assert!(report.summary.contains("Proof status PENDING"), "{}", report.summary);
    assert_check(&report, "proof.found", Status::Unavailable);
    assert_check(&report, "ledger.selection", Status::Pass);
}

#[tokio::test]
async fn proof_only_and_exact_proof_modes() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);
    let report = verify_proof_only(case.capture.hash, &ctx).await;
    assert_eq!((report.mode, report.state), ("proof-only", State::Unsupported));
    assert_check(&report, "image.photo_id", Status::NotApplicable);
    assert_check(&report, "proof.groth16", Status::Pass);
    assert!(report.limitations.iter().any(|l| l.starts_with("Image binding not checked")));
    assert_eq!(report.to_string(), verify_manifest(None, case.capture.hash, &ctx).await.to_string());

    let report = verify_proof_transaction(case.proof_tx, &ctx).await;
    assert_eq!(report.state, State::Unsupported, "{report}");
    assert_check(&report, "proof.groth16", Status::Pass);
}

#[tokio::test]
async fn apple_attestation_bound_to_another_registration_is_invalid() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test-vectors/attestation/device-development.json"
    ))
    .unwrap();
    let mut world = World::new();
    let device = Wallet::from_entropy(&[0x66; 16]);
    let mut chain = world.chain.clone();
    chain.queries.clear();
    let bundle = DeviceBundle {
        kind: DeviceKind::AppleAppAttest, env: AttestationEnv::Development, app_id: APP_ID.into(),
        key_id: hex::decode(vector["key_id_hex"].as_str().unwrap()).unwrap().try_into().unwrap(),
        network: NETWORK, reg_ledger_index: REG_LEDGER, reg_ledger_hash: ledger_hash(REG_LEDGER), reg_nonce: [0x44; 32],
        account_id: device.account_id(),
        attestation: hex::decode(vector["attestation_cbor_hex"].as_str().unwrap()).unwrap(),
    };
    let registration_tx = register_device(&mut chain, &device, &bundle);
    chain.queries.extend(world.chain.queries.iter().filter(|q| q.0 != world.device.account_id()));
    world.chain = chain;
    world.device = device;
    world.registration_tx = registration_tx;
    let rgb = raster(1);
    let mut core = world.core(&rgb, 1);
    core.account_id = world.device.account_id();
    let capture = world.publish_capture(core, L0);
    let case = Case { world, rgb, capture, proof_tx: [0; 32] };
    let report = case.direct().await;
    assert_eq!(report.state, State::Invalid, "{report}");
    assert_check(&report, "registration.attestation", Status::Fail);
    assert_check(&report, "registration.bundle", Status::Pass);
    assert_eq!(ids::capture_id(&case.capture.core.encode()), case.capture.capture_id);
}

#[tokio::test]
async fn hostile_inputs_do_not_panic() {
    let case = valid_case();
    let ledger = case.world.chain.ledger();
    let ctx = case.world.ctx(&ledger);
    for bytes in [&b""[..], b"\x89PNG\r\n\x1a\n", b"\xff\xd8\xff", &[0u8; 64]] {
        let report = verify_photo(bytes, None, &ctx).await;
        assert!(matches!(report.state, State::Invalid | State::Unsupported), "{report}");
    }
    let mut png_bytes = png(&case.rgb);
    let len = png_bytes.len();
    png_bytes[len / 2] ^= 0xff;
    let report = verify_photo(&png_bytes, None, &ctx).await;
    assert!(matches!(report.state, State::Invalid | State::Unsupported), "{report}");

    let parse = galaxy_verifier::parse_lookup_response;
    let photo = [0xabu8; 32];
    assert!(parse(b"{}", &photo, 1, 1).is_err());
    let mut document = serde_json::json!({
        "protocol":"galaxy-node/v1","network":"xrpl-testnet","network_id":1,"software_version":"0.1.0",
        "photo_id":hex::encode(photo),"indexed_from":1,"indexed_through":2,
        "indexed_ranges":[{"from":1,"through":2}],"incomplete":false,"next_cursor":"page-2",
        "extra":"forward-compatible",
        "captures":[{"capture_id":hex::encode([0xafu8;32]),"manifest_tx":hex::encode_upper([0xabu8;32]),"ledger_index":2,"proof_tx":null}]
    });
    let body = document.to_string();
    let parsed = parse(body.as_bytes(), &photo, 1, 1).unwrap();
    assert_eq!(parsed.result.candidates[0].manifest_tx, [0xab; 32]);
    assert_eq!(parsed.next_cursor.as_deref(), Some("page-2"));
    assert!(parse(body.as_bytes(), &[0; 32], 1, 1).is_err());
    assert!(parse(body.as_bytes(), &photo, 2, 1).is_err());
    document["captures"][0]["manifest_tx"] = hex::encode([0xabu8; 32]).into();
    assert!(parse(document.to_string().as_bytes(), &photo, 1, 1).is_err());
    document["captures"][0]["manifest_tx"] = hex::encode_upper([0xabu8; 32]).into();
    document.as_object_mut().unwrap().remove("next_cursor");
    assert!(parse(document.to_string().as_bytes(), &photo, 1, 1).is_err());
}
