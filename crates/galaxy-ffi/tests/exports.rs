use galaxy_ffi::{
    certified_file_with_info, certified_photo_details, lossless_png_from_certified_jpeg,
    CaptureProfile, ExportInfo, GalaxyError,
};
use galaxy_image::{decode_any, embed_xmp, encode, photo_id, ImageProfile};
use galaxy_protocol::{network::NetworkConfig, xmp::GalaxyXmp};
use galaxy_verifier::{recorded::Recording, verify_photo, Report, State, Status, VerifyContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const JPEG: &[u8] = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");
const PNG_REFS: &str = include_str!("../../../test-vectors/verifier/dev-unattested/refs.json");
const PNG_RECORDING: &[u8] = include_bytes!("../../../test-vectors/verifier/dev-unattested/recording.json");
const PNG_NETWORK: &str = include_str!("../../../test-vectors/verifier/dev-unattested/network.toml");
const JPEG_RECORDING: &[u8] = include_bytes!("../../../test-vectors/e2e/development-4ad25185-account-txn-id/jpeg-v1.recording.json");
const JPEG_NETWORK: &str = include_str!("../../../test-vectors/e2e/development-4ad25185-account-txn-id/network.toml");

fn info() -> ExportInfo {
    ExportInfo {
        account: Some("rrrrrrrrrrrrrrrrrrrrrhoLvTp".into()),
        registration_tx: Some(hex::encode_upper([0xe3; 32])),
        capture_ledger: Some(u32::MAX),
        selection_ledger: Some(18),
        proof_ledger: Some(21),
        capture_time: Some("2026-11-21T10:57:18Z".into()),
        claimed_time: Some("2026-11-21T10:57:17.123Z".into()),
        circuit_id: Some(hex::encode([0xc5; 32])),
        policy_id: Some(hex::encode([0xa4; 32])),
        policy_name: Some("Public & informative only".into()),
        setup_ceremony: Some("development-single-party".into()),
        attestation_environment: Some("dev-unattested".into()),
        capture_tx_url: None,
        proof_tx_url: None,
        app_version: "1.5.2".into(),
    }
}

fn png_recorded_vector() -> Vec<u8> {
    let refs: Value = serde_json::from_str(PNG_REFS).unwrap();
    let hash = |key: &str| hex::decode(refs[key].as_str().unwrap()).unwrap().try_into().unwrap();
    // The raster(1) recipe from galaxy-verifier/tests/vectors.rs.
    let rgb: Vec<_> = (0..galaxy_image::RGB_LEN)
        .map(|i| ((31 * i + 7 * (i / 3072) + 13) % 251) as u8)
        .collect();
    let png = encode(ImageProfile::PngV1, &rgb).unwrap();
    let model = GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "png-v1".into(),
        photo_id: hash("photo_id"),
        capture_id: hash("capture_id"),
        manifest_tx: hash("manifest_tx"),
        proof_tx: hash("proof_tx"),
    };
    let png = embed_xmp(&png, &model).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&png)), refs["image_sha256"].as_str().unwrap());
    png
}

fn jpeg_vector() -> (Vec<u8>, GalaxyXmp) {
    let jpeg = decode_any(JPEG).unwrap();
    let model = GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "jpeg-v1".into(),
        photo_id: photo_id(&jpeg.raster).unwrap(),
        capture_id: [1; 32],
        manifest_tx: [2; 32],
        proof_tx: [3; 32],
    };
    (embed_xmp(JPEG, &model).unwrap(), model)
}

/// Return full IDAT chunks (including headers and CRCs), without reconstructing a raster.
fn idat_chunks(mut png: &[u8]) -> Vec<&[u8]> {
    png = png.strip_prefix(b"\x89PNG\r\n\x1a\n").unwrap();
    let mut chunks = Vec::new();
    while !png.is_empty() {
        let len = u32::from_be_bytes(png[..4].try_into().unwrap()) as usize;
        let (chunk, rest) = png.split_at(12 + len);
        if &chunk[4..8] == b"IDAT" {
            chunks.push(chunk);
        }
        png = rest;
    }
    assert!(!chunks.is_empty());
    chunks
}

