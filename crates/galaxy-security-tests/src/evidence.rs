use galaxy_evidence::{
    bundle::{self, EvidenceBundle, MAX_CAPTURE_STATE_BYTES},
    ingest::from_gxrw,
    synthetic::SyntheticCapture,
    Evidence, ImageProfile, Salts,
};
use galaxy_image::{ANALYSIS_HEIGHT, ANALYSIS_WIDTH, MAX_IMAGE_BYTES, RGB_LEN, SOURCE_HEIGHT, SOURCE_WIDTH};

use super::*;

const HEADER_CAP: usize = MIB;
const PIXELS: usize = ANALYSIS_HEIGHT * ANALYSIS_WIDTH;
const PAYLOAD_LEN: usize = 3 * (SOURCE_WIDTH * SOURCE_HEIGHT * 3 / 2 + PIXELS * 5);
const INGEST_HEAP: usize = 96 * MIB;
const BUNDLE_HEAP: usize = 2 * MAX_IMAGE_BYTES + MAX_CAPTURE_STATE_BYTES + 16 * MIB;

#[test]
fn evidence_arbitrary_binary_inputs_and_size_caps() {
    property(192, (noise(4 * KIB), any::<bool>()), |(bytes, framed)| {
        let raw = if framed { [b"GXRW\x01" as &[u8], &bytes].concat() } else { bytes.clone() };
        let stored = if framed { [b"GXEV\x01\x00" as &[u8], &bytes].concat() } else { bytes };
        let _ = bounded("raw capture", INGEST_HEAP, || from_gxrw(&raw, ImageProfile::PngV1));
        let _ = bounded("evidence bundle", BUNDLE_HEAP, || bundle::decode(&stored));
        Ok(())
    });

    let mut raw = b"GXRW\x01".to_vec();
    raw.extend_from_slice(&((HEADER_CAP + 1) as u32).to_be_bytes());
    raw.resize(9 + HEADER_CAP + 1 + PAYLOAD_LEN, 0);
    assert!(bounded("raw capture oversized header", 0, || from_gxrw(&raw, ImageProfile::PngV1)).is_err());

    let mut stored = b"GXEV\x01\x00".to_vec();
    stored.extend_from_slice(&((MAX_IMAGE_BYTES + 1) as u32).to_be_bytes());
    stored.resize(10 + MAX_IMAGE_BYTES + 1, 0);
    assert!(bounded("evidence bundle oversized image", 0, || bundle::decode(&stored)).is_err());
}

#[test]
fn gxrw_framed_header_noise_and_mutations() {
    let bytes = SyntheticCapture::default().to_gxrw();
    let header_end = 9 + u32::from_be_bytes(bytes[5..9].try_into().unwrap()) as usize;
    let payload = &bytes[header_end..];
    assert_eq!(payload.len(), PAYLOAD_LEN);
    property(24, noise(4 * KIB), |header| {
        let mut input = Vec::with_capacity(9 + header.len() + payload.len());
        input.extend_from_slice(b"GXRW\x01");
        input.extend_from_slice(&(header.len() as u32).to_be_bytes());
        input.extend_from_slice(&header);
        input.extend_from_slice(payload);
        let _ = bounded("raw capture header", INGEST_HEAP, || from_gxrw(&input, ImageProfile::PngV1));
        Ok(())
    });
    for profile in [ImageProfile::PngV1, ImageProfile::JpegV1] {
        let evidence = bounded("synthetic capture", INGEST_HEAP, || from_gxrw(&bytes, profile)).unwrap();
        check_ingest(&evidence, profile).unwrap();
        property(12, (any::<bool>(), mutation()), |(header, (position, mask))| {
            let mut input = bytes.clone();
            let range = if header { &mut input[..header_end] } else { &mut input[header_end..] };
            change(range, position, mask);
            if let Ok(evidence) = bounded("mutated raw capture", INGEST_HEAP, || from_gxrw(&input, profile)) {
                // GXRW ingestion quantizes and does not expose an inverse encoder.
                check_ingest(&evidence, profile)?;
            }
            Ok(())
        });
    }
}

fn check_ingest(evidence: &Evidence, profile: ImageProfile) -> TestCaseResult {
    prop_assert!(evidence.validate().is_ok());
    prop_assert_eq!(evidence.image.profile, profile);
    prop_assert_eq!(evidence.image.rgb.len(), RGB_LEN);
    prop_assert!(evidence.image.encoded.len() <= MAX_IMAGE_BYTES);
    prop_assert!(galaxy_image::decode_any(&evidence.image.encoded).unwrap().raster == evidence.image.rgb);
    Ok(())
}

#[test]
fn gxev_generated_round_trips_and_canonical_mutations() {
    let raw = SyntheticCapture::default().to_gxrw();
    for profile in [ImageProfile::PngV1, ImageProfile::JpegV1] {
        let evidence = from_gxrw(&raw, profile).unwrap();
        property(16, (noise(512), any::<u32>(), mutation()), |(capture_state, salt, (position, mask))| {
            let salts = Salts {
                depth0: 1u32.into(), frame_minus: 2u32.into(), frame_plus: 3u32.into(),
                camera: 4u32.into(), imu: 5u32.into(), bluetooth: 6u32.into(),
                noise: 7u32.into(), meta: salt.into(),
            };
            let model = EvidenceBundle { evidence: evidence.clone(), salts, capture_state };
            let mut bytes = bundle::encode(&model).unwrap();
            prop_assert!(bounded("generated evidence bundle", BUNDLE_HEAP, || bundle::decode(&bytes)).is_ok_and(|v| v == model));
            let at = change(&mut bytes, position, mask);
            if let Ok(parsed) = bounded("mutated evidence bundle", BUNDLE_HEAP, || bundle::decode(&bytes)) {
                prop_assert!(bundle::encode(&parsed).is_ok_and(|encoded| encoded == bytes), "noncanonical GXEV at offset {at}");
            }
            Ok(())
        });
    }
}
