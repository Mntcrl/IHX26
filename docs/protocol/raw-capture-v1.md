# GXRW v1 — reader and ingest notes (`galaxy_evidence::ingest`)

Normative format: spec §21 (writer: `apps/capture-ios/XRPL_Galaxy/RawCaptureWriter.swift`). This file records how the Rust reader interprets it. GXRW bytes are sensitive sensor data (spec §19): the reader never logs header values, pixels, depth or IMU samples; errors carry only a class (and, for JSON, a line/column).

## Framing
- `"GXRW"` ‖ `0x01` ‖ u32be header_len ‖ header ‖ payload. Wrong magic/version, `header_len` = 0 or > 1 MiB, or a header longer than the input → `IngestError::Prefix`.
- The payload must be exactly `3 × (1920·1440 + 1920·720 + 256·192·4 + 256·192)` = 13,178,880 bytes; each frame has four regions `y`, `cbcr`, `depth`, `conf` with exactly those lengths. The twelve regions must lie inside the payload, must not overlap and must tile it completely (any order) → otherwise `IngestError::Regions`.
- Header: UTF-8 JSON, unknown fields rejected at every level (`deny_unknown_fields`), floats parsed exactly (`serde_json` `float_roundtrip`). `bluetooth` and `noise` may be absent (→ version-0 all-zero aggregates); every other field is mandatory. `device.os`, `tracking_reason`, `quality.reasons` and `claimed_time_ms` are parsed for strictness but not committed.

## Checks and error classes
| condition | error |
|---|---|
| `format` ≠ `galaxy-raw-capture/v1`, image ≠ 1920×1440 `420f`, depth ≠ 256×192 | `Format` (capture unsupported) |
| roles not exactly `minus`, `center`, `plus`; quantized timestamps not strictly increasing | `Frames` |
| non-finite or out-of-range intrinsics / transform / timestamps / IMU values, tracking > 2, IMU interval > u32 | `Quantization(field)` |
| IMU not covering `[t_minus − 0.05 s, t_plus + 0.05 s]`, more than 64 samples in a window, no sample at all | `Motion` |
| conf value > 2, inconsistent aggregates, `app_build` not a decimal u32 | `Evidence(ValidationError)` |

All of them make the capture INCONCLUSIVE (spec §21); none is retried with relaxed rules.

## Quantization (spec §5; details in ADR 0021)
- Image: `ycbcr420_full_to_canonical` (integer BT.601 full range, chroma at `(x>>1, y>>1)`, exact 15:8 box filter) for all three frames.
- Depth: `f32` little-endian → `round_half_even(d·1000)` mm if finite and `0 < d ≤ 65.535`, else 0. Confidence copied.
- Intrinsics: `[fx, fy, cx, cy]` → `fx/7.5`, `fy/7.5`, `(c + 0.5)/7.5 − 0.5`, Q16 with round-half-away-from-zero, each in `[0, 2^26)`.
- Pose: `transform` is column-major; `R_arkit[row][col] = transform[4·col + row]`, `R_cv = R_arkit · diag(1, −1, −1)` (columns 1 and 2 negated), Q30; `T = transform[12..15]` in µm; every entry in i32.
- Timestamps: `round(ts·10^6)` µs, `< 2^48`.
- IMU: samples sorted by raw timestamp (stable), quantized to µs; a sample whose µs timestamp equals the previous kept sample is dropped (the first one in time order is kept). Window 0 = `(ts_0, ts_1]`, window 1 = `(ts_1, ts_2]`; `dt` of the first sample is measured from the window start; ω = `round(rate·2^20)`; zero padding to 64 at the end. `g0` = gravity (Q20) of the kept sample nearest `ts_1` (earliest on ties). `a_peak_mg` = `round(1000·max √(ax²+ay²+az²))` over kept samples in `[ts_0, ts_2]` (IEEE-754 f64, correctly rounded `sqrt`, no fused operations).
- META as in `evidence-v1.md`.

## Real device captures (private fixtures)
`fixtures/real/capture-ios/*.gxrw` (gitignored, spec §19) are ingested by `crates/galaxy-evidence/tests/real_captures.rs` when present (skipped otherwise). The test ingests both profiles, checks decoder/raster consistency, commits with fresh salts and round-trips the bundle; it prints only quantized aggregates (valid-depth fraction, high-confidence fraction, center-frame intrinsics on the analysis grid rounded to 0.1 px, baselines to the center frame in mm, frame offsets, tracking codes, IMU sample counts, a_peak bucketed to 50 mg, matrix/model codes).

Measured on 2026-11-20 (iPhone 13 Pro Max, `cargo test --release -p galaxy-evidence --test real_captures -- --nocapture`; both captures pass ingest under png-v1 and jpeg-v1, `validate`, decode equality, commit and GXEV round trip):

| | capture #1 | capture #2 |
|---|---|---|
| GXRW size | 13,194,535 B | 13,194,191 B |
| png-v1 / jpeg-v1 file | 1,012,313 B / 162,189 B | 981,156 B / 154,078 B |
| GXEV bundle (jpeg-v1, empty app state) | 902,212 B | 894,101 B |
| valid-depth fraction (t−, t0, t+) | 1.00, 1.00, 1.00 | 1.00, 1.00, 1.00 |
| high-confidence fraction (t−, t0, t+) | 0.03, 0.57, 0.57 | 0.61, 0.63, 0.56 |
| analysis-grid intrinsics t0 (fx, fy, cx, cy) px | 193.6, 193.6, 128.0, 95.5 | 197.3, 197.3, 128.0, 95.6 |
| baseline to t0 (t−, t+) | 91 mm, 228 mm | 150 mm, 220 mm |
| frame offsets | −267, 0, 267 ms | −267, 0, 267 ms |
| tracking codes (t−, t0, t+) | 1, 2, 2 (t− limited) | 2, 2, 2 |
| IMU samples (window 0, window 1) | 27, 26 | 26, 26 |
| a_peak bucket | 550 mg | 350 mg |
| matrix code / model code | 1 (BT.601) / 4 (iPhone14,3) | 1 / 4 |

Capture #1's limited tracking at t− and low confidence at t− are carried into the evidence unchanged; the policy decides on them (spec §8), ingest does not.
