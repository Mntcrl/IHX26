use super::*;
use crate::{
    build_device_manifest, decode_evidence_bundle, device_registration_client_data, ingest_gxrw,
    prepare_device_publication, wallet_identity, xrpl_account_info_request, xrpl_autofill,
    xrpl_latest_validated, xrpl_ledger_request, xrpl_parse_submit, xrpl_parse_tx,
    xrpl_parse_validated_ledger, xrpl_sign_account_set, xrpl_submit_request, xrpl_tx_request,
    xrpl_sign_account_preparation,
    AppAttestEnvironment, CaptureProfile, RegistrationChallenge, SignedTransaction, XrplAutofill,
    XrplNetwork, XrplSubmitClass, XrplTxStatus,
};
use galaxy_evidence::{bundle::EvidenceBundle, Salts};
use galaxy_geometry::synthetic::{synthesize_with_profile, Scenario};
use galaxy_protocol::{
    canonical_json,
    wire::{ChunkMemo, DeviceBundle, DeviceManifest},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const TX: &str = "AA96B924CDD99DEBA0B6ACEC7337FD8C4D537A4BE089253D329B22B764AA49F2";
const SECRET: [u8; 16] = [7; 16]; // Public, deterministic test-only entropy.
const NET: XrplNetwork = XrplNetwork {
    network_id: 1,
    allow_mainnet: false,
    max_fee_drops: 1000,
};
const GRAPH: &[u8] = include_bytes!("../../../test-vectors/prover/fixture.graph.bin");
const ZKEY: &[u8] = include_bytes!("../../../test-vectors/prover/fixture.zkey");
const VK: &[u8] = include_bytes!("../../../test-vectors/prover/vk.bin");

fn test_path(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(path)
}

fn pins_for(descriptor: &[u8], policy_id: Hash32, vk_digest: Hash32) -> String {
    // Local test allowlist, never used by the released binaries.
    let config = include_str!("../../../deployments/networks/xrpl-testnet.toml");
    // Replace only the accepted_circuits field (usually empty, or table array after registration).
    let prefix: String = config
        .lines()
        .take_while(|line| {
            !line.starts_with("accepted_circuits =") && !line.starts_with("[[accepted_circuits]]")
        })
        .map(|line| format!("{line}\n"))
        .collect();
    format!("{prefix}\n[[accepted_circuits]]\ncircuit_id = {:?}\npolicy_id = {:?}\nvk_digest = {:?}\nregistration_tx = {TX:?}\n",
        hex::encode(ids::circuit_id(descriptor).unwrap()), hex::encode(policy_id), hex::encode(vk_digest))
}

fn fixture_descriptor() -> (Value, Hash32, Hash32) {
    let policy: Policy = serde_json::from_slice(BUNDLED_POLICY).unwrap();
    let policy_id = policy.id().unwrap();
    let vk = ids::vk_digest(VK);
    let desc = json!({
        "name": "Test-only tiny Groth16 fixture", "version": 1, "profile": policy.profile,
        "policy_id": hex::encode(policy_id), "witness_graph_sha256": hex::encode(Sha256::digest(GRAPH)),
        "zkey_sha256": hex::encode(Sha256::digest(ZKEY)), "vk_digest": hex::encode(vk),
        "n_public": PUBLIC_COUNT, "public_layout": "galaxy-pc-v1-public-22", "ceremony": "development-single-party"
    });
    (desc, policy_id, vk)
}

fn fixture_circuit() -> Arc<CaptureCircuit> {
    let (descriptor, policy, vk) = fixture_descriptor();
    let bytes = canonical_json::to_vec(&descriptor).unwrap();
    Arc::new(
        CaptureCircuit::from_pins(&bytes, BUNDLED_POLICY, &pins_for(&bytes, policy, vk)).unwrap(),
    )
}

fn restore_synthetic(
    circuit: &CaptureCircuit,
    scene: Scenario,
    profile: galaxy_image::ImageProfile,
) -> Arc<CaptureEvidence> {
    let evidence = synthesize_with_profile(scene, 0, &circuit.policy, profile).unwrap();
    let inner = CaptureEvidence::from_bundle(EvidenceBundle {
        evidence,
        salts: Salts {
            depth0: 1u8.into(),
            frame_minus: 2u8.into(),
            frame_plus: 3u8.into(),
            camera: 4u8.into(),
            imu: 5u8.into(),
            bluetooth: 6u8.into(),
            noise: 7u8.into(),
            meta: 8u8.into(),
        },
        capture_state: b"test journal state".to_vec(),
    }, true)
    .unwrap();
    let restored = decode_evidence_bundle(inner.encode_bundle().unwrap()).unwrap();
    assert_eq!(
        inner.commitment_ids().evidence_root,
        restored.commitment_ids().evidence_root
    );
    assert_eq!(restored.capture_state(), b"test journal state");
    inner
}

fn ledger(index: u32) -> ValidatedLedger {
    ValidatedLedger {
        index,
        hash: vec![0x53; 32],
        close_time: 1_800_000_000,
    }
}

fn statement_result(
    circuit: &CaptureCircuit, evidence: Arc<CaptureEvidence>, case: u8,
) -> Result<CaptureStatement> {
    let flags = u16::from(evidence.profile().code());
    circuit.build_capture_statement(evidence, CaptureParameters {
        network: 1, registration_tx: TX.into(), preledger: ledger(400),
        nonce: vec![case; 32], claimed_time_ms: 1_800_000_000_000, flags,
        account_id: wallet_identity(SECRET.to_vec()).unwrap().account_id,
    })
}

fn statement(circuit: &CaptureCircuit, evidence: Arc<CaptureEvidence>, case: u8) -> CaptureStatement {
    statement_result(circuit, evidence, case).unwrap()
}

fn derive(circuit: Arc<CaptureCircuit>, core: Vec<u8>) -> Arc<CaptureChallenge> {
    let lsel = circuit.selection_ledger_index(core.clone(), 401).unwrap();
    circuit
        .derive_challenge(core, TX.into(), 401, ledger(lsel))
        .unwrap()
}

fn passing(circuit: Arc<CaptureCircuit>) -> Arc<CaptureEvaluation> {
    // Synthetic test only: try a fixed finite sequence of independent vectors.
    for case in 0..8 {
        let evidence = restore_synthetic(&circuit, Scenario::Room, galaxy_image::ImageProfile::PngV1);
        let s = statement(&circuit, Arc::clone(&evidence), case);
        let ch = derive(Arc::clone(&circuit), s.core);
        let eval = ch.evaluate(Arc::clone(&evidence)).unwrap();
        if eval.verdict().pass {
            return eval;
        }
    }
    panic!("test room did not pass any synthetic vector under the tested policy");
}

fn apple_vector() -> Value {
    serde_json::from_str(include_str!(
        "../../../test-vectors/attestation/device-development.json"
    ))
    .unwrap()
}

fn apple_assertion() -> Vec<u8> {
    // The published vector's signature is for a placeholder core; these tests only
    // exercise structural memo encoding. No verifier or network submission is implied.
    hex::decode(
        apple_vector()["assertions"][0]["assertion_cbor_hex"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

fn rpc(result: Value) -> String {
    json!({"result": result}).to_string()
}

#[test]
fn pins_require_all_accepted_digests_and_development_network() {
    let (descriptor, policy_id, vk) = fixture_descriptor();
    let bytes = canonical_json::to_vec(&descriptor).unwrap();
    let net = pins_for(&bytes, policy_id, vk);
    assert!(CaptureCircuit::from_pins(&bytes, BUNDLED_POLICY, &net).is_ok());
    for bad in [
        pins_for(&bytes, [1; 32], vk),
        pins_for(&bytes, policy_id, [2; 32]),
    ] {
        assert!(matches!(
            CaptureCircuit::from_pins(&bytes, BUNDLED_POLICY, &bad),
            Err(GalaxyError::Circuit)
        ));
    }
    let mainnet = net
        .replace("name = \"xrpl-testnet\"", "name = \"xrpl-mainnet\"")
        .replace("network_id = 1", "network_id = 0")
        .replace("allow_mainnet = false", "allow_mainnet = true");
    assert!(matches!(
        CaptureCircuit::from_pins(&bytes, BUNDLED_POLICY, &mainnet),
        Err(GalaxyError::Circuit)
    ));
    let mut altered = bytes.clone();
    altered.push(b'\n');
    assert!(matches!(
        CaptureCircuit::from_pins(&altered, BUNDLED_POLICY, &net),
        Err(GalaxyError::Circuit)
    ));
    let mut policy: Policy = serde_json::from_slice(BUNDLED_POLICY).unwrap();
    policy.windows.w_cap += 1;
    assert!(matches!(
        CaptureCircuit::from_pins(&bytes, &policy.canonical_json().unwrap(), &net),
        Err(GalaxyError::Circuit)
    ));
    assert!(matches!(
        CaptureCircuit::configure(bytes),
        Err(GalaxyError::Circuit)
    ));
}

#[test]
fn capture_core_challenge_and_witness_are_bound() {
    let circuit = fixture_circuit();
    let evidence = restore_synthetic(&circuit, Scenario::Room, galaxy_image::ImageProfile::PngV1);
    let s = statement(&circuit, Arc::clone(&evidence), 1);
    assert!(matches!(statement_result(&circuit, Arc::clone(&evidence), 2), Err(GalaxyError::CaptureBinding)));
    let restored = decode_evidence_bundle(evidence.encode_bundle().unwrap()).unwrap();
    assert!(matches!(statement_result(&circuit, Arc::clone(&restored), 2), Err(GalaxyError::CaptureBinding)));
    assert!(derive(Arc::clone(&circuit), s.core.clone()).evaluate(restored).is_ok());
    let core = CaptureCore::decode(&s.core).unwrap();
    let limit = circuit.policy_limits();
    assert_eq!(s.core.len(), 299);
    assert_eq!(s.capture_id, ids::capture_id(&s.core));
    assert_eq!(
        s.assertion_client_data_hash,
        ids::assertion_client_data_hash(&s.core)
    );
    assert_eq!(
        s.capture_deadline,
        core.preledger_index + limit.capture_window
    );
    let memo = build_capture_memo(s.core.clone(), apple_assertion()).unwrap();
    assert_eq!(CaptureMemo::decode(&memo.data).unwrap().core, core);
    assert!(matches!(
        build_capture_memo(s.core.clone(), vec![0]),
        Err(GalaxyError::Attestation)
    ));
    assert!(matches!(
        circuit.selection_ledger_index(s.core.clone(), 400),
        Err(GalaxyError::LedgerWindow)
    ));
    assert!(matches!(
        circuit.selection_ledger_index(s.core.clone(), s.capture_deadline + 1),
        Err(GalaxyError::LedgerWindow)
    ));
    assert!(matches!(
        Arc::clone(&circuit).derive_challenge(s.core.clone(), TX.into(), 401, ledger(401)),
        Err(GalaxyError::LedgerWindow)
    ));
    let ch = derive(Arc::clone(&circuit), s.core);
    let expected = Challenge::derive(
        core.network,
        &ch.capture_id,
        ch.selection_index,
        &ch.selection_hash,
    );
    assert_eq!(ch.info().seed, expected.seed);
    assert_eq!(ch.info().sampled_tiles, expected.tiles);
    assert_eq!(
        ch.info().proof_deadline,
        ch.selection_index + limit.proof_window
    );
    assert_eq!(ch.public_inputs().len(), PUBLIC_COUNT);
    assert_eq!(ch.public_inputs()[0], core.rgb_root);
    assert_eq!(ch.public_inputs()[1], core.evidence_root);

    let passing = passing(Arc::clone(&circuit));
    let witness = passing.build_witness().unwrap();
    let json: Value = serde_json::from_str(&witness.json).unwrap();
    let expected = witness
        .challenge
        .public_inputs
        .map(|f| galaxy_crypto::field::fr_to_dec(&f));
    // Compare only the public statement, never print the witness.
    for (i, name) in [
        "rgb_root",
        "evidence_root",
        "capture_id_hi",
        "capture_id_lo",
        "seed_hi",
        "seed_lo",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(json[name].as_str() == Some(expected[i].as_str()));
    }
    assert!(json["sampled_tiles"] == json!(expected[6..]));

    let other = restore_synthetic(
        &circuit,
        Scenario::FlatWall,
        galaxy_image::ImageProfile::PngV1,
    );
    assert!(matches!(
        Arc::clone(&ch).evaluate(Arc::clone(&other)),
        Err(GalaxyError::CaptureBinding)
    ));
    let bad = derive(
        Arc::clone(&circuit),
        statement(&circuit, Arc::clone(&other), 1).core,
    )
    .evaluate(other)
    .unwrap();
    assert!(!bad.verdict().pass);
    assert!(matches!(
        bad.build_witness(),
        Err(GalaxyError::Inconclusive)
    ));
}

#[test]
fn embeds_png_and_jpeg_without_changing_canonical_pixels() {
    let circuit = fixture_circuit();
    for profile in [
        galaxy_image::ImageProfile::PngV1,
        galaxy_image::ImageProfile::JpegV1,
    ] {
        let evidence = restore_synthetic(&circuit, Scenario::Room, profile);
        let original = evidence.canonical_image();
        let s = statement(&circuit, Arc::clone(&evidence), 1);
        let ch = derive(Arc::clone(&circuit), s.core);
        let export = ch.export_image(evidence, TX.into()).unwrap();
        let a = galaxy_image::decode_any(&original).unwrap();
        let b = galaxy_image::decode_any(&export).unwrap();
        assert!(a.raster == b.raster);
        let xmp = b.xmp.unwrap().model;
        assert_eq!(xmp.manifest_tx, hash_hex(TX).unwrap());
        assert_eq!(xmp.proof_tx, hash_hex(TX).unwrap());
        assert_eq!(xmp.capture_id, ch.capture_id);
        assert_eq!(
            xmp.image_profile,
            xmp::IMAGE_PROFILES[usize::from(profile.code())]
        );
    }
}

#[test]
fn real_gxrw_uses_strict_ingest_and_resumable_bundle() {
    let path = test_path("fixtures/real/calibration/classes/genuine-3d");
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: local private calibration directory absent");
            return;
        }
        Err(_) => panic!("could not read local calibration directory"),
    };
    let Some(fixture) = entries
        .map(|e| e.expect("calibration entry").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "gxrw"))
        .min()
    else {
        eprintln!("skipped: no local private calibration capture");
        return;
    };
    let raw = fs::read(fixture).expect("read local calibration capture");
    let preview = crate::canonical_image_from_gxrw(raw.clone(), CaptureProfile::PngV1).unwrap();
    let evidence = ingest_gxrw(
        raw,
        CaptureProfile::PngV1,
        vec![],
    )
    .unwrap();
    let bundle = evidence.encode_bundle().unwrap();
    let restored = decode_evidence_bundle(bundle).unwrap();
    let original = evidence.commitment_ids();
    let recovered = restored.commitment_ids();
    assert_eq!(original.photo_id, recovered.photo_id);
    assert_eq!(original.rgb_root, recovered.rgb_root);
    assert_eq!(original.evidence_root, recovered.evidence_root);
    assert!(evidence.capture_state().is_empty());
    assert!(galaxy_image::decode_any(&preview).unwrap().xmp.is_none());
    assert!(preview == evidence.canonical_image());
    assert!(preview == restored.canonical_image());
    assert!(matches!(
        ingest_gxrw(Vec::new(), CaptureProfile::PngV1, vec![]),
        Err(GalaxyError::Evidence)
    ));
    assert!(matches!(
        crate::canonical_image_from_gxrw(Vec::new(), CaptureProfile::PngV1),
        Err(GalaxyError::Evidence)
    ));
    assert!(matches!(
        decode_evidence_bundle(vec![0]),
        Err(GalaxyError::EvidenceBundle)
    ));
}

#[test]
fn chunks_and_manifest_match_published_app_attest_material() {
    let v = apple_vector();
    let binding = hex::decode(v["registration_client_data_hex"].as_str().unwrap()).unwrap();
    let fields = binding.strip_prefix(b"GALAXY/DEVICE-REG/v1\0").unwrap();
    let challenge = RegistrationChallenge {
        network: u32::from_be_bytes(fields[..4].try_into().unwrap()),
        ledger: ValidatedLedger {
            index: u32::from_be_bytes(fields[4..8].try_into().unwrap()),
            hash: fields[8..40].to_vec(),
            close_time: 0,
        },
        nonce: fields[40..72].to_vec(),
        key_id: fields[72..104].to_vec(),
        account_id: fields[104..124].to_vec(),
    };
    let data = device_registration_client_data(challenge.clone()).unwrap();
    assert_eq!(data.client_data_hash, Sha256::digest(binding).as_slice());
    let attestation = hex::decode(v["attestation_cbor_hex"].as_str().unwrap()).unwrap();
    let app_id = v["app_id"].as_str().unwrap();
    let publication = prepare_device_publication(
        challenge.clone(),
        app_id.into(),
        AppAttestEnvironment::Development,
        attestation,
    )
    .unwrap();
    let mut reconstructed = Vec::new();
    for (i, memo) in publication.chunks.iter().enumerate() {
        assert_eq!(memo.memo_type, MemoKind::DeviceChunk.as_str());
        let chunk = ChunkMemo::decode(&memo.data).unwrap();
        assert_eq!(chunk.index as usize, i);
        assert_eq!(chunk.bundle_id.as_slice(), publication.bundle_id);
        reconstructed.extend(chunk.payload);
    }
    assert!(publication.bundle == reconstructed);
    assert_eq!(publication.deadline, data.deadline);
    let mut included = vec![challenge.ledger.index + 1; publication.chunks.len()];
    let device_memo = build_device_manifest(publication.bundle, included.clone()).unwrap();
    assert_eq!(device_memo.memo_type, MemoKind::Device.as_str());
    let manifest = DeviceManifest::decode(&device_memo.data).unwrap();
    let bundle = DeviceBundle::decode(&reconstructed).unwrap();
    galaxy_attestation::verify_registration(
        &bundle,
        &manifest,
        v["verify_at_unix_seconds"].as_u64().unwrap(),
        &galaxy_attestation::AcceptedAttestation {
            app_ids: vec![app_id.into()],
            environments: vec![galaxy_protocol::wire::AttestationEnv::Development],
        },
    )
    .unwrap();
    included[0] = challenge.ledger.index;
    assert!(matches!(
        build_device_manifest(reconstructed, included),
        Err(GalaxyError::LedgerWindow)
    ));
}

fn validated(record: &SignedTransaction, code: &str) -> String {
    rpc(
        json!({"tx_blob": hex::encode_upper(&record.blob), "meta_blob": code,
        "validated": true, "ledger_index": 403, "hash": record.hash_hex}),
    )
}

#[test]
fn signs_journals_and_evaluates_only_validated_xrpl_success() {
    let wallet = wallet_identity(SECRET.to_vec()).unwrap();
    assert_eq!(wallet.public_key[0], 0xED);
    assert!(matches!(
        wallet_identity(vec![0; 15]),
        Err(GalaxyError::InvalidInput)
    ));
    let info = rpc(
        json!({"status":"success","info":{"network_id":1,"complete_ledgers":"1-500","validated_ledger":{"seq":400}}}),
    );
    assert_eq!(xrpl_latest_validated(NET, info.clone()).unwrap(), 400);
    assert!(matches!(
        xrpl_latest_validated(
            XrplNetwork {
                network_id: 2,
                ..NET
            },
            info
        ),
        Err(GalaxyError::Network)
    ));
    let fee = rpc(json!({"drops":{"open_ledger_fee":"12"}}));
    let account = rpc(
        json!({"validated":true,"ledger_index":400,"account_data":{"Account":wallet.classic_address,"Sequence":9,"AccountTxnID":TX}}),
    );
    assert!(xrpl_account_info_request(wallet.classic_address.clone(), 400).is_ok());
    let filled = xrpl_autofill(NET, 400, Some(410), wallet.classic_address, fee, account).unwrap();
    assert_eq!((filled.sequence, filled.last_ledger_sequence), (9, 410));
    assert_eq!(filled.account_txn_id, Some(hex::decode(TX).unwrap()));
    let prepare = xrpl_sign_account_preparation(SECRET.to_vec(), NET, XrplAutofill {
        account_txn_id: None, ..filled.clone()
    }).unwrap();
    let setup = galaxy_xrpl::codec::decode(&prepare.blob).unwrap();
    assert_eq!(setup["SetFlag"], 5);
    assert!(setup.get("Memos").is_none());
    assert_eq!(prepare.purpose, "account-prepare");
    assert!(xrpl_submit_request(NET, prepare, 400).is_ok());

    let circuit = fixture_circuit();
    let ev = restore_synthetic(&circuit, Scenario::Room, galaxy_image::ImageProfile::PngV1);
    let capture = statement(&circuit, ev, 1);
    let purpose = "capture/test".to_string();
    let memo = || build_capture_memo(capture.core.clone(), apple_assertion()).unwrap();
    let signed =
        xrpl_sign_account_set(SECRET.to_vec(), NET, purpose.clone(), filled.clone(), memo()).unwrap();
    assert_eq!(
        signed.hash_hex,
        hex::encode_upper(galaxy_xrpl::keys::tx_hash(&signed.blob))
    );
    assert!(matches!(
        xrpl_sign_account_set(vec![8; 16], NET, purpose, filled, memo()),
        Err(GalaxyError::CaptureBinding)
    ));
    assert!(xrpl_submit_request(NET, signed.clone(), 400).is_ok());
    let result = xrpl_parse_submit(
        NET,
        signed.clone(),
        rpc(json!({"engine_result":"tesSUCCESS"})),
    )
    .unwrap();
    assert!(matches!(result.result_class, XrplSubmitClass::Success));
    assert!(xrpl_tx_request(NET, signed.clone(), 405).is_ok());
    let not_found = |searched| rpc(json!({"error":"txnNotFound","searched_all":searched}));
    assert!(matches!(
        xrpl_parse_tx(NET, signed.clone(), 405, not_found(false)).unwrap(),
        XrplTxStatus::Pending
    ));
    assert!(matches!(
        xrpl_parse_tx(NET, signed.clone(), 411, not_found(true)).unwrap(),
        XrplTxStatus::Expired
    ));
    assert!(matches!(
        xrpl_parse_tx(
            NET,
            signed.clone(),
            405,
            validated(&signed, "201C00000003F8F1031000")
        )
        .unwrap(),
        XrplTxStatus::Validated {
            ledger_index: 403,
            success: true,
            ..
        }
    ));
    assert!(matches!(
        xrpl_parse_tx(
            NET,
            signed.clone(),
            405,
            validated(&signed, "201C00000003F8F103107D")
        )
        .unwrap(),
        XrplTxStatus::Validated {
            ledger_index: 403,
            success: false,
            ..
        }
    ));
    let mut tampered = signed;
    tampered.blob[10] ^= 1;
    assert!(xrpl_submit_request(NET, tampered, 400).is_err());
}

#[test]
fn ledger_headers_require_exact_index_and_validation() {
    assert!(xrpl_ledger_request(400)
        .unwrap()
        .contains("\"ledger_index\":400"));
    let hash = "53".repeat(32);
    let mut res = json!({"status":"success","validated":true,"ledger_index":400,"ledger_hash":hash,
        "ledger":{"closed":true,"ledger_index":400,"ledger_hash":hash,
            "parent_hash":"42".repeat(32),"transaction_hash":"56".repeat(32),"close_time":10}});
    let got = xrpl_parse_validated_ledger(400, rpc(res.clone())).unwrap();
    assert_eq!((got.index, got.close_time), (400, 946_684_810));
    assert_eq!(got.hash, [0x53; 32]);
    assert!(xrpl_parse_validated_ledger(401, rpc(res.clone())).is_err());
    res["validated"] = json!(false);
    assert!(matches!(
        xrpl_parse_validated_ledger(400, rpc(res)),
        Err(GalaxyError::LedgerWindow)
    ));
}

#[test]
fn pinned_prover_matches_expected_public_statement_and_builds_proof_memo() {
    let circuit = fixture_circuit();
    let p = test_path("test-vectors/prover");
    let graph = p.join("fixture.graph.bin").to_str().unwrap().to_owned();
    let zkey = p.join("fixture.zkey").to_str().unwrap().to_owned();
    assert!(matches!(
        circuit.open_prover(
            graph.clone(),
            p.join("fixture_zkey_new.zkey").to_str().unwrap().into()
        ),
        Err(GalaxyError::Circuit)
    ));
    let prover = circuit.open_prover(graph, zkey).unwrap();
    let ev = restore_synthetic(&circuit, Scenario::Room, galaxy_image::ImageProfile::PngV1);
    let s = statement(&circuit, ev, 1);
    let challenge = derive(circuit, s.core);
    let public = challenge
        .public_inputs
        .map(|f| galaxy_crypto::field::fr_to_dec(&f));
    // Tiny independent test circuit, never an accepted mobile proving artifact.
    let recipe = json!({"pub_in": public, "secret": ["1", "2"]}).to_string();
    let witness = Arc::new(CaptureWitness {
        challenge: Arc::clone(&challenge),
        json: Zeroizing::new(recipe),
    });
    let cancel = ProvingCancellation::new();
    cancel.cancel();
    assert!(matches!(
        prover.prove(Arc::clone(&witness), None, Some(cancel)),
        Err(GalaxyError::Cancelled)
    ));
    let result = prover.prove(witness, None, None).unwrap();
    assert!(prover
        .verify_proof(Arc::clone(&challenge), result.proof.clone())
        .unwrap());
    let memo = prover
        .build_proof_memo(Arc::clone(&challenge), result.proof)
        .unwrap();
    assert_eq!(memo.memo_type, MemoKind::Proof.as_str());
    assert_eq!(memo.data.len(), 361);
    let p = ProofMemo::decode(&memo.data).unwrap();
    assert_eq!(p.manifest_tx, hash_hex(TX).unwrap());
    assert_eq!(p.selection_ledger_hash, challenge.selection_hash);
    assert!(matches!(
        prover.build_proof_memo(challenge, vec![0; 256]),
        Err(GalaxyError::ProofVerification)
    ));
}

#[test]
#[ignore = "requires accepted main_v1m artifacts and a heavy build slot"]
fn accepted_main_v1m_proves_the_real_witness() {
    let dir = std::env::var_os("GALAXY_IOS_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| test_path("circuits/artifacts/v1m"));
    let circuit =
        CaptureCircuit::configure(fs::read(dir.join("descriptor.json")).unwrap()).unwrap();
    let prover = circuit
        .open_prover(
            dir.join("main_v1m.graph.bin").to_str().unwrap().into(),
            dir.join("main_v1m.zkey").to_str().unwrap().into(),
        )
        .unwrap();
    let eval = passing(circuit);
    let out = prover
        .prove(eval.build_witness().unwrap(), None, None)
        .unwrap();
    assert!(prover
        .verify_proof(Arc::clone(&eval.challenge), out.proof.clone())
        .unwrap());
    assert_eq!(
        prover
            .build_proof_memo(Arc::clone(&eval.challenge), out.proof)
            .unwrap()
            .data
            .len(),
        361
    );
    eprintln!(
        "main_v1m: witness {:.3}s, prove {:.3}s",
        out.witness_seconds, out.prove_seconds
    );
}
