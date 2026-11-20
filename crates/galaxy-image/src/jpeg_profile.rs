use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

use crate::{ImageError, HEIGHT, MAX_IMAGE_BYTES, MAX_XMP_BYTES, RGB_LEN, WIDTH};
use ImageError::{Invalid, Unsupported};

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXTENDED_XMP_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

pub(crate) fn encode(rgb: &[u8]) -> Result<Vec<u8>, ImageError> {
    let mut bytes = Vec::new();
    let mut encoder = Encoder::new(&mut bytes, 92);
    encoder.set_sampling_factor(SamplingFactor::F_1_1);
    encoder.set_progressive(false);
    encoder.set_optimized_huffman_tables(false);
    encoder.set_restart_interval(0);
    encoder.encode(rgb, WIDTH as u16, HEIGHT as u16, ColorType::Rgb)
        .map_err(|_| Invalid("JPEG encoder raster"))?;
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<u8>, ImageError> {
    // Caller must inspect the marker layout before handing bytes to the decoder.
    let options = DecoderOptions::default()
        .set_use_unsafe(false)
        .set_strict_mode(true)
        .set_max_width(WIDTH)
        .set_max_height(HEIGHT)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(bytes, options);
    let mut rgb = vec![0; RGB_LEN];
    decoder.decode_into(&mut rgb).map_err(|_| Invalid("JPEG entropy decode"))?;
    Ok(rgb)
}

pub(crate) struct Layout<'a> {
    pub(crate) packet: Option<&'a [u8]>,
    insert_at: usize,
}

fn segment(bytes: &[u8]) -> Result<(&[u8], &[u8]), ImageError> {
    let (length, body) = bytes.split_first_chunk::<2>().ok_or(Invalid("JPEG segment length"))?;
    let length = usize::from(u16::from_be_bytes(*length)).checked_sub(2)
        .ok_or(Invalid("JPEG segment length"))?;
    body.split_at_checked(length).ok_or(Invalid("truncated JPEG segment"))
}

fn marker(bytes: &[u8]) -> Result<(u8, &[u8]), ImageError> {
    let rest = bytes.strip_prefix(b"\xff").ok_or(Invalid("JPEG marker prefix"))?;
    let index = rest.iter().position(|&b| b != 0xff).ok_or(Invalid("truncated JPEG marker"))?;
    if rest[index] == 0 {
        return Err(Invalid("JPEG stuffed byte outside scan"));
    }
    Ok((rest[index], &rest[index + 1..]))
}

fn frame(data: &[u8]) -> Result<[u8; 3], ImageError> {
    let (&[precision, h0, h1, w0, w1, n], refs) = data.split_first_chunk::<6>()
        .ok_or(Invalid("JPEG SOF0 length"))?;
    if refs.len() != usize::from(n) * 3 || n == 0 {
        return Err(Invalid("JPEG SOF0 components"));
    }
    if (precision, u16::from_be_bytes([w0, w1]), u16::from_be_bytes([h0, h1]), n)
        != (8, WIDTH as u16, HEIGHT as u16, 3)
    {
        return Err(Unsupported("JPEG SOF0 profile"));
    }
    let mut ids = [0; 3];
    for (i, component) in refs.chunks_exact(3).enumerate() {
        if ids[..i].contains(&component[0]) || component[2] > 3 {
            return Err(Invalid("JPEG component identifier or table"));
        }
        if component[1] != 0x11 {
            return Err(Unsupported("JPEG chroma subsampling"));
        }
        ids[i] = component[0];
    }
    Ok(ids)
}

fn scan(data: &[u8], ids: [u8; 3]) -> Result<(), ImageError> {
    let (&n, components) = data.split_first().ok_or(Invalid("JPEG SOS length"))?;
    if components.len() != usize::from(n) * 2 + 3 || !(1..=3).contains(&n) {
        return Err(Invalid("JPEG SOS components"));
    }
    if n != 3 {
        return Err(Unsupported("non-interleaved JPEG scan"));
    }
    let mut seen = 0;
    for channel in components[..6].chunks_exact(2) {
        let pos = ids.iter().position(|&id| id == channel[0]).ok_or(Invalid("JPEG SOS selector"))?;
        if seen & (1 << pos) != 0 || channel[1] >> 4 > 1 || channel[1] & 15 > 1 {
            return Err(Invalid("JPEG SOS selector or Huffman table"));
        }
        seen |= 1 << pos;
    }
    if components[6..] != [0, 63, 0] {
        return Err(Unsupported("JPEG scan parameters"));
    }
    Ok(())
}

