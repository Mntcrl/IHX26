# Evidence v1 — implementation notes (`crates/galaxy-evidence`)

Normative text: spec §5 (model, quantization), §6 (commitments), §21 (GXRW), §22 (API shapes). Decisions: ADR 0021 (ingest quantization), ADR 0022 (BLUETOOTH / NOISE / META layouts), ADR 0023 (analysis raster alignment). GXRW reader notes: `raw-capture-v1.md`.

## Public API

- Model types exactly as §22: `FrameData`, `CameraFrame`, `ImuSample`, `ImuEvidence`, `CanonicalImage`, `ImageProfile` (re-exported from galaxy-image), `Evidence`, `BluetoothAggregate`, `NoiseAggregate`, `Meta`, `Salts`. Model types holding sensor data, and `Salts`, intentionally do not implement `Debug`.
- Constants: `IMU_MAX = 64`, `CAMERA_ELEMENTS = 54`, `IMU_ELEMENTS = 133`, `BLUETOOTH_ELEMENTS = 24`, `NOISE_ELEMENTS = 18`, `META_ELEMENTS = 9`.
- `Evidence::validate() -> Result<(), ValidationError>`: every range of this document (frame sizes, conf ≤ 2, frame-1 analysis raster = sampled canonical raster, intrinsics in [0, 2^26), R/T in i32, ts < 2^48 strictly increasing, track ≤ 2, IMU windows of exactly 64 with padding only at the end, padding samples all zero, Σdt per window ≤ frame interval, aggregates, META consistency).
- `ingest::from_gxrw(&[u8], ImageProfile) -> Result<Evidence, IngestError>`.
- `component_elements(&Evidence) -> [Vec<Fr>; 5]` (CAMERA, IMU, BLUETOOTH, NOISE, META; slots 4–8).
- `commit(&Evidence, &Salts) -> Commitments {rgb_tree, depth_tree, minus_tree, plus_tree, components: [Fr; 16], rgb_root, evidence_root}`; `try_commit` returns `ValidationError` instead of panicking for hand-built evidence.
- `Salts::random()`: 8 × 31 bytes from the OS CSPRNG (`getrandom`; `wasm_js` backend on wasm32), each read as a 248-bit big-endian integer → always canonical.
- `bundle::{EvidenceBundle {evidence, salts, capture_state}, encode, decode, BundleError, MAX_CAPTURE_STATE_BYTES}`.
- `synthetic::{SyntheticCapture, assemble}`: deterministic GXRW writer for tests and vectors (not a device substitute).

## Component element layouts (version 1)

Negative values enter the field as `x mod r` (`fr_from_i128`). All counts are fixed; the circuit only hashes these elements (spec §6).

### CAMERA (slot 4, 54 elements) and IMU (slot 5, 133 elements)
As spec §6. IMU packing: `dt·2^96 + (ωx+2^31)·2^64 + (ωy+2^31)·2^32 + (ωz+2^31)`; element 4 is the reserved 0.

### BLUETOOTH (slot 6, 24 elements)
| index | field | bounds |
|---|---|---|
| 0 | version | 0 (absent) or 1 |
| 1 | flags | bit0 poweredOn, bit1 authorized, bit2 scanned; no other bits; scanned ⇒ flags = 7 |
| 2 | scan_duration_ms | u32; 0 unless scanned |
| 3 | n_adv | u32; 0 unless scanned; = Σ histogram |
| 4 | n_unique | ≤ n_adv (deduplicated with a discarded per-session HMAC key) |
| 5–20 | rssi_histogram[16] | 5 dB bins from −100 dBm (bin 0: < −95 dBm incl. clamped lower values, bin 15: ≥ −25 dBm) |
| 21 | rssi_mean_q8 | i32, mean RSSI · 256, in [−25600, 0]; 0 if n_adv = 0 |
| 22 | rssi_var_q8 | u32, variance (dB²) · 256, ≤ 640,000; 0 if n_adv = 0 |
| 23 | persistence_pct | ≤ 100; 0 if n_adv = 0 |

Version 0 means "no aggregate" and requires every field to be zero (GXRW without a `bluetooth` object). Denied or unavailable Bluetooth is version 1 with the corresponding flags cleared and zero counts; it never invalidates a proof. No peripheral identifier or address is ever part of the aggregate.

### NOISE (slot 7, 18 elements)
| index | field | bounds |
|---|---|---|
| 0 | version | 0 (absent, no stats) or 1 |
| 1 | count | number of statistics, ≤ 16 |
| 2–17 | stats[16] | i16 residual statistics, zero-filled after `count` |

The statistics are bounded quantized residual summaries chosen by the capture app (documented in `ios-capture.md`); they never contain PRNU or another persistent sensor fingerprint.

