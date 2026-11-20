//! GXRW ingest (§21, §5), commitments (§6) and the GXEV bundle codec, driven by the deterministic
//! synthetic capture. Golden values: test-vectors/evidence/synthetic-v1.json
//! (regenerate on Linux only with `GALAXY_EVIDENCE_REGEN=1` after an intentional change).

use galaxy_crypto::field::{fr_from_dec, fr_to_dec};
use galaxy_evidence::{
    bundle::{self, BundleError, EvidenceBundle, MAX_CAPTURE_STATE_BYTES},
    commit, component_elements,
    ingest::{from_gxrw, IngestError},
    synthetic::{assemble, SyntheticCapture},
    try_commit, Evidence, ImageProfile, Salts, ValidationError, BLUETOOTH_ELEMENTS, CAMERA_ELEMENTS,
    IMU_ELEMENTS, IMU_MAX, META_ELEMENTS, NOISE_ELEMENTS,
};
use galaxy_image::{analysis_raster, decode_any, photo_id, rgb_root_from_raster};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

static CAPTURE: LazyLock<(Value, Vec<u8>)> = LazyLock::new(|| {
    let synthetic = SyntheticCapture::default();
    (synthetic.header(), synthetic.payload())
});
static PNG: LazyLock<Evidence> = LazyLock::new(|| from_gxrw(&gxrw(), ImageProfile::PngV1).unwrap());
static JPEG: LazyLock<Evidence> = LazyLock::new(|| from_gxrw(&gxrw(), ImageProfile::JpegV1).unwrap());

fn capture() -> &'static (Value, Vec<u8>) {
    &CAPTURE
}

fn gxrw() -> Vec<u8> {
    let (header, payload) = capture();
    assemble(header, payload)
}

fn ingest_with(edit: impl FnOnce(&mut Value)) -> Result<Evidence, IngestError> {
    let (header, payload) = capture();
    let mut header = header.clone();
    edit(&mut header);
    from_gxrw(&assemble(&header, payload), ImageProfile::PngV1)
}

fn evidence(profile: ImageProfile) -> &'static Evidence {
    match profile {
        ImageProfile::PngV1 => &PNG,
        ImageProfile::JpegV1 => &JPEG,
    }
}

fn fixed_salts() -> Salts {
    let s = |i: u64| galaxy_crypto::Fr::from(0x5a17_0000_0000_0000u64 + i);
    Salts { depth0: s(1), frame_minus: s(2), frame_plus: s(3), camera: s(4), imu: s(5), bluetooth: s(6), noise: s(7), meta: s(8) }
}

fn decimals(values: &[galaxy_crypto::Fr]) -> Vec<String> {
    values.iter().map(fr_to_dec).collect()
}

fn profile_vector(e: &Evidence) -> Value {
    let c = commit(e, &fixed_salts());
    let frames: Vec<Value> = e.frames.iter().map(|f| {
        let depth: Vec<u8> = f.depth_mm.iter().flat_map(|d| d.to_be_bytes()).collect();
        json!({ "rgb_sha256": sha(&f.rgb), "depth_sha256": sha(&depth), "conf_sha256": sha(&f.conf) })
    }).collect();
    let camera: Vec<Value> = e.camera.iter().map(|k| json!({
        "fx": k.fx, "fy": k.fy, "cx": k.cx, "cy": k.cy, "r": k.r, "t": k.t, "ts_us": k.ts_us, "track": k.track,
    })).collect();
    let samples: Vec<u8> = e.imu.windows.iter().flatten()
        .flat_map(|s| [s.dt_us.to_be_bytes(), s.w[0].to_be_bytes(), s.w[1].to_be_bytes(), s.w[2].to_be_bytes()].concat())
        .collect();
    let [_, _, bluetooth, noise, meta] = component_elements(e);
    json!({
        "encoded_sha256": sha(&e.image.encoded),
        "photo_id": hex::encode(photo_id(&e.image.rgb).unwrap()),
        "rgb_root": fr_to_dec(&c.rgb_root),
        "frames": frames,
        "camera": camera,
        "imu": {
            "g0": e.imu.g0, "a_peak_mg": e.imu.a_peak_mg, "samples_sha256": sha(&samples),
            "real_samples": e.imu.windows.iter().map(|w| w.iter().filter(|s| s.dt_us != 0).count()).collect::<Vec<_>>(),
        },
        "bluetooth_elements": decimals(&bluetooth),
        "noise_elements": decimals(&noise),
        "meta_elements": decimals(&meta),
        "components": decimals(&c.components),
        "evidence_root": fr_to_dec(&c.evidence_root),
        "bundle_sha256": sha(&bundle::encode(&EvidenceBundle {
            evidence: e.clone(),
            salts: fixed_salts(),
            capture_state: b"state-v1".to_vec(),
        }).unwrap()),
    })
}

