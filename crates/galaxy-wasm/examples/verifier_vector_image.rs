//! Reconstruct the committed `dev-unattested` recording's synthetic PNG. Its
//! pinned XMP references keep this snapshot independent of the active policy.

use galaxy_image::{embed_xmp, encode, ImageProfile, RGB_LEN};
use galaxy_protocol::{xmp::GalaxyXmp, Hash32};
use serde_json::Value;

fn main() {
    let path = std::env::args_os().nth(1).expect("pass an output PNG path");
    let refs: Value = serde_json::from_str(include_str!(
        "../../../test-vectors/verifier/dev-unattested/refs.json"
    ))
    .expect("committed verifier references");
    let hash = |key: &str| -> Hash32 {
        let mut bytes = [0; 32];
        hex::decode_to_slice(refs[key].as_str().expect("fixture hash"), &mut bytes)
            .expect("32-byte hexadecimal hash");
        bytes
    };
    // The fixture's raster(1) recipe (galaxy-verifier/tests/vectors.rs).
    let rgb: Vec<_> = (0..RGB_LEN)
        .map(|i| ((31 * i + 7 * (i / 3072) + 13) % 251) as u8)
        .collect();
    let png = encode(ImageProfile::PngV1, &rgb).expect("synthetic RGB raster");
    let xmp = GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "png-v1".into(),
        photo_id: hash("photo_id"),
        capture_id: hash("capture_id"),
        manifest_tx: hash("manifest_tx"),
        proof_tx: hash("proof_tx"),
    };
    std::fs::write(path, embed_xmp(&png, &xmp).expect("synthetic PNG with XMP"))
        .expect("write synthetic verifier fixture");
}
