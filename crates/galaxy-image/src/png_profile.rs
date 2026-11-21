use std::ops::Range;

use crc32fast::Hasher;
use flate2::{Decompress, FlushDecompress, Status};

use crate::{ImageError, XmpLayout, HEIGHT, MAX_IMAGE_BYTES, MAX_XMP_BYTES, RGB_LEN, WIDTH};
use ImageError::{Invalid, Unsupported};

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const XMP_KEY: &str = "XML:com.adobe.xmp";
const STRIDE: usize = WIDTH * 3;
const FILTERED_LEN: usize = HEIGHT * (STRIDE + 1);

pub(crate) fn encode(rgb: &[u8], packet: Option<&str>) -> Result<Vec<u8>, ImageError> {
    let mut result = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut result, WIDTH as u32, HEIGHT as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Paeth);
        if let Some(packet) = packet {
            encoder.add_itxt_chunk(XMP_KEY.into(), packet.to_owned())
                .map_err(|_| Invalid("PNG encoder XMP"))?;
        }
        let mut writer = encoder.write_header().map_err(|_| Invalid("PNG encoder header"))?;
        writer.write_image_data(rgb).map_err(|_| Invalid("PNG encoder raster"))?;
        writer.finish().map_err(|_| Invalid("PNG encoder finish"))?;
    }
    Ok(result)
}

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    rest: &'a [u8],
}

fn next_chunk(bytes: &[u8]) -> Result<Chunk<'_>, ImageError> {
    let (length, bytes) = bytes.split_first_chunk::<4>().ok_or(Invalid("PNG chunk length"))?;
    let length = u32::from_be_bytes(*length) as usize;
    if length > MAX_IMAGE_BYTES {
        return Err(Invalid("PNG chunk length"));
    }
    let (kind, bytes) = bytes.split_first_chunk::<4>().ok_or(Invalid("PNG chunk type"))?;
    if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
        return Err(Invalid("PNG chunk type"));
    }
    let (data, bytes) = bytes.split_at_checked(length).ok_or(Invalid("truncated PNG chunk"))?;
    let (crc, rest) = bytes.split_first_chunk::<4>().ok_or(Invalid("PNG chunk CRC"))?;
    let mut hasher = Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    if hasher.finalize() != u32::from_be_bytes(*crc) {
        return Err(Invalid("PNG chunk CRC"));
    }
    Ok(Chunk { kind: *kind, data, rest })
}

fn split_text(data: &[u8]) -> Result<(&[u8], &[u8]), ImageError> {
    let index = data.iter().position(|&c| c == 0).ok_or(Invalid("PNG text delimiter"))?;
    Ok((&data[..index], &data[index + 1..]))
}

fn packet_from_text(kind: [u8; 4], data: &[u8]) -> Result<Option<&[u8]>, ImageError> {
    let (keyword, data) = split_text(data)?;
    if !(1..=79).contains(&keyword.len())
        || keyword[0] == b' '
        || keyword[keyword.len() - 1] == b' '
        || keyword.windows(2).any(|p| p == b"  ")
        || keyword.iter().any(|&b| !matches!(b, 32..=126 | 161..=255))
    {
        return Err(Invalid("PNG text keyword"));
    }
    if keyword == XMP_KEY.as_bytes() && kind != *b"iTXt" {
        return Err(Invalid("XMP must use iTXt"));
    }
    match &kind {
        b"iTXt" => {
            let (&[flag, method], data) = data.split_first_chunk::<2>().ok_or(Invalid("iTXt flags"))?;
            if flag > 1 || method != 0 {
                return Err(Invalid("iTXt compression"));
            }
            let (_, data) = split_text(data)?;
            let (_, text) = split_text(data)?;
            if keyword == XMP_KEY.as_bytes() {
                if flag != 0 {
                    return Err(Unsupported("compressed PNG XMP"));
                }
                if text.len() > MAX_XMP_BYTES {
                    return Err(Unsupported("XMP size"));
                }
                return Ok(Some(text));
            }
        }
        b"zTXt" if data.first() != Some(&0) => return Err(Invalid("zTXt compression")),
        _ => {}
    }
    Ok(None)
}

struct Layout<'a> {
    xmp: XmpLayout<'a>,
    first_idat: usize,
}