fn compute() -> Value {
    json!({
        "version": 1,
        "capture": "galaxy_evidence::synthetic::SyntheticCapture::default()",
        "gxrw_sha256": sha(&gxrw()),
        "salts": decimals(&[1u64, 2, 3, 4, 5, 6, 7, 8].map(|i| galaxy_crypto::Fr::from(0x5a17_0000_0000_0000u64 + i))),
        "png-v1": profile_vector(evidence(ImageProfile::PngV1)),
        "jpeg-v1": profile_vector(evidence(ImageProfile::JpegV1)),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn expected() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-vectors/evidence/synthetic-v1.json");
    if std::env::var_os("GALAXY_EVIDENCE_REGEN").is_some() {
        let mut text = serde_json::to_string_pretty(&compute()).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[cfg(target_arch = "wasm32")]
fn expected() -> Value {
    serde_json::from_str(include_str!("../../../test-vectors/evidence/synthetic-v1.json")).unwrap()
}

#[test]
fn golden_synthetic_evidence() {
    let expected = expected();
    let actual = compute();
    for key in ["version", "gxrw_sha256", "salts", "png-v1", "jpeg-v1"] {
        assert_eq!(expected[key], actual[key], "golden mismatch at {key}");
    }
    let salts = fixed_salts();
    assert_eq!(expected["salts"][7].as_str().map(|s| fr_from_dec(s).unwrap()), Some(salts.meta));
}

#[test]
fn ingest_quantization() {
    let e = evidence(ImageProfile::PngV1);
    // Intrinsics: fx/7.5, (cx + 0.5)/7.5 − 0.5, Q16 rounded half away from zero.
    for camera in &e.camera {
        assert_eq!([camera.fx, camera.fy, camera.cx, camera.cy], [12_585_097, 12_585_097, 8_355_840, 6_260_873]);
        // Identity ARKit rotation → R_cv = diag(1, −1, −1) in Q30.
        assert_eq!(camera.r, [1 << 30, 0, 0, 0, -(1 << 30), 0, 0, 0, -(1 << 30)]);
        assert_eq!(camera.track, 2);
    }
    assert_eq!(e.camera.map(|c| c.t), [[-20_000, 0, 0], [0, 1_000, 0], [20_000, 2_000, 0]]);
    assert_eq!(e.camera.map(|c| c.ts_us), [5_000_000_000, 5_000_150_000, 5_000_300_000]);
    // IMU: 100 Hz samples in (t0, t1] and (t1, t2], padded to 64 at the end.
    for window in &e.imu.windows {
        assert_eq!(window.len(), IMU_MAX);
        assert_eq!(window.iter().filter(|s| s.dt_us != 0).count(), 15);
        assert!(window[..15].iter().all(|s| s.dt_us == 10_000 && s.w == [2097, -1049, 524]));
        assert!(window[15..].iter().all(|s| s.dt_us == 0 && s.w == [0; 3]));
    }
    assert_eq!(e.imu.g0, [0, -1_046_479, -52_429]);
    assert_eq!(e.imu.a_peak_mg, 37);
    // Depth: NaN → 0, conf copied; meta from header and timestamps.
    assert_eq!(e.frames[1].depth_mm[4], 0);
    assert_eq!(e.frames[1].depth_mm[2 * 256 + 128], 1500);
    assert_eq!(e.frames[1].conf[2 * 256 + 128], 2);
    assert_eq!(e.meta.frame_offsets_ms, [-150, 0, 150]);
    assert_eq!((e.meta.version, e.meta.app_build, e.meta.device_model_code, e.meta.ycbcr_matrix_code), (1, 42, 4, 1));
    assert_eq!(e.bluetooth.n_adv, 10);
    assert_eq!(e.noise.stats.len(), 8);
}

#[test]
fn canonical_image_per_profile() {
    let png = evidence(ImageProfile::PngV1);
    let jpeg = evidence(ImageProfile::JpegV1);
    assert_eq!(decode_any(&png.image.encoded).unwrap().raster, png.image.rgb);
    // jpeg-v1: the committed raster is the reference decoder output of the encoded bytes,
    // and the frame-1 analysis raster comes from it; t−/t+ use their own downscaled rasters.
    assert_eq!(decode_any(&jpeg.image.encoded).unwrap().raster, jpeg.image.rgb);
    assert_ne!(jpeg.image.rgb, png.image.rgb);
    assert_eq!(jpeg.frames[1].rgb, analysis_raster(&jpeg.image.rgb).unwrap());
    assert!(jpeg.frames[0] == png.frames[0] && jpeg.frames[2] == png.frames[2]);
    assert_eq!(jpeg.meta.image_profile, ImageProfile::JpegV1);
    assert_eq!(commit(jpeg, &fixed_salts()).rgb_root, rgb_root_from_raster(&jpeg.image.rgb).unwrap());
    assert_eq!(commit(png, &fixed_salts()).rgb_root, rgb_root_from_raster(&png.image.rgb).unwrap());
}

#[test]
fn component_layouts() {
    let e = evidence(ImageProfile::PngV1);
    let [camera, imu, bluetooth, noise, meta] = component_elements(e);
    assert_eq!(
        [camera.len(), imu.len(), bluetooth.len(), noise.len(), meta.len()],
        [CAMERA_ELEMENTS, IMU_ELEMENTS, BLUETOOTH_ELEMENTS, NOISE_ELEMENTS, META_ELEMENTS]
    );
    assert_eq!([CAMERA_ELEMENTS, IMU_ELEMENTS, BLUETOOTH_ELEMENTS, NOISE_ELEMENTS, META_ELEMENTS], [54, 133, 24, 18, 9]);
    // Negative values are x mod r; the IMU packs dt·2^96 + (ω+2^31) limbs.
    assert_eq!(camera[8], galaxy_crypto::field::fr_from_i128(-(1 << 30)));
    assert_eq!(imu[4], galaxy_crypto::Fr::from(0u32));
    let w = |v: i32| u128::from((i64::from(v) + (1 << 31)) as u32);
    let packed = (10_000u128 << 96) | (w(2097) << 64) | (w(-1049) << 32) | w(524);
    assert_eq!(imu[5], galaxy_crypto::Fr::from(packed));
    assert_eq!(imu[5 + 15], galaxy_crypto::Fr::from((w(0) << 64) | (w(0) << 32) | w(0)));
    assert_eq!(noise[1], galaxy_crypto::Fr::from(8u32));
    assert_eq!(meta[1], galaxy_crypto::Fr::from(0u32));
}

#[test]
fn commitments_depend_on_salts_and_evidence() {
    let e = evidence(ImageProfile::PngV1);
    let a = commit(e, &fixed_salts());
    let b = commit(e, &fixed_salts());
    assert_eq!(a.components, b.components);
    let mut other = fixed_salts();
    other.camera = galaxy_crypto::Fr::from(99u32);
    let c = commit(e, &other);
    assert_eq!(c.components[..4], a.components[..4]);
    assert_ne!(c.components[4], a.components[4]);
    assert_ne!(c.evidence_root, a.evidence_root);
    let mut changed = e.clone();
    changed.frames[0].depth_mm[500] ^= 1;
    assert_ne!(commit(&changed, &fixed_salts()).components[2], a.components[2]);
    // Salts are 31 random bytes: every value is < 2^248 and the eight salts differ.
    let random = Salts::random().unwrap();
    let all = [random.depth0, random.frame_minus, random.frame_plus, random.camera, random.imu, random.bluetooth, random.noise, random.meta];
    assert!(all.iter().all(|s| galaxy_crypto::field::fr_to_be(s)[0] == 0));
    assert!((0..8).all(|i| (i + 1..8).all(|j| all[i] != all[j])));
}

#[test]
fn validation_rejects_inconsistent_evidence() {
    let base = evidence(ImageProfile::PngV1).clone();
    let check = |edit: &dyn Fn(&mut Evidence), error: ValidationError| {
        let mut e = base.clone();
        edit(&mut e);
        assert_eq!(e.validate(), Err(error));
        assert!(try_commit(&e, &fixed_salts()).is_err());
    };
    check(&|e| e.frames[1].rgb[0] ^= 1, ValidationError::Image);
    check(&|e| e.frames[0].conf[3] = 3, ValidationError::Frame);
    check(&|e| e.frames[2].depth_mm.pop().map(drop).unwrap(), ValidationError::Frame);
    check(&|e| e.camera[0].r[0] = 1 << 31, ValidationError::Camera);
    check(&|e| e.camera[1].fx = 1 << 26, ValidationError::Camera);
    check(&|e| e.camera[2].ts_us = 1 << 48, ValidationError::Camera);
    check(&|e| e.camera[0].track = 3, ValidationError::Camera);
    check(&|e| e.imu.windows[0][3] = Default::default(), ValidationError::Imu);
    check(&|e| e.imu.windows[1][40].w = [1, 0, 0], ValidationError::Imu);
    check(&|e| e.imu.windows[1].truncate(63), ValidationError::Imu);
    check(&|e| e.imu.windows[0][0].dt_us = 200_000, ValidationError::Imu);
    check(&|e| e.bluetooth.n_unique = 11, ValidationError::Bluetooth);
    check(&|e| e.bluetooth.rssi_histogram[0] += 1, ValidationError::Bluetooth);
    check(&|e| e.noise.stats = vec![0; 17], ValidationError::Noise);
    check(&|e| e.meta.frame_offsets_ms[0] = -149, ValidationError::Meta);
    check(&|e| e.meta.image_profile = ImageProfile::JpegV1, ValidationError::Meta);
}

#[test]
fn ingest_rejects_malformed_captures() {
    assert!(ingest_with(|_| {}).is_ok());
    let header = |edit: fn(&mut Value)| ingest_with(edit);
    assert!(matches!(header(|h| h["image"]["pixel_format"] = json!("420v")), Err(IngestError::Format)));
    assert!(matches!(header(|h| h["image"]["width"] = json!(1280)), Err(IngestError::Format)));
    assert!(matches!(header(|h| h["format"] = json!("galaxy-raw-capture/v2")), Err(IngestError::Format)));
    assert!(matches!(header(|h| h["extra"] = json!(1)), Err(IngestError::Header { .. })));
    assert!(matches!(header(|h| { h.as_object_mut().unwrap().remove("claimed_time_ms"); }), Err(IngestError::Header { .. })));
    assert!(matches!(header(|h| { h["frames"].as_array_mut().unwrap().pop(); }), Err(IngestError::Header { .. })));
    assert!(matches!(header(|h| h["frames"][0]["intrinsics"][0] = Value::Null), Err(IngestError::Header { .. })));
    assert!(header(|h| h["frames"].as_array_mut().unwrap().swap(0, 2)).is_err());
    assert!(matches!(header(|h| h["frames"][1]["role"] = json!("centre")), Err(IngestError::Frames)));
    assert!(matches!(header(|h| h["frames"][2]["timestamp_s"] = json!(5000.1)), Err(IngestError::Frames)));
    assert!(matches!(header(|h| h["frames"][0]["timestamp_s"] = json!(-1.0)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["frames"][0]["tracking"] = json!(3)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["frames"][0]["transform"][12] = json!(3000.0)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["frames"][0]["transform"][0] = json!(2.5)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["frames"][0]["intrinsics"][0] = json!(1e9)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["frames"][0]["intrinsics"][2] = json!(-100.0)), Err(IngestError::Quantization(_))));
    assert!(matches!(header(|h| h["imu"]["samples"][0][1] = json!(5000.0)), Err(IngestError::Quantization(_))));
    // Regions: overlap, gap and wrong size.
    assert!(matches!(header(|h| h["frames"][1]["y"]["offset"] = json!(0)), Err(IngestError::Regions)));
    assert!(matches!(header(|h| h["frames"][0]["conf"]["len"] = json!(49151)), Err(IngestError::Regions)));
    assert!(matches!(header(|h| {
        let (a, b) = (h["frames"][0]["y"].clone(), h["frames"][0]["cbcr"].clone());
        h["frames"][0]["y"] = b;
        h["frames"][0]["cbcr"] = a;
    }), Err(IngestError::Regions)));
    // IMU coverage, rate and duplicates.
    assert!(matches!(header(|h| { h["imu"]["samples"].as_array_mut().unwrap().remove(0); h["imu"]["samples"].as_array_mut().unwrap().remove(0); h["imu"]["samples"].as_array_mut().unwrap().remove(0); h["imu"]["samples"].as_array_mut().unwrap().remove(0); }), Err(IngestError::Motion)));
    let dup = header(|h| {
        let samples = h["imu"]["samples"].as_array_mut().unwrap();
        let copy = samples[12].clone();
        samples.insert(13, copy);
    }).unwrap();
    assert!(dup == *evidence(ImageProfile::PngV1), "duplicate timestamps are dropped");
    let reordered = header(|h| h["imu"]["samples"].as_array_mut().unwrap().swap(20, 21)).unwrap();
    assert!(reordered == *evidence(ImageProfile::PngV1), "IMU samples are time-ordered");
    // Aggregates and device fields.
    assert!(matches!(header(|h| h["bluetooth"]["n_unique"] = json!(11)), Err(IngestError::Evidence(ValidationError::Bluetooth))));
    assert!(matches!(header(|h| h["noise"]["stats"] = json!(vec![0; 17])), Err(IngestError::Evidence(ValidationError::Noise))));
    assert!(matches!(header(|h| h["device"]["app_build"] = json!("4a2")), Err(IngestError::Evidence(ValidationError::Meta))));
    assert!(matches!(header(|h| h["device"]["app_build"] = json!("99999999999")), Err(IngestError::Evidence(ValidationError::Meta))));
    let missing = header(|h| {
        let o = h.as_object_mut().unwrap();
        o.remove("bluetooth");
        o.remove("noise");
    }).unwrap();
    assert!(missing.bluetooth == Default::default() && missing.noise == Default::default());
    let unknown = header(|h| { h["device"]["model"] = json!("iPhone99,9"); h["image"]["ycbcr_matrix"] = json!("unknown"); }).unwrap();
    assert_eq!((unknown.meta.device_model_code, unknown.meta.ycbcr_matrix_code), (0, 0));
}

#[test]
fn ingest_rejects_framing_errors() {
    let bytes = gxrw();
    assert!(matches!(from_gxrw(&bytes[..bytes.len() - 1], ImageProfile::PngV1), Err(IngestError::Regions)));
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(matches!(from_gxrw(&extra, ImageProfile::PngV1), Err(IngestError::Regions)));
    let mut version = bytes.clone();
    version[4] = 2;
    assert!(matches!(from_gxrw(&version, ImageProfile::PngV1), Err(IngestError::Prefix)));
    let mut zero = bytes.clone();
    zero[5..9].copy_from_slice(&[0; 4]);
    assert!(matches!(from_gxrw(&zero, ImageProfile::PngV1), Err(IngestError::Prefix)));
    let mut large = bytes.clone();
    large[5..9].copy_from_slice(&(1u32 << 20 | 1).to_be_bytes());
    assert!(matches!(from_gxrw(&large, ImageProfile::PngV1), Err(IngestError::Prefix)));
    for cut in [0, 4, 8, 100] {
        assert!(from_gxrw(&bytes[..cut], ImageProfile::PngV1).is_err());
    }
    // Confidence value 3 in the center frame payload.
    let (header, payload) = capture();
    let conf_offset = header["frames"][1]["conf"]["offset"].as_u64().unwrap() as usize;
    let mut payload = payload.clone();
    payload[conf_offset + 1000] = 3;
    assert!(matches!(from_gxrw(&assemble(header, &payload), ImageProfile::PngV1), Err(IngestError::Evidence(ValidationError::Frame))));
}

#[test]
fn bundle_round_trip_and_strictness() {
    for profile in [ImageProfile::PngV1, ImageProfile::JpegV1] {
        let original = EvidenceBundle { evidence: evidence(profile).clone(), salts: fixed_salts(), capture_state: b"state-v1".to_vec() };
        let bytes = bundle::encode(&original).unwrap();
        assert_eq!(&bytes[..5], b"GXEV\x01");
        let decoded = bundle::decode(&bytes).unwrap();
        assert!(decoded == original);
        assert_eq!(bundle::encode(&decoded).unwrap(), bytes);

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(matches!(bundle::decode(&trailing), Err(BundleError::Length)));
        let mut magic = bytes.clone();
        magic[4] = 2;
        assert!(matches!(bundle::decode(&magic), Err(BundleError::Prefix)));
        let mut profile_byte = bytes.clone();
        profile_byte[5] = 7;
        assert!(matches!(bundle::decode(&profile_byte), Err(BundleError::Profile)));
        // Salt equal to the field modulus is non-canonical.
        let r = galaxy_crypto::field::fr_to_be(&-galaxy_crypto::Fr::from(1u32));
        let mut modulus = r;
        modulus[31] += 1;
        let salt_at = bytes.len() - 4 - original.capture_state.len() - 32;
        let mut field = bytes.clone();
        field[salt_at..salt_at + 32].copy_from_slice(&modulus);
        assert!(matches!(bundle::decode(&field), Err(BundleError::Field)));
        // Corrupted embedded image or evidence fields fail closed.
        let mut image = bytes.clone();
        image[5 + 1 + 4 + 200] ^= 0x40;
        assert!(bundle::decode(&image).is_err());
        for cut in (0..bytes.len()).step_by(bytes.len() / 97) {
            assert!(bundle::decode(&bytes[..cut]).is_err());
        }
    }
    let mut big = EvidenceBundle { evidence: evidence(ImageProfile::PngV1).clone(), salts: fixed_salts(), capture_state: vec![0; MAX_CAPTURE_STATE_BYTES + 1] };
    assert!(matches!(bundle::encode(&big), Err(BundleError::Length)));
    big.capture_state.truncate(MAX_CAPTURE_STATE_BYTES);
    assert!(bundle::decode(&bundle::encode(&big).unwrap()).is_ok());
    let mut invalid = big;
    invalid.evidence.camera[0].track = 3;
    assert!(matches!(bundle::encode(&invalid), Err(BundleError::Evidence(ValidationError::Camera))));
}
