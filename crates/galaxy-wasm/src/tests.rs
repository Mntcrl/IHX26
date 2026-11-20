use super::inspect;
use galaxy_crypto::field::fr_to_be;
use galaxy_image::{decode_any, embed_xmp, encode, photo_id, rgb_root_from_raster, ImageProfile};
use galaxy_protocol::xmp::GalaxyXmp;
use serde_json::{json, Value};

const PHOTO: &[u8] = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");

fn inspected(bytes: &[u8]) -> Value {
    serde_json::to_value(inspect(bytes)).unwrap()
}

fn ids(raster: &[u8]) -> (String, String) {
    (
        hex::encode(photo_id(raster).unwrap()),
        hex::encode(fr_to_be(&rgb_root_from_raster(raster).unwrap())),
    )
}

fn xmp(photo: [u8; 32]) -> GalaxyXmp {
    GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "png-v1".into(),
        photo_id: photo,
        capture_id: [0xab; 32],
        manifest_tx: [0xcd; 32],
        proof_tx: [0xef; 32],
    }
}

#[test]
fn jpeg_ids_match_image_core() {
    let raster = decode_any(PHOTO).unwrap().raster;
    let (photo_id, rgb_root) = ids(&raster);
    assert_eq!(
        inspected(PHOTO),
        json!({
            "container": "jpeg", "profile": "jpeg-v1", "photo_id": photo_id, "rgb_root": rgb_root,
            "xmp": null, "xmp_error": null, "errors": [],
        })
    );
}

#[test]
fn pixel_identical_png_has_the_same_ids() {
    let raster = decode_any(PHOTO).unwrap().raster;
    let png = encode(ImageProfile::PngV1, &raster).unwrap();
    let (jpeg, png) = (inspected(PHOTO), inspected(&png));
    assert_eq!((png["container"].as_str(), png["profile"].as_str()), (Some("png"), Some("png-v1")));
    assert_eq!(png["photo_id"], jpeg["photo_id"]);
    assert_eq!(png["rgb_root"], jpeg["rgb_root"]);
}

#[test]
fn xmp_references_use_spec_casing() {
    let raster = decode_any(PHOTO).unwrap().raster;
    let png = encode(ImageProfile::PngV1, &raster).unwrap();
    let tagged = embed_xmp(&png, &xmp(photo_id(&raster).unwrap())).unwrap();
    let result = inspected(&tagged);
    assert_eq!(
        result["xmp"],
        json!({
            "protocol": "galaxy/v1", "network": "xrpl-testnet", "image_profile": "png-v1",
            "photo_id": hex::encode(photo_id(&raster).unwrap()), "capture_id": "ab".repeat(32),
            "manifest_tx": "CD".repeat(32), "proof_tx": "EF".repeat(32),
        })
    );
    assert_eq!(result["photo_id"], inspected(&png)["photo_id"]);
}

#[test]
fn unparsable_xmp_is_reported_without_losing_pixel_ids() {
    let photo = [0x5a; 32];
    let tagged = embed_xmp(PHOTO, &xmp(photo)).unwrap();
    let needle = hex::encode(photo);
    let at = tagged.windows(needle.len()).position(|window| window == needle.as_bytes()).unwrap();
    let mut broken = tagged.clone();
    broken[at] = b'z';
    let result = inspected(&broken);
    assert_eq!(result["xmp"], Value::Null);
    assert!(result["xmp_error"].as_str().unwrap().contains("XMP"));
    assert_eq!(result["photo_id"], inspected(PHOTO)["photo_id"]);
    assert_eq!(result["errors"], json!([]));
}

#[test]
fn undecodable_inputs_fail_closed() {
    let unsupported = inspected(b"GIF89a");
    assert_eq!(unsupported["errors"][0]["state"], "UNSUPPORTED");
    assert_eq!(unsupported["photo_id"], Value::Null);
    let truncated = inspected(&PHOTO[..PHOTO.len() / 2]);
    assert_eq!(truncated["errors"][0]["state"], "INVALID");
    assert_eq!(truncated["rgb_root"], Value::Null);
}