/// Return the JPEG from SOS through EOI (scan header and entropy-coded bytes).
fn scan_bytes(mut jpeg: &[u8]) -> &[u8] {
    jpeg = jpeg.strip_prefix(b"\xff\xd8").unwrap();
    loop {
        assert_eq!(jpeg[0], 0xff);
        if jpeg[1] == 0xda {
            return jpeg;
        }
        let len = usize::from(u16::from_be_bytes(jpeg[2..4].try_into().unwrap()));
        jpeg = &jpeg[2 + len..];
    }
}

async fn replay(file: &[u8], recording: &[u8], network: &str) -> Report {
    let ledger = Recording::from_json(recording).unwrap().into_ledger();
    let config = NetworkConfig::from_toml(network).unwrap();
    verify_photo(file, None, &VerifyContext::new(&ledger, &config)).await
}

fn assert_both_bindings(report: &Report) {
    for id in ["capture.photo_binding", "capture.rgb_binding", "proof.groth16"] {
        assert!(report.checks.iter().any(|c| c.id == id && c.status == Status::Pass),
            "expected {id} to pass; state={:?}", report.state);
    }
}

#[tokio::test]
async fn repackaged_png_has_identical_idats_photo_id_and_direct_report() {
    let original = png_recorded_vector();
    let enriched = certified_file_with_info(original.clone(), info()).unwrap();
    assert!(original != enriched);
    assert!(idat_chunks(&original) == idat_chunks(&enriched));
    let a = replay(&original, PNG_RECORDING, PNG_NETWORK).await;
    let b = replay(&enriched, PNG_RECORDING, PNG_NETWORK).await;
    assert_eq!(a.state, State::Unsupported); // dev-unattested is not certification.
    assert_both_bindings(&a);
    assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
}

#[test]
fn jpeg_export_keeps_scan_and_lossless_png_keeps_raster_and_complete_packet() {
    let (original, model) = jpeg_vector();
    let enriched = certified_file_with_info(original.clone(), info()).unwrap();
    assert!(scan_bytes(&original) == scan_bytes(&enriched));
    assert!(enriched == certified_file_with_info(enriched.clone(), info()).unwrap());
    let jpeg = decode_any(&enriched).unwrap();
    let packet = jpeg.xmp.unwrap();
    assert_eq!(packet.model, model);
    assert_eq!(photo_id(&jpeg.raster).unwrap(), model.photo_id);
    assert!(std::str::from_utf8(&packet.packet).unwrap().contains("<galaxyinfo:captureLedger>4294967295</galaxyinfo:captureLedger>"));
    let png = lossless_png_from_certified_jpeg(enriched.clone()).unwrap();
    let decoded = decode_any(&png).unwrap();
    assert_eq!(decoded.profile_hint, ImageProfile::PngV1);
    assert!(decoded.raster == jpeg.raster);
    assert_eq!(photo_id(&decoded.raster).unwrap(), model.photo_id);
    assert_eq!(galaxy_image::rgb_root_from_raster(&decoded.raster).unwrap(),
        galaxy_image::rgb_root_from_raster(&jpeg.raster).unwrap());
    assert!(decoded.xmp.unwrap().packet == packet.packet);
    let from_jpeg = certified_photo_details(enriched).unwrap();
    let from_png = certified_photo_details(png).unwrap();
    assert_eq!(from_png.photo_id, hex::encode(model.photo_id));
    assert!(matches!(from_jpeg.image_profile, CaptureProfile::JpegV1));
    assert!(matches!(from_png.image_profile, CaptureProfile::JpegV1));
    assert_eq!(from_jpeg.capture_tx_url, from_png.capture_tx_url);
    assert_eq!(from_jpeg.proof_tx_url, from_png.proof_tx_url);
    assert!(from_png.capture_tx_url.ends_with(&hex::encode_upper(model.manifest_tx)));
    assert!(from_png.proof_tx_url.ends_with(&hex::encode_upper(model.proof_tx)));
}

