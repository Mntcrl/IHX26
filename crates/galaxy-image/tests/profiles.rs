//! Container strictness, XMP placement and malformed-input tests for png-v1 and jpeg-v1 (spec §4).

use galaxy_image::{
    decode_any, decode_with_xmp_status, embed_xmp, embed_xmp_packet, encode, extract_xmp, Container,
    ImageError, ImageProfile, HEIGHT, RGB_LEN, WIDTH,
};
use galaxy_protocol::xmp::GalaxyXmp;
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

const PHOTO: &[u8] = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");

fn raster() -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..RGB_LEN)
        .map(|i| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let smooth = ((i / 3) % WIDTH / 4 + (i / 3) / WIDTH / 3) as u32;
            (smooth + (state >> 28)) as u8
        })
        .collect()
}

fn xmp() -> GalaxyXmp {
    GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "png-v1".into(),
        photo_id: [0x11; 32],
        capture_id: [0x22; 32],
        manifest_tx: [0x33; 32],
        proof_tx: [0x44; 32],
    }
}

fn unsupported(result: Result<impl Sized, ImageError>) {
    assert!(matches!(result, Err(ImageError::Unsupported(_))), "expected Unsupported");
}

fn invalid(result: Result<impl Sized, ImageError>) {
    assert!(matches!(result, Err(ImageError::Invalid(_))), "expected Invalid");
}

/// Truncations and single-byte mutations must return a result, never panic.
fn no_panic_on_corruption(bytes: &[u8]) {
    let step = (bytes.len() / 150).max(1);
    for cut in (0..bytes.len()).step_by(step) {
        let _ = decode_any(&bytes[..cut]);
    }
    let mut state = 0x9e37_79b9u32;
    let positions = (0..64).chain((0..300).map(|_| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as usize % bytes.len()
    }));
    for at in positions {
        let mut mutated = bytes.to_vec();
        for delta in [1u8, 0x80, 0xff] {
            mutated[at] = bytes[at].wrapping_add(delta);
            let _ = decode_any(&mutated);
        }
    }
}

// ---------------------------------------------------------------- PNG

struct Chunk {
    kind: [u8; 4],
    data: Vec<u8>,
}

fn png_chunks(bytes: &[u8]) -> Vec<Chunk> {
    let mut rest = &bytes[8..];
    let mut chunks = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        chunks.push(Chunk { kind: rest[4..8].try_into().unwrap(), data: rest[8..8 + len].to_vec() });
        rest = &rest[12 + len..];
    }
    chunks
}

fn chunk_bytes(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    out.extend_from_slice(&hasher.finalize().to_be_bytes());
    out
}

fn png_assemble(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for chunk in chunks {
        out.extend(chunk_bytes(&chunk.kind, &chunk.data));
    }
    out
}

/// Insert a chunk before the first chunk of kind `before`.
fn png_insert(bytes: &[u8], before: &[u8; 4], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunks = png_chunks(bytes);
    let at = chunks.iter().position(|c| &c.kind == before).unwrap();
    chunks.insert(at, Chunk { kind: *kind, data: data.to_vec() });
    png_assemble(&chunks)
}

fn png_with(filter: png::Filter, compression: png::Compression, idat_size: usize, rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, WIDTH as u32, HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_filter(filter);
    encoder.set_compression(compression);
    let mut writer = encoder.write_header().unwrap();
    {
        let mut stream = writer.stream_writer_with_size(idat_size).unwrap();
        std::io::Write::write_all(&mut stream, rgb).unwrap();
        stream.finish().unwrap();
    }
    drop(writer);
    out
}

#[test]
fn png_round_trip_and_lossless_reencodings() {
    let rgb = raster();
    let encoded = encode(ImageProfile::PngV1, &rgb).unwrap();
    assert_eq!(encode(ImageProfile::PngV1, &rgb).unwrap(), encoded, "encoder is deterministic");
    let decoded = decode_any(&encoded).unwrap();
    assert_eq!((decoded.container, decoded.profile_hint), (Container::Png, ImageProfile::PngV1));
    assert_eq!(decoded.raster, rgb);
    assert!(decoded.xmp.is_none());
    // Other filters, compression levels and IDAT splits verify as long as the raster is equal.
    for (filter, compression, idat) in [
        (png::Filter::NoFilter, png::Compression::NoCompression, 1 << 20),
        (png::Filter::Sub, png::Compression::Balanced, 8192),
        (png::Filter::Up, png::Compression::High, 1000),
        (png::Filter::Avg, png::Compression::Fastest, 65536),
        (png::Filter::Adaptive, png::Compression::Balanced, 1 << 15),
    ] {
        assert_eq!(decode_any(&png_with(filter, compression, idat, &rgb)).unwrap().raster, rgb);
    }
}