fn end_of_scan(mut bytes: &[u8]) -> Result<&[u8], ImageError> {
    loop {
        let i = bytes.iter().position(|&b| b == 0xff).ok_or(Invalid("JPEG missing EOI"))?;
        let marker = &bytes[i..];
        let non_ff = marker.iter().position(|&b| b != 0xff).ok_or(Invalid("truncated JPEG scan marker"))?;
        match marker[non_ff] {
            0 if non_ff == 1 => bytes = &marker[non_ff + 1..],
            0 => return Err(Invalid("JPEG malformed stuffed byte")),
            0xd0..=0xd7 => return Err(Unsupported("JPEG restart markers")),
            _ => return Ok(marker),
        }
    }
}

fn quant_tables(mut data: &[u8]) -> Result<(), ImageError> {
    if data.is_empty() {
        return Err(Invalid("JPEG empty DQT"));
    }
    while let Some((&table, rest)) = data.split_first() {
        if table >> 4 != 0 {
            return Err(Unsupported("JPEG quantization precision"));
        }
        let (entries, rest) = rest.split_first_chunk::<64>().ok_or(Invalid("JPEG DQT length"))?;
        if table & 15 > 3 || entries.contains(&0) {
            return Err(Invalid("JPEG DQT entries"));
        }
        data = rest;
    }
    Ok(())
}

fn huffman_tables(mut data: &[u8]) -> Result<(), ImageError> {
    if data.is_empty() {
        return Err(Invalid("JPEG empty DHT"));
    }
    while !data.is_empty() {
        let (head, rest) = data.split_first_chunk::<17>().ok_or(Invalid("JPEG DHT length"))?;
        let class = head[0] >> 4;
        let id = head[0] & 15;
        if class > 1 || id > 1 {
            return Err(Unsupported("JPEG Huffman table class or identifier"));
        }
        let mut remaining = 1i32;
        let mut count = 0usize;
        for &width in &head[1..] {
            remaining = remaining * 2 - i32::from(width);
            if remaining <= 0 {
                return Err(Invalid("oversubscribed JPEG DHT"));
            }
            count += usize::from(width);
        }
        if !(1..=256).contains(&count) {
            return Err(Invalid("JPEG Huffman symbol count"));
        }
        let (symbols, rest) = rest.split_at_checked(count).ok_or(Invalid("JPEG DHT symbols"))?;
        if symbols.iter().any(|&s| {
            if class == 0 { s > 11 } else { s != 0 && s != 0xf0 && !(1..=10).contains(&(s & 15)) }
        }) {
            return Err(Invalid("JPEG baseline Huffman symbol"));
        }
        data = rest;
    }
    Ok(())
}

fn exif_orientation(tiff: &[u8]) -> Result<(), ImageError> {
    let (endian, _) = tiff.split_first_chunk::<2>().ok_or(Invalid("EXIF TIFF header"))?;
    let le = match endian {
        b"II" => true,
        b"MM" => false,
        _ => return Err(Invalid("EXIF byte order")),
    };
    let u16_at = |at: usize| -> Result<u16, ImageError> {
        let value = *tiff.get(at..).and_then(|bytes| bytes.first_chunk::<2>()).ok_or(Invalid("EXIF field"))?;
        Ok(if le { u16::from_le_bytes(value) } else { u16::from_be_bytes(value) })
    };
    let u32_at = |at: usize| -> Result<u32, ImageError> {
        let value = *tiff.get(at..).and_then(|bytes| bytes.first_chunk::<4>()).ok_or(Invalid("EXIF field"))?;
        Ok(if le { u32::from_le_bytes(value) } else { u32::from_be_bytes(value) })
    };
    if u16_at(2)? != 42 {
        return Err(Invalid("EXIF TIFF magic"));
    }
    let ifd = u32_at(4)? as usize;
    if ifd < 8 || ifd % 2 != 0 {
        return Err(Invalid("EXIF IFD0 offset"));
    }
    let count = usize::from(u16_at(ifd)?);
    // A successful u16_at(ifd) guarantees ifd + 2 is in the packet (max 64 KiB).
    let start = ifd + 2;
    let end = start + 12 * count;
    u32_at(end)?; // next-IFD pointer is structurally present; thumbnail tags are not used.
    let mut orientation = false;
    for at in (start..end).step_by(12) {
        if u16_at(at)? == 0x0112 {
            if orientation || u16_at(at + 2)? != 3 || u32_at(at + 4)? != 1 {
                return Err(Invalid("EXIF orientation field"));
            }
            orientation = true;
            if u16_at(at + 8)? != 1 {
                return Err(Unsupported("EXIF orientation"));
            }
        }
    }
    Ok(())
}

