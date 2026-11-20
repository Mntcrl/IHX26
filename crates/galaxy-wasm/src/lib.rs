//! wasm-bindgen surface for the browser verifier: image decode, ids, `rgb_root`, XMP extract and full verification with a JavaScript `LedgerSource` (spec §15).
//!
//! [`inspect`] is target-independent so native tests pin it to the Rust image core; the
//! JavaScript bindings and the verification entry point exist only on `wasm32`.

use galaxy_image::{Container, ImageError, ImageProfile};
use serde::Serialize;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::{inspect_image, verify};

/// Local, network-free image summary. XMP references are untrusted lookup hints.
#[derive(Debug, Serialize)]
pub struct Inspection {
    container: Option<&'static str>,
    profile: Option<ImageProfile>,
    photo_id: Option<String>,
    rgb_root: Option<String>,
    xmp: Option<XmpReferences>,
    /// A present XMP packet that is not valid Galaxy XMP; the pixels still decode.
    xmp_error: Option<String>,
    errors: Vec<InspectionError>,
}

#[derive(Debug, Serialize)]
struct XmpReferences {
    protocol: String,
    network: String,
    image_profile: String,
    photo_id: String,
    capture_id: String,
    manifest_tx: String,
    proof_tx: String,
}

#[derive(Debug, Serialize)]
struct InspectionError {
    state: &'static str,
    detail: String,
}

/// Hashes use spec §2/§4 casing: ids lowercase hex, transaction hashes uppercase hex,
/// `rgb_root` as the 32-byte big-endian field element in lowercase hex.
pub fn inspect(bytes: &[u8]) -> Inspection {
    let result = galaxy_image::decode_with_xmp_status(bytes).and_then(|(image, xmp)| {
        let photo_id = hex::encode(galaxy_image::photo_id(&image.raster)?);
        let rgb_root = hex::encode(galaxy_crypto::field::fr_to_be(
            &galaxy_image::rgb_root_from_raster(&image.raster)?,
        ));
        let (xmp, xmp_error) = match xmp {
            Ok(xmp) => (xmp.map(|xmp| references(xmp.model)), None),
            Err(err) => (None, Some(err.to_string())),
        };
        Ok(Inspection {
            container: Some(match image.container {
                Container::Png => "png",
                Container::Jpeg => "jpeg",
            }),
            profile: Some(image.profile_hint),
            photo_id: Some(photo_id),
            rgb_root: Some(rgb_root),
            xmp,
            xmp_error,
            errors: Vec::new(),
        })
    });
    result.unwrap_or_else(|err| Inspection {
        container: None,
        profile: None,
        photo_id: None,
        rgb_root: None,
        xmp: None,
        xmp_error: None,
        errors: vec![InspectionError {
            state: match err {
                ImageError::Invalid(_) => "INVALID",
                ImageError::Unsupported(_) => "UNSUPPORTED",
            },
            detail: err.to_string(),
        }],
    })
}

fn references(model: galaxy_protocol::xmp::GalaxyXmp) -> XmpReferences {
    XmpReferences {
        protocol: model.protocol,
        network: model.network,
        image_profile: model.image_profile,
        photo_id: hex::encode(model.photo_id),
        capture_id: hex::encode(model.capture_id),
        manifest_tx: hex::encode_upper(model.manifest_tx),
        proof_tx: hex::encode_upper(model.proof_tx),
    }
}

#[cfg(test)]
mod tests;