#[test]
fn png_allowed_ancillary_chunks() {
    let encoded = encode(ImageProfile::PngV1, &raster()).unwrap();
    let mut bytes = png_insert(&encoded, b"IDAT", b"sRGB", &[0]);
    bytes = png_insert(&bytes, b"IDAT", b"pHYs", &[0, 0, 11, 19, 0, 0, 11, 19, 1]);
    bytes = png_insert(&bytes, b"IDAT", b"tEXt", b"Comment\0hello");
    bytes = png_insert(&bytes, b"IDAT", b"zTXt", b"Title\0\0\x78\x9c\x03\x00\x00\x00\x00\x01");
    bytes = png_insert(&bytes, b"IEND", b"tIME", &[7, 234, 11, 20, 12, 0, 0]);
    bytes = png_insert(&bytes, b"IEND", b"tEXt", b"Author\0x");
    assert_eq!(decode_any(&bytes).unwrap().raster, raster());
}

#[test]
fn png_rejected_layouts() {
    let encoded = encode(ImageProfile::PngV1, &raster()).unwrap();
    for (kind, data) in [
        (b"gAMA", &[0, 0, 177, 143][..]),
        (b"cHRM", &[0; 32][..]),
        (b"iCCP", b"icc\0\0\x78\x9c\x03\x00\x00\x00\x00\x01"),
        (b"sBIT", &[8, 8, 8][..]),
        (b"PLTE", &[0, 0, 0][..]),
        (b"tRNS", &[0, 0, 0, 0, 0, 0][..]),
        (b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0][..]),
        (b"bKGD", &[0, 0, 0, 0, 0, 0][..]),
        (b"ABCD", &[1][..]),
    ] {
        unsupported(decode_any(&png_insert(&encoded, b"IDAT", kind, data)));
    }
    let chunks = png_chunks(&encoded);
    // IHDR profile fields.
    for (index, value) in [(3, 1u8), (7, 0), (8, 16), (9, 6), (10, 1), (11, 1), (12, 1)] {
        let mut changed = png_chunks(&encoded);
        changed[0].data[index] = value;
        unsupported(decode_any(&png_assemble(&changed)));
    }
    // Duplicate IHDR, CRC, trailing data, IEND payload.
    invalid(decode_any(&png_insert(&encoded, b"IDAT", b"IHDR", &chunks[0].data)));
    let mut bad_crc = encoded.clone();
    let crc_at = 8 + 8 + 13;
    bad_crc[crc_at] ^= 1;
    invalid(decode_any(&bad_crc));
    let mut trailing = encoded.clone();
    trailing.push(0);
    invalid(decode_any(&trailing));
    let mut iend = png_chunks(&encoded);
    iend.last_mut().unwrap().data.push(0);
    invalid(decode_any(&png_assemble(&iend)));
    // Missing IDAT, missing IEND.
    let no_idat: Vec<Chunk> = png_chunks(&encoded).into_iter().filter(|c| &c.kind != b"IDAT").collect();
    invalid(decode_any(&png_assemble(&no_idat)));
    invalid(decode_any(&encoded[..encoded.len() - 12]));
    // Signature.
    let mut signature = encoded.clone();
    signature[7] = 0;
    invalid(decode_any(&signature));
}

