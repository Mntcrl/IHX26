use galaxy_image::{decode_any, decode_with_xmp_status, embed_xmp, encode, ImageProfile, MAX_IMAGE_BYTES, RGB_LEN};
use galaxy_protocol::xmp::{GalaxyXmp, IMAGE_PROFILES, MAX_PACKET_LEN, NETWORKS, PROTOCOL};

use super::*;

const IMAGE_HEAP: usize = 24 * MIB;
const XMP_HEAP: usize = 4 * MAX_PACKET_LEN;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG: &[u8] = b"\xff\xd8";
const PROFILES: [ImageProfile; 2] = [ImageProfile::PngV1, ImageProfile::JpegV1];

fn xmp(profile: usize, network: usize, ids: [[u8; 32]; 4]) -> GalaxyXmp {
    let [photo_id, capture_id, manifest_tx, proof_tx] = ids;
    GalaxyXmp {
        protocol: PROTOCOL.into(), network: NETWORKS[network].into(),
        image_profile: IMAGE_PROFILES[profile].into(), photo_id, capture_id, manifest_tx, proof_tx,
    }
}

#[test]
fn image_decoders_arbitrary_and_oversize() {
    property(128, (noise(4 * KIB), any::<bool>()), |(bytes, framed)| {
        for prefix in [PNG, JPEG] {
            let input = if framed { [prefix, &bytes].concat() } else { bytes.clone() };
            let _ = bounded("strict image", IMAGE_HEAP, || decode_any(&input));
            let _ = bounded("image with separate XMP status", IMAGE_HEAP, || decode_with_xmp_status(&input));
        }
        Ok(())
    });
    let mut oversized = vec![0; MAX_IMAGE_BYTES + 1];
    for prefix in [PNG, JPEG] {
        oversized[..prefix.len()].copy_from_slice(prefix);
        assert!(bounded("image oversize", 0, || decode_any(&oversized)).is_err());
    }
}

#[test]
fn generated_images_and_embedded_metadata_round_trip() {
    property(8, (any::<u8>(), any::<[[u8; 32]; 4]>()), |(shade, ids)| {
        let raster = vec![shade; RGB_LEN];
        for (p, profile) in PROFILES.into_iter().enumerate() {
            let model = xmp(p, 1, ids);
            let encoded = encode(profile, &raster).unwrap();
            let tagged = embed_xmp(&encoded, &model).unwrap();
            let decoded = bounded("generated image", IMAGE_HEAP, || decode_any(&tagged)).unwrap();
            prop_assert_eq!(decoded.profile_hint, profile);
            prop_assert_eq!(decoded.raster.len(), RGB_LEN);
            prop_assert!(decoded.raster == decode_any(&encoded).unwrap().raster);
            let extracted = decoded.xmp.unwrap();
            prop_assert_eq!(extracted.model, model.clone());
            prop_assert_eq!(extracted.packet, model.to_packet().unwrap());
            if profile == ImageProfile::PngV1 {
                prop_assert!(decoded.raster == raster);
            } else {
                // JPEG is lossy; a constant grayscale image must stay within IDCT rounding.
                prop_assert!(decoded.raster.iter().all(|&c| c.abs_diff(shade) <= 1));
            }
        }
        Ok(())
    });
}

#[test]
fn png_jpeg_single_byte_mutations() {
    let model = xmp(0, 1, [[0xa5; 32]; 4]);
    let rgb = vec![0x80; RGB_LEN];
    let png = embed_xmp(&encode(ImageProfile::PngV1, &rgb).unwrap(), &model).unwrap();
    let jpeg = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");
    for (fixture, profile) in [(png.as_slice(), ImageProfile::PngV1), (jpeg.as_slice(), ImageProfile::JpegV1)] {
        let reference = decode_any(fixture).unwrap();
        prop_assert_image_shape(&reference, profile).unwrap();
        property(32, mutation(), |(position, mask)| {
            let mut bytes = fixture.to_vec();
            change(&mut bytes, position, mask);
            if let Ok(decoded) = bounded("mutated image", IMAGE_HEAP, || decode_any(&bytes)) {
                prop_assert_image_shape(&decoded, profile)?;
                if profile == ImageProfile::PngV1 {
                    let compressed = encode(profile, &decoded.raster).unwrap();
                    let encoded = embed_xmp(&compressed, &decoded.xmp.unwrap().model).unwrap();
                    prop_assert!(encoded == bytes, "PNG fixture mutation escaped chunk integrity checks");
                } else {
                    // JPEG has no canonical container encoding. Check that the decoded sample
                    // is a stable raster, using the lossless profile as the round-trip encoder.
                    let lossless = encode(ImageProfile::PngV1, &decoded.raster).unwrap();
                    prop_assert!(decode_any(&lossless).unwrap().raster == decoded.raster);
                }
            }
            Ok(())
        });
    }
}

fn prop_assert_image_shape(image: &galaxy_image::DecodedImage, profile: ImageProfile) -> TestCaseResult {
    prop_assert_eq!(image.profile_hint, profile);
    prop_assert_eq!(image.raster.len(), RGB_LEN);
    Ok(())
}

#[test]
fn xmp_arbitrary_bytes_and_packet_cap() {
    property(256, (noise(4 * KIB), any::<bool>()), |(bytes, framed)| {
        let input = if framed { [b"<xmp>" as &[u8], &bytes, b"</xmp>"].concat() } else { bytes };
        let _ = bounded("XMP", XMP_HEAP, || GalaxyXmp::parse_packet(&input));
        Ok(())
    });
    let too_large = vec![b' '; MAX_PACKET_LEN + 1];
    assert!(bounded("XMP oversize", 0, || GalaxyXmp::parse_packet(&too_large)).is_err());
}

#[test]
fn xmp_generated_packets_and_semantic_mutations() {
    property(192, (0..2usize, 0..3usize, any::<[[u8; 32]; 4]>(), mutation()), |(profile, network, ids, (position, mask))| {
        let model = xmp(profile, network, ids);
        let mut packet = model.to_packet().unwrap();
        prop_assert_eq!(bounded("XMP", XMP_HEAP, || GalaxyXmp::parse_packet(&packet)).unwrap(), model);
        change(&mut packet, position, mask);
        if let Ok(parsed) = bounded("mutated XMP", XMP_HEAP, || GalaxyXmp::parse_packet(&packet)) {
            // XML permits alternate namespace prefixes, attribute form and whitespace.
            // Serialize the model, not the surrounding XML, to check semantic stability.
            prop_assert_eq!(GalaxyXmp::parse_packet(&parsed.to_packet().unwrap()).unwrap(), parsed);
        }
        Ok(())
    });
}