### META (slot 8, 9 elements)
| index | field | bounds |
|---|---|---|
| 0 | version | 1 |
| 1 | image profile code | 0 png-v1, 1 jpeg-v1; equals `CanonicalImage.profile` |
| 2 | app_build | u32 parsed from the decimal `device.app_build` string |
| 3 | device_model_code | u32 table below, 0 unknown |
| 4–6 | frame_offsets_ms | i32 `round_half_away((ts_f − ts_1)/1000)` for f = 0, 1, 2 (element 5 is always 0) |
| 7 | ycbcr_matrix_code | 0 unknown, 1 `ITU_R_601_4`, 2 `ITU_R_709_2`, 3 `ITU_R_2020`, 4 `SMPTE_240M_1995` |
| 8 | quality_flags | u32 copied from GXRW `quality.flags` (informational) |

Device model codes: iPhone13,3 → 1, iPhone13,4 → 2, iPhone14,2 → 3, iPhone14,3 → 4, iPhone15,2 → 5, iPhone15,3 → 6, iPhone16,1 → 7, iPhone16,2 → 8, iPhone17,1 → 9, iPhone17,2 → 10. New models append codes; existing codes never change.

## Canonical image per profile

- png-v1: `CanonicalImage.rgb` = §5 box-downscaled raster of the center frame; `encoded` = `galaxy_image::encode(PngV1, rgb)`.
- jpeg-v1: `encoded` = `galaxy_image::encode(JpegV1, downscaled)`; `rgb` = reference-decoder output of `encoded`. Frame 1's analysis raster is sampled from that decoded raster, so `rgb_root` matches what any verifier recomputes from the exported file.
- Frames 0 and 2 always use their own downscaled rasters (no JPEG round trip); the same capture ingested under both profiles has identical frames 0 and 2, camera, IMU and aggregates.

## GXEV EvidenceBundle v1

Local persistence for resumable proving; contains secrets (salts) and sensor data. Never logged, never uploaded. All integers big-endian; `len` = u32.

```
"GXEV" ‖ u8 version = 1 ‖ u8 profile code
len ‖ encoded image bytes (1 .. MAX_IMAGE_BYTES)
frame 0: len ‖ rgb (49152·3) ‖ len ‖ depth u16 × 49152 ‖ len ‖ conf (49152)
frame 1: len ‖ depth u16 × 49152 ‖ len ‖ conf (49152)        (rgb derived, not stored)
frame 2: as frame 0
camera × 3: fx fy cx cy (i32) ‖ R[9] (i32) ‖ T[3] (i32) ‖ ts_us (u64) ‖ track (u8)
imu: g0[3] (i32) ‖ a_peak_mg (u32) ‖ 2 × (len = 64 ‖ 64 × (dt_us u32 ‖ ω[3] i32))
bluetooth: version u8 ‖ flags u8 ‖ scan_duration_ms ‖ n_adv ‖ n_unique ‖ histogram[16] (u32) ‖ rssi_mean_q8 i32 ‖ rssi_var_q8 u32 ‖ persistence_pct u8
noise: version u8 ‖ len (≤ 16) ‖ stats i16 × len
meta: version u8 ‖ app_build u32 ‖ device_model_code u32 ‖ frame_offsets_ms[3] i32 ‖ ycbcr_matrix_code u8 ‖ quality_flags u32
salts: 8 × 32-byte canonical field elements (depth0, frame_minus, frame_plus, camera, imu, bluetooth, noise, meta)
len ‖ capture_state (opaque app state, ≤ 1 MiB)
```

Decoding is strict: exact lengths for every fixed-size array, no trailing bytes, canonical field elements, profile code 0/1, the embedded image must pass `decode_any` with no XMP and its container must match the profile; `CanonicalImage.rgb` and frame 1's analysis raster are recomputed from it; the result must pass `Evidence::validate`. `encode` refuses evidence that fails validation or whose encoded image does not decode to `CanonicalImage.rgb`. `encode(decode(b)) == b` for every accepted `b`.

## Golden vectors

`test-vectors/evidence/synthetic-v1.json` (generated by `crates/galaxy-evidence/tests/evidence.rs` with `GALAXY_EVIDENCE_REGEN=1`, checked on every target by `scripts/image-golden.sh`): GXRW SHA-256 of `SyntheticCapture::default()`, fixed salts, and per profile: encoded-file SHA-256, `photo_id`, `rgb_root`, per-frame rgb/depth/conf SHA-256, every camera field, IMU g0 / a_peak / sample-stream SHA-256 / real sample counts, BLUETOOTH / NOISE / META elements, all 16 components, `evidence_root`, and the GXEV bundle SHA-256.