#[test]
fn png_idat_contiguity_and_zlib_integrity() {
    let rgb = raster();
    let split = png_with(png::Filter::Sub, png::Compression::Balanced, 4096, &rgb);
    let chunks = png_chunks(&split);
    assert!(chunks.iter().filter(|c| &c.kind == b"IDAT").count() > 2);
    let second_idat = chunks.iter().enumerate().filter(|(_, c)| &c.kind == b"IDAT").nth(1).unwrap().0;
    let mut gap = png_chunks(&split);
    gap.insert(second_idat, Chunk { kind: *b"tEXt", data: b"Comment\0x".to_vec() });
    invalid(decode_any(&png_assemble(&gap)));

    // Adler-32 mismatch at the end of the zlib stream (CRC recomputed).
    let mut adler = png_chunks(&split);
    let last = adler.iter().rposition(|c| &c.kind == b"IDAT").unwrap();
    *adler[last].data.last_mut().unwrap() ^= 1;
    invalid(decode_any(&png_assemble(&adler)));

    // Unknown row filter type 5.
    let mut stored = Vec::new();
    for row in rgb.chunks_exact(WIDTH * 3) {
        stored.push(5);
        stored.extend_from_slice(row);
    }
    let zlib = |filtered: &[u8]| {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, filtered).unwrap();
        encoder.finish().unwrap()
    };
    let mut bad_filter = png_chunks(&split);
    bad_filter.retain(|c| &c.kind != b"IDAT");
    bad_filter.insert(1, Chunk { kind: *b"IDAT", data: zlib(&stored) });
    invalid(decode_any(&png_assemble(&bad_filter)));

    // Decompression bomb: 64 MiB of zeros in a tiny stream stops at the fixed raster size.
    let bomb = zlib(&vec![0; 64 << 20]);
    assert!(bomb.len() < 1 << 20);
    let mut bombed = png_chunks(&split);
    bombed.retain(|c| &c.kind != b"IDAT");
    bombed.insert(1, Chunk { kind: *b"IDAT", data: bomb });
    invalid(decode_any(&png_assemble(&bombed)));

    // One row short.
    let mut short = png_chunks(&split);
    short.retain(|c| &c.kind != b"IDAT");
    let mut filtered = vec![0u8; HEIGHT * (WIDTH * 3 + 1)];
    filtered.truncate(filtered.len() - (WIDTH * 3 + 1));
    short.insert(1, Chunk { kind: *b"IDAT", data: zlib(&filtered) });
    invalid(decode_any(&png_assemble(&short)));
}

#[test]
fn png_size_cap_rejects_before_decoding() {
    let mut huge = encode(ImageProfile::PngV1, &raster()).unwrap();
    huge.resize(galaxy_image::MAX_IMAGE_BYTES + 1, 0);
    unsupported(decode_any(&huge));
}

#[test]
fn png_xmp_embed_extract() {
    let rgb = raster();
    let encoded = encode(ImageProfile::PngV1, &rgb).unwrap();
    let packet = xmp().to_packet().unwrap();
    let embedded = embed_xmp(&encoded, &xmp()).unwrap();
    assert_eq!(embedded, embed_xmp_packet(&encoded, &packet).unwrap());
    let extracted = extract_xmp(&embedded).unwrap().unwrap();
    assert_eq!(extracted.packet, packet);
    assert!(extracted.model == xmp());
    assert_eq!(decode_any(&embedded).unwrap().raster, rgb);
    // Exactly one iTXt chunk before the first IDAT; every other chunk is byte-identical.
    let (before, after) = (png_chunks(&encoded), png_chunks(&embedded));
    let itxt = after.iter().position(|c| &c.kind == b"iTXt").unwrap();
    assert!(itxt < after.iter().position(|c| &c.kind == b"IDAT").unwrap());
    assert!(after[itxt].data.starts_with(b"XML:com.adobe.xmp\0\0\0\0\0"));
    let rest: Vec<_> = after.iter().enumerate().filter(|(i, _)| *i != itxt).map(|(_, c)| (c.kind, &c.data)).collect();
    assert_eq!(rest, before.iter().map(|c| (c.kind, &c.data)).collect::<Vec<_>>());
    // Duplicates and late placement fail; a second embed is refused.
    invalid(embed_xmp_packet(&embedded, &packet));
    let mut itxt_data = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    itxt_data.extend_from_slice(&packet);
    invalid(decode_any(&png_insert(&embedded, b"IDAT", b"iTXt", &itxt_data)));
    invalid(decode_any(&png_insert(&encoded, b"IEND", b"iTXt", &itxt_data)));
    invalid(decode_any(&png_insert(&encoded, b"IDAT", b"tEXt", b"XML:com.adobe.xmp\0x")));
    // Compressed or oversized packets are outside the profile.
    unsupported(decode_any(&png_insert(&encoded, b"IDAT", b"iTXt", b"XML:com.adobe.xmp\0\x01\0\0\0\x78\x9c\x03\x00\x00\x00\x00\x01")));
    let mut big = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    big.resize(big.len() + 64 * 1024 + 1, b' ');
    unsupported(decode_any(&png_insert(&encoded, b"IDAT", b"iTXt", &big)));
    // A non-Galaxy packet: strict decode fails, the status mode reports it separately.
    let mut foreign = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    foreign.extend_from_slice(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"></x:xmpmeta>");
    let foreign = png_insert(&encoded, b"IDAT", b"iTXt", &foreign);
    invalid(decode_any(&foreign));
    let (decoded, status) = decode_with_xmp_status(&foreign).unwrap();
    assert_eq!(decoded.raster, rgb);
    assert!(decoded.xmp.is_none());
    invalid(status);
    let (_, status) = decode_with_xmp_status(&embedded).unwrap();
    assert!(status.unwrap().unwrap().model == xmp());
    invalid(embed_xmp_packet(&encoded, b"not xml"));
}

