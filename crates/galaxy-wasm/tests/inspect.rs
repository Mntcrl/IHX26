//! `inspect_image` through the wasm-bindgen boundary on wasm32 (runner: `.tools/bin/wasm-bindgen-test-runner`, Node.js),
//! checked against the galaxy-image jpeg-v1 golden vectors computed natively.

#![cfg(target_arch = "wasm32")]

use galaxy_crypto::field::{fr_from_dec, fr_to_be};
use galaxy_image::{decode_any, encode, ImageProfile};
use serde_json::Value;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

const PHOTO: &[u8] = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");
const GOLDEN: &str = include_str!("../../../test-vectors/image/jpeg-golden.json");

fn inspect(bytes: &[u8]) -> Value {
    let value: JsValue = galaxy_wasm::inspect_image(bytes).unwrap().into();
    serde_json::from_str(&js_sys::JSON::stringify(&value).unwrap().as_string().unwrap()).unwrap()
}

fn golden_case(name: &str) -> Value {
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    golden["cases"].as_array().unwrap().iter().find(|case| case["name"] == name).unwrap().clone()
}

#[wasm_bindgen_test]
fn jpeg_v1_reencode_matches_golden_ids() {
    let case = golden_case("cc0-street-photo");
    let raster = decode_any(PHOTO).unwrap().raster;
    let rgb_root = hex::encode(fr_to_be(&fr_from_dec(case["rgb_root"].as_str().unwrap()).unwrap()));
    let jpeg = inspect(&encode(ImageProfile::JpegV1, &raster).unwrap());
    assert_eq!(jpeg["container"], "jpeg");
    assert_eq!(jpeg["photo_id"], case["photo_id"]);
    assert_eq!(jpeg["rgb_root"].as_str(), Some(rgb_root.as_str()));
    assert_eq!(jpeg["errors"], serde_json::json!([]));

    // The same pixels in png-v1 carry the same identifiers.
    let decoded = decode_any(&encode(ImageProfile::JpegV1, &raster).unwrap()).unwrap().raster;
    let png = inspect(&encode(ImageProfile::PngV1, &decoded).unwrap());
    assert_eq!((png["container"].as_str(), png["photo_id"].clone()), (Some("png"), case["photo_id"].clone()));
    assert_eq!(png["rgb_root"], jpeg["rgb_root"]);
}

#[wasm_bindgen_test]
fn source_photo_matches_core_in_wasm() {
    let raster = decode_any(PHOTO).unwrap().raster;
    let result = inspect(PHOTO);
    assert_eq!(result["photo_id"].as_str().unwrap(), hex::encode(galaxy_image::photo_id(&raster).unwrap()));
    assert_eq!(result["profile"], "jpeg-v1");
    assert_eq!(result["xmp"], Value::Null);
}

#[wasm_bindgen_test]
fn rejects_non_image_bytes() {
    assert_eq!(inspect(b"not an image")["errors"][0]["state"], "UNSUPPORTED");
}
