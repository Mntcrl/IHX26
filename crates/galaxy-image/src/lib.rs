//! Image profiles `png-v1` and `jpeg-v1`: strict decoding to the canonical 1024×768 RGB8 raster, YCbCr→RGB conversion, exact box downscale, 256×192 analysis raster, `photo_id`, and XMP embed/extract (spec §4, §5).

mod jpeg_profile;
mod png_profile;
mod raster;

use galaxy_crypto::{tree::{FrameTree, TreeKind}, Fr};
use galaxy_protocol::{ids, xmp::GalaxyXmp, Hash32};
use thiserror::Error;

pub use raster::{analysis_raster, downscale_rgb, ycbcr420_full_to_rgb, ycbcr420_full_to_canonical};

pub const WIDTH: usize = 1024;
pub const HEIGHT: usize = 768;
pub const RGB_LEN: usize = WIDTH * HEIGHT * 3;
pub const ANALYSIS_WIDTH: usize = 256;
pub const ANALYSIS_HEIGHT: usize = 192;
pub const SOURCE_WIDTH: usize = 1920;
pub const SOURCE_HEIGHT: usize = 1440;
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_XMP_BYTES: usize = 64 * 1024;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    #[error("unsupported image: {0}")]
    Unsupported(&'static str),
    #[error("invalid image: {0}")]
    Invalid(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Png,
    Jpeg,
}

/// Untrusted XMP lookup metadata, with the exact original XML bytes.
pub struct ExtractedXmp {
    pub model: GalaxyXmp,
    pub packet: Vec<u8>,
}

pub struct DecodedImage {
    pub container: Container,
    /// A container-derived hint only; CAPTURE flags determine the committed profile.
    pub profile_hint: ImageProfile,
    pub raster: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub xmp: Option<ExtractedXmp>,
}

fn container(bytes: &[u8]) -> Result<Container, ImageError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageError::Unsupported("container size"));
    }
    if bytes.starts_with(b"\x89PNG") {
        Ok(Container::Png)
    } else if bytes.starts_with(b"\xff\xd8") {
        Ok(Container::Jpeg)
    } else {
        Err(ImageError::Unsupported("container signature"))
    }
}

pub fn encode(profile: ImageProfile, rgb: &[u8]) -> Result<Vec<u8>, ImageError> {
    if rgb.len() != RGB_LEN {
        return Err(ImageError::Unsupported("RGB raster size"));
    }
    match profile {
        ImageProfile::PngV1 => png_profile::encode(rgb),
        ImageProfile::JpegV1 => jpeg_profile::encode(rgb),
    }
}

/// Strictly validated container bytes with a still-unparsed XMP packet.
struct Parsed<'a> {
    container: Container,
    profile_hint: ImageProfile,
    raster: Vec<u8>,
    packet: Option<&'a [u8]>,
}

fn decode_container(bytes: &[u8]) -> Result<Parsed<'_>, ImageError> {
    let container = container(bytes)?;
    let (profile_hint, raster, packet) = match container {
        Container::Png => {
            let (raster, packet) = png_profile::decode(bytes)?;
            (ImageProfile::PngV1, raster, packet)
        }
        Container::Jpeg => {
            // The strict marker walk runs before the decoder sees any byte.
            let layout = jpeg_profile::inspect(bytes)?;
            (ImageProfile::JpegV1, jpeg_profile::decode(bytes)?, layout.packet)
        }
    };
    Ok(Parsed { container, profile_hint, raster, packet })
}

impl Parsed<'_> {
    fn into_image(self, xmp: Option<ExtractedXmp>) -> DecodedImage {
        DecodedImage {
            container: self.container,
            profile_hint: self.profile_hint,
            raster: self.raster,
            width: WIDTH as u32,
            height: HEIGHT as u32,
            xmp,
        }
    }
}

/// Strict decode: a present XMP packet must be valid Galaxy XMP.
pub fn decode_any(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    let decoded = decode_container(bytes)?;
    let xmp = decoded.packet.map(decode_packet).transpose()?;
    Ok(decoded.into_image(xmp))
}

/// Container-strict decode where an unparsable XMP packet is reported separately instead of
/// failing the decode (for verification by manually supplied transaction ids). Duplicate,
/// oversized or misplaced XMP still fails container validation. `DecodedImage::xmp` is `None`.
#[allow(clippy::type_complexity)]
pub fn decode_with_xmp_status(
    bytes: &[u8],
) -> Result<(DecodedImage, Result<Option<ExtractedXmp>, ImageError>), ImageError> {
    let decoded = decode_container(bytes)?;
    let xmp = decoded.packet.map(decode_packet).transpose();
    Ok((decoded.into_image(None), xmp))
}

pub fn extract_xmp(bytes: &[u8]) -> Result<Option<ExtractedXmp>, ImageError> {
    Ok(decode_any(bytes)?.xmp)
}

/// Embed a serialized Galaxy XMP model; see [`embed_xmp_packet`].
pub fn embed_xmp(bytes: &[u8], xmp: &GalaxyXmp) -> Result<Vec<u8>, ImageError> {
    let packet = xmp.to_packet().map_err(|_| ImageError::Invalid("Galaxy XMP"))?;
    embed_xmp_packet(bytes, &packet)
}

/// Insert one XMP packet (PNG iTXt before the first IDAT, JPEG APP1 after SOI/APP0).
/// Image data bytes (IDAT chunks, entropy-coded scan) are copied unchanged.
pub fn embed_xmp_packet(bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ImageError> {
    decode_packet(packet)?;
    let decoded = decode_container(bytes)?;
    if decoded.packet.is_some() {
        return Err(ImageError::Invalid("duplicate XMP"));
    }
    match decoded.container {
        Container::Png => png_profile::embed(bytes, packet),
        Container::Jpeg => jpeg_profile::embed(bytes, packet),
    }
}

fn decode_packet(packet: &[u8]) -> Result<ExtractedXmp, ImageError> {
    if packet.len() > MAX_XMP_BYTES {
        return Err(ImageError::Unsupported("XMP size"));
    }
    let model = GalaxyXmp::parse_packet(packet).map_err(|_| ImageError::Invalid("Galaxy XMP"))?;
    Ok(ExtractedXmp { model, packet: packet.to_vec() })
}

pub fn photo_id(rgb: &[u8]) -> Result<Hash32, ImageError> {
    if rgb.len() != RGB_LEN {
        return Err(ImageError::Unsupported("RGB raster size"));
    }
    Ok(ids::photo_id(WIDTH as u32, HEIGHT as u32, rgb))
}

pub fn rgb_root_from_raster(rgb: &[u8]) -> Result<Fr, ImageError> {
    let analysis = analysis_raster(rgb)?;
    Ok(FrameTree::build(TreeKind::Rgb, 1, &analysis, &[], &[])
        .map_err(|_| ImageError::Invalid("RGB commitment data"))?
        .root())
}

/// Capture profile recorded in CAPTURE flags; not necessarily the presented container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum ImageProfile {
    #[serde(rename = "png-v1")]
    PngV1 = 0,
    #[serde(rename = "jpeg-v1")]
    JpegV1 = 1,
}

impl ImageProfile {
    pub const fn code(self) -> u8 {
        self as u8
    }
}