#[test]
fn png_malformed_input_never_panics() {
    let rgb = raster();
    no_panic_on_corruption(&embed_xmp(&encode(ImageProfile::PngV1, &rgb).unwrap(), &xmp()).unwrap());
    no_panic_on_corruption(&png_with(png::Filter::Adaptive, png::Compression::Balanced, 3000, &rgb));
}

// ---------------------------------------------------------------- JPEG

fn jpeg_with(configure: impl FnOnce(&mut Encoder<&mut Vec<u8>>), width: u16, height: u16, color: ColorType, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, 92);
    encoder.set_sampling_factor(SamplingFactor::F_1_1);
    configure(&mut encoder);
    encoder.encode(data, width, height, color).unwrap();
    out
}

fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    // Header segments only (up to and including SOS); the scan is returned as marker 0.
    let mut out = Vec::new();
    let mut at = 2;
    loop {
        let code = bytes[at + 1];
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        out.push((code, bytes[at + 4..at + 2 + len].to_vec()));
        at += 2 + len;
        if code == 0xda {
            out.push((0, bytes[at..].to_vec()));
            return out;
        }
    }
}

fn jpeg_assemble(segments: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"\xff\xd8".to_vec();
    for (code, data) in segments {
        if *code == 0 {
            out.extend_from_slice(data);
        } else {
            out.extend_from_slice(&[0xff, *code]);
            out.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
            out.extend_from_slice(data);
        }
    }
    out
}

fn jpeg_insert_after_app0(bytes: &[u8], code: u8, data: &[u8]) -> Vec<u8> {
    let mut segments = jpeg_segments(bytes);
    segments.insert(1, (code, data.to_vec()));
    jpeg_assemble(&segments)
}

fn exif_orientation(value: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
    exif.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    exif.extend_from_slice(&value.to_be_bytes());
    exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    exif
}

fn adobe(transform: u8) -> Vec<u8> {
    let mut data = b"Adobe\0\x64\0\0\0\0".to_vec();
    data.push(transform);
    data
}

#[test]
fn jpeg_round_trip() {
    let rgb = raster();
    let encoded = encode(ImageProfile::JpegV1, &rgb).unwrap();
    assert_eq!(encode(ImageProfile::JpegV1, &rgb).unwrap(), encoded, "encoder is deterministic");
    let decoded = decode_any(&encoded).unwrap();
    assert_eq!((decoded.container, decoded.profile_hint), (Container::Jpeg, ImageProfile::JpegV1));
    assert_eq!(decoded.raster.len(), RGB_LEN);
    let error: u64 = decoded.raster.iter().zip(&rgb).map(|(&a, &b)| u64::from(a.abs_diff(b))).sum();
    assert!(error / (RGB_LEN as u64) < 4, "mean absolute error {}", error as f64 / RGB_LEN as f64);
    assert_eq!(decode_any(&encoded).unwrap().raster, decoded.raster, "decoder is deterministic");
    // The encoder emits no EXIF/ICC; only SOI, APP0, DQT, SOF0, DHT, SOS.
    let codes: Vec<u8> = jpeg_segments(&encoded).iter().map(|s| s.0).collect();
    assert!(codes.iter().all(|c| matches!(c, 0xe0 | 0xdb | 0xc0 | 0xc4 | 0xda | 0)), "{codes:x?}");
    // A third-party (libjpeg) baseline 4:4:4 photograph is accepted.
    assert_eq!(decode_any(PHOTO).unwrap().profile_hint, ImageProfile::JpegV1);
}

