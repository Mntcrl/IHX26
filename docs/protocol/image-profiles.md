# Image profiles — implementation notes (`crates/galaxy-image`)

Normative text: spec §4 (profiles, XMP placement) and §5 (sensor conversion, downscale). Decisions: ADR 0005, ADR 0020 (JPEG reference decoder), ADR 0023 (analysis raster alignment).

## Public API

| item | meaning |
|---|---|
| `WIDTH`, `HEIGHT`, `RGB_LEN` | canonical raster 1024 × 768 RGB8 (`RGB_LEN` = 2,359,296 bytes) |
| `ANALYSIS_WIDTH`, `ANALYSIS_HEIGHT` | analysis grid 256 × 192 |
| `SOURCE_WIDTH`, `SOURCE_HEIGHT` | ARKit capturedImage 1920 × 1440 |
| `MAX_IMAGE_BYTES` | 16 MiB cap on any container handed to the decoder |
| `ImageProfile::{PngV1, JpegV1}`, `code()` | profile codes 0 / 1 (CAPTURE flags bits 0–3) |
| `encode(profile, &rgb)` | deterministic encoder of the profile, no metadata |
| `decode_any(&bytes) -> DecodedImage {container, profile_hint, raster, width, height, xmp}` | strict container walk, then reference decode; a present XMP packet must parse |
| `decode_with_xmp_status(&bytes)` | as `decode_any`, but an unparsable (well-placed) XMP packet is returned as a separate `Err` instead of failing the decode |
| `extract_xmp`, `embed_xmp`, `embed_xmp_packet` | XMP lookup hints (untrusted), see below |
| `ycbcr420_full_to_rgb`, `downscale_rgb`, `ycbcr420_full_to_canonical` | §5 integer conversion and exact 15:8 box filter (the fused form is the production path; equality with the two-step form is tested) |
| `analysis_raster(&rgb)` | `A[u,v] = raster[4u+2, 4v+2]` |
| `photo_id(&rgb)`, `rgb_root_from_raster(&rgb)` | `galaxy_protocol::ids::photo_id(1024, 768, rgb)`; RGB tree root via `galaxy_crypto::tree` (analysis raster of the canonical raster) |

`ImageError` has two classes: `Unsupported(&str)` — well-formed input outside the v1 profiles (wrong dimensions or colour type, disallowed chunk or marker, progressive / multi-scan / restart markers, Adobe transform ≠ 1, EXIF orientation ≠ 1, extended XMP, oversized XMP or container); `Invalid(&str)` — corrupt or contradictory input (bad signature, CRC, zlib/Adler-32, IDAT gaps, duplicates, trailing bytes, entropy-decode failure). Verifiers map `Unsupported` to an unsupported-image result and `Invalid` to an invalid-image result. Dimensions are validated from IHDR / SOF0 before any raster allocation; every decode allocates exactly `RGB_LEN` bytes (plus, for PNG, the 2,360,064-byte filtered buffer).

## png-v1

- Encoder: `png` crate (workspace pin), colour type 2, bit depth 8, no interlace, `Compression::Fast`, `Filter::Paeth`; no ancillary chunks. Only the raster is committed; the encoder output is deterministic for a given `png` version and its hash is recorded in the evidence vectors (`encoded_sha256`).
- Decoder: own chunk walker, then zlib (`flate2` rust backend) and manual unfiltering; the `png` crate is not used for decoding.
  - Signature exact; first chunk IHDR with exactly `1024, 768, 8, 2, 0, 0, 0`.
  - Every chunk CRC checked; chunk length bounded by the remaining input.
  - Allowed ancillary chunks: `sRGB` (once, before IDAT, value 0–3), `pHYs` (once, before IDAT, unit ≤ 1), `tIME` (once, field ranges checked), `tEXt`/`zTXt`/`iTXt`. Every other chunk type (including `iCCP`, `gAMA`, `cHRM`, `sBIT`, `PLTE`, `tRNS`, `bKGD`, APNG chunks and any unknown chunk) → `Unsupported`.
  - IDAT chunks contiguous; IEND last, empty, with no trailing byte; duplicate IHDR → `Invalid`.
  - The zlib stream must inflate to exactly 768 × (1 + 3072) bytes, Adler-32 verified, no trailing compressed data; filter bytes 0–4.