fn walk(bytes: &[u8]) -> Result<Layout<'_>, ImageError> {
    let start = bytes.strip_prefix(SIGNATURE).ok_or(Invalid("PNG signature"))?;
    let header = next_chunk(start)?;
    if header.kind != *b"IHDR" || header.data.len() != 13 {
        return Err(Invalid("PNG IHDR"));
    }
    let dim = &header.data[..8];
    if dim != [0, 0, 4, 0, 0, 0, 3, 0] || header.data[8..] != [8, 2, 0, 0, 0] {
        return Err(Unsupported("PNG IHDR profile"));
    }
    let mut existing: Option<(&[u8], Range<usize>)> = None;
    let mut first_idat = None;
    let (mut after_idat, mut srgb, mut phys, mut time) = (false, false, false, false);
    let mut rest = header.rest;
    loop {
        let chunk = next_chunk(rest)?;
        match &chunk.kind {
            b"IDAT" => {
                if after_idat {
                    return Err(Invalid("non-contiguous IDAT"));
                }
                first_idat.get_or_insert(bytes.len() - rest.len());
            }
            b"IEND" => {
                if !chunk.data.is_empty() || !chunk.rest.is_empty() {
                    return Err(Invalid("PNG IEND or trailing data"));
                }
                let first_idat = first_idat.ok_or(Invalid("PNG missing IDAT"))?;
                let insert_at = existing.as_ref().map_or(first_idat, |(_, range)| range.start);
                return Ok(Layout { xmp: XmpLayout { existing, insert_at }, first_idat });
            }
            b"IHDR" => return Err(Invalid("duplicate IHDR")),
            b"sRGB" => {
                if srgb || first_idat.is_some() || !matches!(chunk.data, [0..=3]) {
                    return Err(Invalid("PNG sRGB"));
                }
                srgb = true;
            }
            b"pHYs" => {
                if phys || first_idat.is_some() || chunk.data.len() != 9 || chunk.data[8] > 1 {
                    return Err(Invalid("PNG pHYs"));
                }
                phys = true;
            }
            b"tIME" => {
                if time || !matches!(chunk.data, [_, _, 1..=12, 1..=31, 0..=23, 0..=59, 0..=60]) {
                    return Err(Invalid("PNG tIME"));
                }
                time = true;
            }
            b"tEXt" | b"zTXt" | b"iTXt" => {
                if let Some(text) = packet_from_text(chunk.kind, chunk.data)? {
                    let range = bytes.len() - rest.len()..bytes.len() - chunk.rest.len();
                    if first_idat.is_some() || existing.replace((text, range)).is_some() {
                        return Err(Invalid("late or duplicate PNG XMP"));
                    }
                }
            }
            _ => return Err(Unsupported("PNG chunk type")),
        }
        if chunk.kind != *b"IDAT" && first_idat.is_some() {
            after_idat = true;
        }
        rest = chunk.rest;
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<(Vec<u8>, XmpLayout<'_>), ImageError> {
    // Inspect all chunks before allocating the fixed-size raster or running the inflater.
    let layout = walk(bytes)?;
    let mut inflater = Decompress::new(true);
    let mut filtered = vec![0; FILTERED_LEN + 1];
    let mut finished = false;
    let mut rest = &bytes[layout.first_idat..];
    while let [_, _, _, _, b'I', b'D', b'A', b'T', ..] = rest {
        // The full walk already verified lengths and CRCs.
        let length = u32::from_be_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        let data = &rest[8..8 + length];
        if !data.is_empty() {
            if finished {
                return Err(Invalid("trailing PNG zlib bytes"));
            }
            let before = inflater.total_in();
            let written = inflater.total_out() as usize;
            finished = inflater.decompress(data, &mut filtered[written..], FlushDecompress::None)
                .map_err(|_| Invalid("PNG zlib stream"))? == Status::StreamEnd;
            if inflater.total_out() > FILTERED_LEN as u64 || inflater.total_in() - before != length as u64 {
                return Err(Invalid("PNG decompressed size or trailing zlib bytes"));
            }
        }
        rest = &rest[length + 12..];
    }
    if !finished || inflater.total_out() != FILTERED_LEN as u64 {
        return Err(Invalid("incomplete PNG raster"));
    }
    unfilter(&mut filtered)?;
    filtered.truncate(RGB_LEN);
    Ok((filtered, layout.xmp))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a0, b0, c0) = (i32::from(a), i32::from(b), i32::from(c));
    let (pa, pb, pc) = ((b0 - c0).abs(), (a0 - c0).abs(), (a0 + b0 - 2 * c0).abs());
    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
}

fn unfilter(bytes: &mut [u8]) -> Result<(), ImageError> {
    for row in 0..HEIGHT {
        let src = row * (STRIDE + 1);
        let dst = row * STRIDE;
        let filter = bytes[src];
        if filter > 4 {
            return Err(Invalid("PNG row filter"));
        }
        bytes.copy_within(src + 1..src + 1 + STRIDE, dst);
        if filter == 0 {
            continue;
        }
        let (prior, after) = bytes.split_at_mut(dst);
        let current = &mut after[..STRIDE];
        let previous = prior.get(dst.saturating_sub(STRIDE)..dst).unwrap_or_default();
        for i in 0..STRIDE {
            let left = if i < 3 { 0 } else { current[i - 3] };
            let up = if row == 0 { 0 } else { previous[i] };
            let upper_left = if row == 0 || i < 3 { 0 } else { previous[i - 3] };
            let predict = match filter {
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                _ => paeth(left, up, upper_left),
            };
            current[i] = current[i].wrapping_add(predict);
        }
    }
    Ok(())
}

pub(crate) fn write_xmp(bytes: &[u8], packet: &[u8], layout: &XmpLayout<'_>) -> Result<Vec<u8>, ImageError> {
    let length = XMP_KEY.len() + 5 + packet.len();
    layout.splice(bytes, 12 + length, |out| {
        out.extend_from_slice(&(length as u32).to_be_bytes());
        let crc_start = out.len();
        out.extend_from_slice(b"iTXt");
        out.extend_from_slice(XMP_KEY.as_bytes());
        out.extend_from_slice(&[0; 5]);
        out.extend_from_slice(packet);
        out.extend_from_slice(&crc32fast::hash(&out[crc_start..]).to_be_bytes());
    })
}