#[test]
fn jpeg_accepted_variant_set() {
    let rgb = raster();
    let encoded = encode(ImageProfile::JpegV1, &rgb).unwrap();
    let reference = decode_any(&encoded).unwrap().raster;
    // EXIF orientation 1 and Adobe transform 1 are inside the set and change no pixel.
    assert_eq!(decode_any(&jpeg_insert_after_app0(&encoded, 0xe1, &exif_orientation(1))).unwrap().raster, reference);
    assert_eq!(decode_any(&jpeg_insert_after_app0(&encoded, 0xee, &adobe(1))).unwrap().raster, reference);
    assert_eq!(decode_any(&jpeg_insert_after_app0(&encoded, 0xfe, b"comment")).unwrap().raster, reference);

    unsupported(decode_any(&jpeg_insert_after_app0(&encoded, 0xe1, &exif_orientation(6))));
    unsupported(decode_any(&jpeg_insert_after_app0(&encoded, 0xee, &adobe(0))));
    unsupported(decode_any(&jpeg_insert_after_app0(&encoded, 0xee, &adobe(2))));
    invalid(decode_any(&jpeg_insert_after_app0(&jpeg_insert_after_app0(&encoded, 0xee, &adobe(1)), 0xee, &adobe(1))));
    unsupported(decode_any(&jpeg_with(|e| e.set_progressive(true), 1024, 768, ColorType::Rgb, &rgb)));
    unsupported(decode_any(&jpeg_with(|e| e.set_sampling_factor(SamplingFactor::F_2_2), 1024, 768, ColorType::Rgb, &rgb)));
    unsupported(decode_any(&jpeg_with(|e| e.set_sampling_factor(SamplingFactor::F_2_1), 1024, 768, ColorType::Rgb, &rgb)));
    unsupported(decode_any(&jpeg_with(|e| e.set_restart_interval(8), 1024, 768, ColorType::Rgb, &rgb)));
    unsupported(decode_any(&jpeg_with(|_| {}, 1024, 767, ColorType::Rgb, &rgb[..1024 * 767 * 3])));
    unsupported(decode_any(&jpeg_with(|_| {}, 768, 1024, ColorType::Rgb, &rgb)));
    unsupported(decode_any(&jpeg_with(|_| {}, 1024, 768, ColorType::Luma, &rgb[..1024 * 768])));
    let rgba: Vec<u8> = rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    unsupported(decode_any(&jpeg_with(|_| {}, 1024, 768, ColorType::Cmyk, &rgba)));
    // SOF variants: extended (C1), progressive (C2), lossless (C3), arithmetic (C9); 12-bit precision.
    let segments = jpeg_segments(&encoded);
    let sof = segments.iter().position(|s| s.0 == 0xc0).unwrap();
    for code in [0xc1, 0xc2, 0xc3, 0xc9] {
        let mut changed = segments.clone();
        changed[sof].0 = code;
        unsupported(decode_any(&jpeg_assemble(&changed)));
    }
    let mut twelve_bit = segments.clone();
    twelve_bit[sof].1[0] = 12;
    unsupported(decode_any(&jpeg_assemble(&twelve_bit)));
    // Non-zero restart interval via DRI and a restart marker inside the scan.
    let mut dri = segments.clone();
    dri.insert(sof, (0xdd, vec![0, 4]));
    unsupported(decode_any(&jpeg_assemble(&dri)));
    let mut zero_dri = segments.clone();
    zero_dri.insert(sof, (0xdd, vec![0, 0]));
    assert_eq!(decode_any(&jpeg_assemble(&zero_dri)).unwrap().raster, reference);
    // Arithmetic conditioning table, DNL, and extended XMP are outside the profile.
    unsupported(decode_any(&jpeg_insert_after_app0(&encoded, 0xcc, &[0x00, 0x10])));
    unsupported(decode_any(&jpeg_insert_after_app0(&encoded, 0xe1, b"http://ns.adobe.com/xmp/extension/\0x")));
}

