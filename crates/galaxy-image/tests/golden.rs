//! jpeg-v1 determinism vectors (spec §4): the decoded raster of every case must hash identically
//! on x86_64 Linux, aarch64-apple-darwin and wasm32-unknown-unknown (`scripts/image-golden.sh`).
//! Regenerate on Linux only with `GALAXY_IMAGE_REGEN=1` after an intentional pin change.

use galaxy_crypto::field::fr_to_dec;
use galaxy_image::{
    decode_any, downscale_rgb, encode, photo_id, rgb_root_from_raster, ycbcr420_full_to_canonical,
    ycbcr420_full_to_rgb, Container, ImageProfile, HEIGHT, RGB_LEN, SOURCE_HEIGHT, SOURCE_WIDTH, WIDTH,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

const PHOTO: &[u8] = include_bytes!("../../../test-vectors/image/cc0-street-photo-1024x768.jpg");

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// xorshift64* with a fixed seed: a platform-independent pseudo-random byte stream.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8
    }
}

fn image(mut pixel: impl FnMut(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(RGB_LEN);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            rgb.extend_from_slice(&pixel(x, y));
        }
    }
    rgb
}

/// Deterministic 1920×1440 420f planes run through the sensor ingest conversion and box filter.
fn sensor_scene() -> Vec<u8> {
    let mut rng = Rng(0x420f_0000_0001);
    let mut y = vec![0u8; SOURCE_WIDTH * SOURCE_HEIGHT];
    for (i, luma) in y.iter_mut().enumerate() {
        let (px, py) = (i % SOURCE_WIDTH, i / SOURCE_WIDTH);
        let base = 16 + (px * 200 / SOURCE_WIDTH) as i32 + if (px / 97 + py / 61) % 2 == 0 { 20 } else { -10 };
        *luma = (base + i32::from(rng.next() % 9) - 4).clamp(0, 255) as u8;
    }
    let mut cbcr = vec![0u8; SOURCE_WIDTH * SOURCE_HEIGHT / 2];
    for (i, pair) in cbcr.chunks_exact_mut(2).enumerate() {
        let (cx, cy) = (i % (SOURCE_WIDTH / 2), i / (SOURCE_WIDTH / 2));
        pair[0] = (cx * 255 / (SOURCE_WIDTH / 2 - 1)) as u8;
        pair[1] = (255 - cy * 255 / (SOURCE_HEIGHT / 2 - 1)) as u8;
    }
    let fused = ycbcr420_full_to_canonical(&y, &cbcr).unwrap();
    assert_eq!(fused, downscale_rgb(&ycbcr420_full_to_rgb(&y, &cbcr).unwrap()).unwrap());
    fused
}

fn synthetic_cases() -> Vec<(&'static str, Vec<u8>)> {
    let mut noise = Rng(0x6761_6c61_7879);
    let mut texture = Rng(0x7465_7874_7572);
    let mut rects = Rng(0x7265_6374_7321);
    let boxes: Vec<[usize; 7]> = (0..60)
        .map(|_| {
            let mut v = || usize::from(rects.next());
            [v() * 4, v() * 3, v() + 8, v() + 8, v(), v(), v()]
        })
        .collect();
    vec![
        ("gradient-linear", image(|x, y| [(x / 4) as u8, (y / 3) as u8, ((x + y) / 7) as u8])),
        ("gradient-radial", image(|x, y| {
            let (dx, dy) = (x as i64 - 512, y as i64 - 384);
            let d = ((dx * dx + dy * dy) / 2400) as u8;
            [d, 255 - d, d / 2 + 64]
        })),
        ("noise-uniform", image(|_, _| [noise.next(), noise.next(), noise.next()])),
        ("checker-8-aligned", image(|x, y| if (x / 8 + y / 8) % 2 == 0 { [255; 3] } else { [0; 3] })),
        ("checker-7-misaligned", image(|x, y| if (x / 7 + y / 7) % 2 == 0 { [250, 10, 30] } else { [5, 240, 220] })),
        ("stripes-diagonal", image(|x, y| if (x + 2 * y) % 5 < 2 { [0, 0, 0] } else { [255, 255, 0] })),
        ("color-bars-saturated", image(|x, y| {
            const BARS: [[u8; 3]; 8] = [[255, 255, 255], [255, 255, 0], [0, 255, 255], [0, 255, 0], [255, 0, 255], [255, 0, 0], [0, 0, 255], [0, 0, 0]];
            if y < 600 { BARS[x / 128] } else { [(x % 256) as u8; 3] }
        })),
        ("extremes-alternating", image(|x, y| if (x ^ y) & 1 == 0 { [0, 255, 0] } else { [255, 0, 255] })),
        ("smooth-plus-noise", image(|x, y| {
            let n = i32::from(texture.next() % 17) - 8;
            [(x * 255 / 1023) as i32 + n, (y * 255 / 767) as i32 - n, 128 + n].map(|v| v.clamp(0, 255) as u8)
        })),
        ("rectangles-edges", image(|x, y| {
            boxes.iter().rev()
                .find(|b| (b[0]..b[0] + b[2]).contains(&x) && (b[1]..b[1] + b[3]).contains(&y))
                .map_or([128, 128, 128], |b| [b[4] as u8, b[5] as u8, b[6] as u8])
        })),
        ("sensor-420f-scene", sensor_scene()),
    ]
}