#[test]
fn rejects_missing_or_wrong_binding_and_untrusted_explorer_links() {
    let (jpeg, model) = jpeg_vector();
    for info in [
        ExportInfo { capture_tx_url: Some("http://testnet.xrpl.org/transactions/abc".into()), ..info() },
        ExportInfo { proof_tx_url: Some(format!("https://testnet.xrpl.org/transactions/{}", hex::encode_upper(model.manifest_tx))), ..info() },
        ExportInfo { proof_tx_url: Some(format!("https://example.org/transactions/{}", hex::encode_upper(model.proof_tx))), ..info() },
    ] {
        assert!(matches!(certified_file_with_info(jpeg.clone(), info), Err(GalaxyError::Image)));
    }
    let bad_account = ExportInfo { account: Some("rrrrrrrrrrrrrrrrrrrrrhoLvTb".into()), ..info() };
    assert!(matches!(certified_file_with_info(jpeg, bad_account), Err(GalaxyError::InvalidInput)));
    for bad in [
        JPEG.to_vec(),
        embed_xmp(JPEG, &GalaxyXmp { photo_id: [0; 32], ..model.clone() }).unwrap(),
    ] {
        assert!(matches!(certified_file_with_info(bad.clone(), info()), Err(GalaxyError::Image)));
        assert!(matches!(lossless_png_from_certified_jpeg(bad), Err(GalaxyError::Image)));
    }
    assert!(matches!(lossless_png_from_certified_jpeg(png_recorded_vector()), Err(GalaxyError::Image)));
}

#[test]
fn jpeg_app1_has_a_smaller_packet_limit_than_png() {
    let (jpeg, _) = jpeg_vector();
    let png = lossless_png_from_certified_jpeg(jpeg.clone()).unwrap();
    let with_policy_name = |name| ExportInfo { policy_name: Some(name), ..info() };
    let base = certified_file_with_info(png.clone(), with_policy_name("p".into())).unwrap();
    let len = decode_any(&base).unwrap().xmp.unwrap().packet.len();
    let name = "p".repeat(1 + galaxy_protocol::xmp::MAX_PACKET_LEN - len);
    let max_png = certified_file_with_info(png, with_policy_name(name.clone())).unwrap();
    assert_eq!(decode_any(&max_png).unwrap().xmp.unwrap().packet.len(), galaxy_protocol::xmp::MAX_PACKET_LEN);
    assert!(matches!(certified_file_with_info(jpeg, with_policy_name(name)), Err(GalaxyError::Image)));
}

// The image from this Testnet run is private and is never committed. The recorded
// ledger responses are public, committed, and replayed without network access.
#[tokio::test]
#[ignore = "set GALAXY_EXPORT_REPLAY_JPEG to the private matching jpeg-v1 export under fixtures/real/"]
async fn recorded_jpeg_and_lossless_png_bind_to_same_capture() {
    let path = std::env::var_os("GALAXY_EXPORT_REPLAY_JPEG").expect("set GALAXY_EXPORT_REPLAY_JPEG");
    let jpeg = std::fs::read(path).expect("read private JPEG fixture");
    let original_report = replay(&jpeg, JPEG_RECORDING, JPEG_NETWORK).await;
    assert_eq!(original_report.state, State::Unsupported); // successful proof, dev-unattested
    assert_both_bindings(&original_report);
    let jpeg = certified_file_with_info(jpeg, info()).unwrap();
    let enriched_report = replay(&jpeg, JPEG_RECORDING, JPEG_NETWORK).await;
    let original = serde_json::to_value(original_report).unwrap();
    assert_eq!(serde_json::to_value(enriched_report).unwrap(), original);
    let png = lossless_png_from_certified_jpeg(jpeg).unwrap();
    let png_report = replay(&png, JPEG_RECORDING, JPEG_NETWORK).await;
    assert_both_bindings(&png_report);
    let mut expected = original;
    expected["image"]["format"] = json!("png");
    expected["image"]["profile"] = json!("png-v1");
    assert_eq!(serde_json::to_value(png_report).unwrap(), expected);
}
