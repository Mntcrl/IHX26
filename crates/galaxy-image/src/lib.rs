//! Image profiles `png-v1` and `jpeg-v1`: strict decoding to the canonical 1024×768 RGB8 raster, YCbCr→RGB conversion, exact box downscale, 256×192 analysis raster, `photo_id`, and XMP embed/extract (spec §4, §5).

mod jpeg_profile;
mod png_profile;
mod raster;

use std::ops::Range;

use galaxy_crypto::{tree::{FrameTree, TreeKind}, Fr};
use galaxy_protocol::{ids, xmp::{GalaxyXmp, XmpError, MAX_PACKET_LEN}, Hash32};
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
const MAX_XMP_BYTES: usize = MAX_PACKET_LEN;

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
        ImageProfile::PngV1 => png_profile::encode(rgb, None),
        ImageProfile::JpegV1 => jpeg_profile::encode(rgb),
    }
}

/// An insertion point and, when present, the exact bytes of the metadata it replaces.
struct XmpLayout<'a> {
    insert_at: usize,
    existing: Option<(&'a [u8], Range<usize>)>,
}

impl XmpLayout<'_> {
    fn packet(&self) -> Option<&[u8]> {
        self.existing.as_ref().map(|(packet, _)| *packet)
    }

    fn splice(
        &self, bytes: &[u8], metadata_len: usize, write: impl FnOnce(&mut Vec<u8>),
    ) -> Result<Vec<u8>, ImageError> {
        let at = self.insert_at;
        let remove = self.existing.as_ref().map_or(at..at, |(_, range)| range.clone());
        let size = bytes.len() - remove.len() + metadata_len;
        if size > MAX_IMAGE_BYTES {
            return Err(ImageError::Unsupported("container size"));
        }
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(&bytes[..at]);
        write(&mut out);
        out.extend_from_slice(&bytes[at..remove.start]);
        out.extend_from_slice(&bytes[remove.end..]);
        Ok(out)
    }
}

/// Strictly validated container bytes with a still-unparsed XMP packet.
struct Parsed<'a> {
    container: Container,
    profile_hint: ImageProfile,
    raster: Vec<u8>,
    xmp: XmpLayout<'a>,
}

fn decode_container(bytes: &[u8]) -> Result<Parsed<'_>, ImageError> {
    let container = container(bytes)?;
    let (profile_hint, raster, xmp) = match container {
        Container::Png => {
            let (raster, layout) = png_profile::decode(bytes)?;
            (ImageProfile::PngV1, raster, layout)
        }
        Container::Jpeg => {
            // The strict marker walk runs before the decoder sees any byte.
            let layout = jpeg_profile::inspect(bytes)?;
            (ImageProfile::JpegV1, jpeg_profile::decode(bytes)?, layout)
        }
    };
    Ok(Parsed { container, profile_hint, raster, xmp })
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

    fn bound_xmp(&self) -> Result<(&[u8], GalaxyXmp), ImageError> {
        let packet = self.xmp.packet().ok_or(ImageError::Invalid("missing Galaxy XMP"))?;
        let model = parse_packet(packet)?;
        if photo_id(&self.raster)? != model.photo_id {
            return Err(ImageError::Invalid("Galaxy XMP photo binding"));
        }
        Ok((packet, model))
    }

    fn write_xmp(&self, bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ImageError> {
        match self.container {
            Container::Png => png_profile::write_xmp(bytes, packet, &self.xmp),
            Container::Jpeg => jpeg_profile::write_xmp(bytes, packet, &self.xmp),
        }
    }
}

/// Strict decode: a present XMP packet must be valid Galaxy XMP.
pub fn decode_any(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    let decoded = decode_container(bytes)?;
    let xmp = decoded.xmp.packet().map(decode_packet).transpose()?;
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
    let xmp = decoded.xmp.packet().map(decode_packet).transpose();
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
    parse_packet(packet)?;
    let decoded = decode_container(bytes)?;
    if decoded.xmp.packet().is_some() {
        return Err(ImageError::Invalid("duplicate XMP"));
    }
    decoded.write_xmp(bytes, packet)
}

/// Read the lookup references and check their photo ID against the canonical raster.
/// This checks neither XRPL transaction nor proof validity.
pub fn bound_xmp(bytes: &[u8]) -> Result<GalaxyXmp, ImageError> {
    decode_container(bytes)?.bound_xmp().map(|(_, model)| model)
}

/// Replace the one XMP packet of a photo-bound image without changing any lookup
/// reference or raster bytes. This does not contact XRPL or verify the proof.
pub fn repackage_bound_xmp(
    bytes: &[u8], build_packet: impl FnOnce(&GalaxyXmp) -> Result<Vec<u8>, XmpError>,
) -> Result<Vec<u8>, ImageError> {
    let decoded = decode_container(bytes)?;
    let (_, model) = decoded.bound_xmp()?;
    let packet = build_packet(&model).map_err(|_| ImageError::Invalid("export XMP"))?;
    if parse_packet(&packet)? != model {
        return Err(ImageError::Invalid("changed Galaxy XMP references"));
    }
    decoded.write_xmp(bytes, &packet)
}

/// Encode a photo-bound jpeg-v1 raster as PNG, preserving the exact XMP packet.
pub fn lossless_png_from_bound_jpeg(bytes: &[u8]) -> Result<Vec<u8>, ImageError> {
    if container(bytes)? != Container::Jpeg {
        return Err(ImageError::Invalid("expected JPEG"));
    }
    let decoded = decode_container(bytes)?;
    let (packet, model) = decoded.bound_xmp()?;
    if model.image_profile != "jpeg-v1" {
        return Err(ImageError::Invalid("expected jpeg-v1 commitment"));
    }
    let packet = std::str::from_utf8(packet).map_err(|_| ImageError::Invalid("Galaxy XMP"))?;
    png_profile::encode(&decoded.raster, Some(packet))
}

fn parse_packet(packet: &[u8]) -> Result<GalaxyXmp, ImageError> {
    if packet.len() > MAX_XMP_BYTES {
        return Err(ImageError::Unsupported("XMP size"));
    }
    GalaxyXmp::parse_packet(packet).map_err(|_| ImageError::Invalid("Galaxy XMP"))
}

fn decode_packet(packet: &[u8]) -> Result<ExtractedXmp, ImageError> {
    let model = parse_packet(packet)?;
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