fn case(name: &str, input: &[u8], file_sha256: Option<String>) -> Value {
    let png = encode(ImageProfile::PngV1, input).unwrap();
    let png_decoded = decode_any(&png).unwrap();
    assert_eq!(png_decoded.container, Container::Png);
    assert_eq!(png_decoded.raster, input, "{name}: png-v1 must be lossless");

    let jpeg = encode(ImageProfile::JpegV1, input).unwrap();
    let decoded = decode_any(&jpeg).unwrap();
    assert_eq!((decoded.container, decoded.width, decoded.height), (Container::Jpeg, 1024, 768));
    let mut entry = json!({
        "name": name,
        "input_sha256": sha(input),
        "jpeg_sha256": sha(&jpeg),
        "decoded_sha256": sha(&decoded.raster),
        "photo_id": hex::encode(photo_id(&decoded.raster).unwrap()),
        "rgb_root": fr_to_dec(&rgb_root_from_raster(&decoded.raster).unwrap()),
    });
    if let Some(file) = file_sha256 {
        entry["file_sha256"] = json!(file);
    }
    entry
}

fn compute() -> Value {
    let mut cases: Vec<Value> = synthetic_cases().iter().map(|(name, rgb)| case(name, rgb, None)).collect();
    // A real camera photograph encoded by a third-party (libjpeg) encoder: its decoded raster is
    // the input of a jpeg-v1 re-encode.
    let photo = decode_any(PHOTO).unwrap().raster;
    cases.push(case("cc0-street-photo", &photo, Some(sha(PHOTO))));
    json!({
        "version": 1,
        "decoder": "zune-jpeg =0.4.21 (default-features = false: scalar integer IDCT and color conversion)",
        "encoder": "jpeg-encoder =0.7.1 (quality 92, 4:4:4, baseline, standard Huffman tables, no restart markers)",
        "cases": cases,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn expected() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-vectors/image/jpeg-golden.json");
    if std::env::var_os("GALAXY_IMAGE_REGEN").is_some() {
        let mut text = serde_json::to_string_pretty(&compute()).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[cfg(target_arch = "wasm32")]
fn expected() -> Value {
    serde_json::from_str(include_str!("../../../test-vectors/image/jpeg-golden.json")).unwrap()
}

#[test]
fn jpeg_v1_golden_rasters() {
    let expected = expected();
    let actual = compute();
    let (expected, actual) = (expected["cases"].as_array().unwrap(), actual["cases"].as_array().unwrap());
    assert!(expected.len() >= 12);
    assert_eq!(expected.len(), actual.len());
    for (e, a) in expected.iter().zip(actual) {
        assert_eq!(e, a, "golden mismatch for {}", e["name"]);
    }
}