- Any lossless re-encoding of the same raster verifies (only the raster is committed).

## jpeg-v1

- Encoder: `jpeg-encoder =0.7.1`, quality 92, `SamplingFactor::F_1_1` (4:4:4), baseline, standard Huffman tables (`optimized_huffman_tables = false`), restart interval 0, no APPn other than the encoder's JFIF APP0.
- Strict marker walk before decoding: SOI; exactly one SOF0 (8-bit precision, 1024 × 768, 3 components, all sampling factors 1 × 1, quantisation table ids valid); DHT/DQT parsed for structural validity; exactly one SOS covering the three components with Ss = 0, Se = 63, Ah = Al = 0; no RSTn / DRI ≠ 0 / TEM; Adobe APP14 (at most one, length 12) transform = 1; EXIF APP1 (at most one) orientation absent or 1; XMP APP1 at most one, before the scan; extended XMP rejected; EOI last with no trailing bytes. Any other SOFn, DAC, DNL, DHP, EXP → `Unsupported`.
- Reference decoder: `zune-jpeg =0.4.21` with `default-features = false` (scalar integer IDCT, upsampling and YCbCr→RGB; the `x86` and `neon` features stay disabled, checked by `scripts/image-golden.sh`), strict mode, `use_unsafe = false`, maximum dimensions 1024 × 768, RGB output written into a caller-allocated `RGB_LEN` buffer. ADR 0020 records why this decoder replaces `jpeg-decoder`.
- The committed raster of a jpeg-v1 capture is the reference-decoder output of the exported JPEG bytes. A pixel-identical PNG of that raster verifies as well.

### Cross-target determinism evidence

`test-vectors/image/jpeg-golden.json` holds 12 cases (10 synthetic rasters: gradients, radial gradient, uniform noise, aligned / misaligned checkerboards, diagonal stripes, saturated colour bars, alternating extremes, smooth + noise, random rectangles; one 420f sensor scene through the §5 pipeline; one CC0 street photograph `test-vectors/image/cc0-street-photo-1024x768.jpg` encoded by a third-party encoder). Per case: input SHA-256, jpeg-v1 file SHA-256 (encoder determinism), decoded-raster SHA-256, `photo_id`, `rgb_root`. `scripts/image-golden.sh [linux|wasm|mac]` runs the same test (`crates/galaxy-image/tests/golden.rs`) on x86_64 Linux, wasm32-unknown-unknown under node, and aarch64-apple-darwin on the Mac build host.

Result on 2026-11-20: all 12 cases and the evidence golden vector pass on all three targets (x86_64 Linux rustc 1.91.1, wasm32 under node rustc 1.91.1, aarch64-apple-darwin rustc 1.92.0); details in ADR 0020. aarch64-apple-ios builds the same scalar decoder code (no `target_os`-dependent code in the decoder); an on-device run of the golden test has not been performed yet.

## XMP

- Model: `galaxy_protocol::xmp::GalaxyXmp` (ProtocolCrypto). `ExtractedXmp {model, packet}` returns the parsed model and the exact packet bytes; both are untrusted lookup hints.
- PNG: one `iTXt` chunk, keyword `XML:com.adobe.xmp`, compression flag 0, method 0, empty language tag and translated keyword, inserted immediately before the first IDAT. The XMP keyword in `tEXt`/`zTXt`, a second XMP chunk, or XMP after the first IDAT → `Invalid`; a compressed XMP `iTXt` → `Unsupported`.
- JPEG: one APP1 `http://ns.adobe.com/xap/1.0/\0` segment inserted right after SOI/APP0 (directly after SOI if the file has no leading APP0). A second XMP APP1 or one after the scan → `Invalid`; extended XMP → `Unsupported`.
- Packet cap 64 KiB (`Unsupported` above; for JPEG the APP1 segment length additionally limits the packet to 65,504 bytes). Embedding into a file that already carries XMP → `Invalid("duplicate XMP")`; `embed_xmp_packet` refuses a packet that does not parse as Galaxy XMP.
- Embedding never changes image data: tests check that every chunk / segment other than the inserted one (including all IDAT chunks and the entropy-coded scan) is byte-identical before and after embedding, and that the decoded rasters are equal.