#[test]
fn jpeg_structural_errors_are_invalid() {
    let encoded = encode(ImageProfile::JpegV1, &raster()).unwrap();
    let mut trailing = encoded.clone();
    trailing.push(0);
    invalid(decode_any(&trailing));
    invalid(decode_any(&encoded[..encoded.len() - 2]));
    invalid(decode_any(&encoded[..encoded.len() / 2]));
    let mut eoi_twice = encoded.clone();
    eoi_twice.extend_from_slice(b"\xff\xd9");
    invalid(decode_any(&eoi_twice));
    let segments = jpeg_segments(&encoded);
    let sos = segments.iter().position(|s| s.0 == 0xda).unwrap();
    let without = |code: u8| jpeg_assemble(&segments.iter().filter(|s| s.0 != code).cloned().collect::<Vec<_>>());
    invalid(decode_any(&without(0xc0)));
    invalid(decode_any(&without(0xc4)));
    invalid(decode_any(&without(0xdb)));
    // Scan referencing an unknown component, SOS before SOF0, duplicate SOF0.
    let mut selector = segments.clone();
    selector[sos].1[1] = 9;
    invalid(decode_any(&jpeg_assemble(&selector)));
    let mut early = segments.clone();
    let sof = early.iter().position(|s| s.0 == 0xc0).unwrap();
    let frame = early.remove(sof);
    early.insert(sos - 1, frame.clone());
    early.insert(sos - 1, frame);
    invalid(decode_any(&jpeg_assemble(&early)));
    // Zero quantizer entry and oversubscribed Huffman table.
    let mut dqt = segments.clone();
    let q = dqt.iter().position(|s| s.0 == 0xdb).unwrap();
    dqt[q].1[1] = 0;
    invalid(decode_any(&jpeg_assemble(&dqt)));
    let mut dht = segments.clone();
    let h = dht.iter().position(|s| s.0 == 0xc4).unwrap();
    dht[h].1[1] = 3;
    invalid(decode_any(&jpeg_assemble(&dht)));
    // Late XMP after the scan.
    let mut late = encoded[..encoded.len() - 2].to_vec();
    let packet = xmp().to_packet().unwrap();
    late.extend_from_slice(b"\xff\xe1");
    late.extend_from_slice(&((packet.len() + 31) as u16).to_be_bytes());
    late.extend_from_slice(b"http://ns.adobe.com/xap/1.0/\0");
    late.extend_from_slice(&packet);
    late.extend_from_slice(b"\xff\xd9");
    invalid(decode_any(&late));
}

#[test]
fn jpeg_xmp_embed_preserves_scan() {
    let rgb = raster();
    let encoded = encode(ImageProfile::JpegV1, &rgb).unwrap();
    let mut model = xmp();
    model.image_profile = "jpeg-v1".into();
    let embedded = embed_xmp(&encoded, &model).unwrap();
    assert_eq!(decode_any(&embedded).unwrap().raster, decode_any(&encoded).unwrap().raster);
    let extracted = extract_xmp(&embedded).unwrap().unwrap();
    assert!(extracted.model == model);
    let (before, after) = (jpeg_segments(&encoded), jpeg_segments(&embedded));
    assert_eq!(after[0].0, 0xe0, "APP0 stays first");
    assert_eq!(after[1].0, 0xe1);
    assert!(after[1].1.starts_with(b"http://ns.adobe.com/xap/1.0/\0"));
    let rest: Vec<_> = after.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, s)| s.clone()).collect();
    assert_eq!(rest, before, "every other segment and the entropy-coded scan are byte-identical");
    invalid(embed_xmp(&embedded, &model));
    // Without APP0 the packet follows SOI directly.
    let no_app0 = jpeg_assemble(&before[1..]);
    let embedded = embed_xmp(&no_app0, &model).unwrap();
    assert_eq!(&embedded[..4], b"\xff\xd8\xff\xe1");
    let (_, status) = decode_with_xmp_status(&embedded).unwrap();
    assert!(status.unwrap().unwrap().model == model);
    let foreign = jpeg_insert_after_app0(&encoded, 0xe1, b"http://ns.adobe.com/xap/1.0/\0<x/>");
    invalid(decode_any(&foreign));
    assert!(decode_with_xmp_status(&foreign).unwrap().1.is_err());
}

#[test]
fn jpeg_malformed_input_never_panics() {
    let rgb = raster();
    no_panic_on_corruption(&encode(ImageProfile::JpegV1, &rgb).unwrap());
    no_panic_on_corruption(PHOTO);
}

#[test]
fn container_signature_and_size() {
    unsupported(decode_any(b"GIF89a"));
    unsupported(decode_any(b""));
    unsupported(encode(ImageProfile::PngV1, &[0; 3]));
    unsupported(encode(ImageProfile::JpegV1, &[0; 3]));
}
