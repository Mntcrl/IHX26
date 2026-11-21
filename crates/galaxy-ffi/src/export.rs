use std::sync::LazyLock;

use galaxy_protocol::{
    network::NetworkConfig,
    xmp::{GalaxyInfo, GalaxyXmp, XmpError},
};
use galaxy_xrpl::keys::decode_address;

use crate::{capture::BUNDLED_NETWORK, guarded, CaptureProfile, GalaxyError, Result};

/// Public export metadata only. Omit values not known from the capture journal.
#[derive(uniffi::Record)]
pub struct ExportInfo {
    pub account: Option<String>,
    pub registration_tx: Option<String>,
    pub capture_ledger: Option<u32>,
    pub selection_ledger: Option<u32>,
    pub proof_ledger: Option<u32>,
    pub capture_time: Option<String>,
    pub claimed_time: Option<String>,
    pub circuit_id: Option<String>,
    pub policy_id: Option<String>,
    pub policy_name: Option<String>,
    pub setup_ceremony: Option<String>,
    pub attestation_environment: Option<String>,
    pub capture_tx_url: Option<String>,
    pub proof_tx_url: Option<String>,
    pub app_version: String,
}

/// Local photo binding and explorer links. These details are not a ledger verdict.
#[derive(uniffi::Record)]
pub struct CertifiedPhotoDetails {
    pub photo_id: String,
    pub image_profile: CaptureProfile,
    pub capture_tx_url: String,
    pub proof_tx_url: String,
}

fn explorer_urls(xmp: &GalaxyXmp) -> std::result::Result<(String, String), XmpError> {
    static NETWORK: LazyLock<Option<NetworkConfig>> =
        LazyLock::new(|| NetworkConfig::from_toml(BUNDLED_NETWORK).ok());
    let network = NETWORK
        .as_ref()
        .filter(|network| network.name == xmp.network && network.explorer_tx.starts_with("https://"))
        .ok_or(XmpError::Invalid("explorer network"))?;
    Ok((
        network.explorer_tx_url(&xmp.manifest_tx),
        network.explorer_tx_url(&xmp.proof_tx),
    ))
}

/// Read the committed profile and configured explorer links of a photo-bound export.
#[uniffi::export]
pub fn certified_photo_details(file: Vec<u8>) -> Result<CertifiedPhotoDetails> {
    guarded(|| {
        let xmp = galaxy_image::bound_xmp(&file).map_err(|_| GalaxyError::Image)?;
        let (capture_tx_url, proof_tx_url) = explorer_urls(&xmp).map_err(|_| GalaxyError::Network)?;
        let image_profile = match xmp.image_profile.as_str() {
            "png-v1" => CaptureProfile::PngV1,
            "jpeg-v1" => CaptureProfile::JpegV1,
            _ => return Err(GalaxyError::Image),
        };
        Ok(CertifiedPhotoDetails {
            photo_id: hex::encode(xmp.photo_id),
            image_profile,
            capture_tx_url,
            proof_tx_url,
        })
    })
}

/// Replace the XMP with the same Galaxy references and public informational fields.
/// The input must already be certified; this does not contact XRPL or certify a file.
#[uniffi::export]
pub fn certified_file_with_info(file: Vec<u8>, info: ExportInfo) -> Result<Vec<u8>> {
    guarded(|| {
        if info.account.as_deref().is_some_and(|account| decode_address(account).is_err()) {
            return Err(GalaxyError::InvalidInput);
        }
        galaxy_image::repackage_bound_xmp(&file, |xmp| {
            let (capture_tx_url, proof_tx_url) = explorer_urls(xmp)?;
            for (name, supplied, expected) in [
                ("captureTxUrl", info.capture_tx_url.as_deref(), capture_tx_url.as_str()),
                ("proofTxUrl", info.proof_tx_url.as_deref(), proof_tx_url.as_str()),
            ] {
                if supplied.is_some_and(|value| value != expected) {
                    return Err(XmpError::Invalid(name));
                }
            }
            xmp.to_packet_with_info(&GalaxyInfo {
                account: info.account.as_deref(),
                registration_tx: info.registration_tx.as_deref(),
                capture_ledger: info.capture_ledger,
                selection_ledger: info.selection_ledger,
                proof_ledger: info.proof_ledger,
                capture_time: info.capture_time.as_deref(),
                claimed_time: info.claimed_time.as_deref(),
                circuit_id: info.circuit_id.as_deref(),
                policy_id: info.policy_id.as_deref(),
                policy_name: info.policy_name.as_deref(),
                setup_ceremony: info.setup_ceremony.as_deref(),
                attestation_environment: info.attestation_environment.as_deref(),
                capture_tx_url: &capture_tx_url,
                proof_tx_url: &proof_tx_url,
                app_version: &info.app_version,
            })
        })
        .map_err(|_| GalaxyError::Image)
    })
}

/// Re-encode the reference-decoded jpeg-v1 raster as PNG with the exact XMP bytes.
#[uniffi::export]
pub fn lossless_png_from_certified_jpeg(file: Vec<u8>) -> Result<Vec<u8>> {
    guarded(|| galaxy_image::lossless_png_from_bound_jpeg(&file).map_err(|_| GalaxyError::Image))
}