pub(crate) fn inspect(bytes: &[u8]) -> Result<Layout<'_>, ImageError> {
    let mut rest = bytes.strip_prefix(b"\xff\xd8").ok_or(Invalid("JPEG SOI"))?;
    let mut layout = Layout { packet: None, insert_at: 2 };
    let (mut ids, mut scanned, mut adobe, mut exif) = (None, false, false, false);
    loop {
        let start = bytes.len() - rest.len();
        let (code, body) = marker(rest)?;
        match code {
            0xd9 => {
                if !scanned || !body.is_empty() {
                    return Err(Invalid("JPEG EOI or trailing data"));
                }
                return Ok(layout);
            }
            0xd8 => return Err(Invalid("duplicate JPEG SOI")),
            0xd0..=0xd7 => return Err(Unsupported("JPEG restart markers")),
            0x01 => return Err(Unsupported("JPEG TEM")),
            _ => {}
        }
        let (data, next) = segment(body)?;
        rest = next;
        match code {
            0xc0 => {
                if ids.is_some() {
                    return Err(Invalid("duplicate JPEG SOF0"));
                }
                ids = Some(frame(data)?);
            }
            0xc4 => huffman_tables(data)?,
            0xdb => quant_tables(data)?,
            0xdd => {
                if data.len() != 2 {
                    return Err(Invalid("JPEG DRI length"));
                }
                if data != [0, 0] {
                    return Err(Unsupported("JPEG restart interval"));
                }
            }
            0xda => {
                if scanned {
                    return Err(Unsupported("multi-scan JPEG"));
                }
                scan(data, ids.ok_or(Invalid("JPEG SOS before SOF0"))?)?;
                let end = end_of_scan(rest)?;
                if end.len() == rest.len() {
                    return Err(Invalid("empty JPEG scan"));
                }
                rest = end;
                scanned = true;
            }
            0xe0 if start == 2 => layout.insert_at = bytes.len() - rest.len(),
            0xe1 => {
                if let Some(packet) = data.strip_prefix(XMP_HEADER) {
                    if scanned || layout.packet.replace(packet).is_some() {
                        return Err(Invalid("late or duplicate JPEG XMP"));
                    }
                    if packet.len() > MAX_XMP_BYTES {
                        return Err(Unsupported("XMP size"));
                    }
                } else if data.starts_with(EXTENDED_XMP_HEADER) {
                    return Err(Unsupported("extended JPEG XMP"));
                } else if let Some(tiff) = data.strip_prefix(b"Exif\0\0") {
                    if exif || scanned {
                        return Err(Invalid("late or duplicate JPEG EXIF"));
                    }
                    exif = true;
                    exif_orientation(tiff)?;
                }
            }
            0xee if data.starts_with(b"Adobe") => {
                if adobe || scanned || data.len() != 12 {
                    return Err(Invalid("JPEG Adobe marker"));
                }
                adobe = true;
                if data[11] != 1 {
                    return Err(Unsupported("JPEG Adobe transform"));
                }
            }
            0xe0..=0xef | 0xfe => {}
            _ => return Err(Unsupported("JPEG marker/profile")),
        }
    }
}

pub(crate) fn embed(bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ImageError> {
    let layout = inspect(bytes)?;
    let length = u16::try_from(2 + XMP_HEADER.len() + packet.len()).map_err(|_| Unsupported("JPEG XMP size"))?;
    let size = bytes.len() + 2 + usize::from(length);
    if size > MAX_IMAGE_BYTES {
        return Err(Unsupported("container size"));
    }
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&bytes[..layout.insert_at]);
    out.extend_from_slice(b"\xff\xe1");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(XMP_HEADER);
    out.extend_from_slice(packet);
    out.extend_from_slice(&bytes[layout.insert_at..]);
    Ok(out)
}
