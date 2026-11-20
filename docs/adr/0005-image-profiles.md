# ADR 0005 — Image profiles

Status: accepted (2026-11-20). Normative text: spec §4, §18.4.

## Context
- The legacy app exports `frame.capturedImage` as JPEG q = 0.94 through CoreImage (recon §6), which is not reproducible bit for bit across platforms.
- Verifiers (Rust CLI, node, browser WASM) must derive the same raster, `photo_id` and `rgb_root` from the exported file.

## Decision
- One canonical raster: 1024×768 RGB8 from the ARKit 1920×1440 YCbCr frame via deterministic integer conversion and exact box downscale (spec §5).
- `png-v1`: strict PNG subset (no colour-management chunks, no trailing data); any lossless re-encoding of the same raster verifies.
- `jpeg-v1`: pinned Rust encoder (`jpeg-encoder` =0.7.1, quality 92, 4:4:4, baseline) on device and in tools; pinned decoder (`jpeg-decoder` =0.3.2 with `platform_independent`, no SIMD) everywhere; the committed raster is the reference-decoder output.
- `photo_id` hashes only the decoded raster (container-independent). XMP carries untrusted lookup hints.

## Consequences
- JPEG certification depends on cross-platform decoder identity (x86_64, aarch64-apple-ios, aarch64-apple-darwin, wasm32), proven by golden vectors; if not achieved, JPEG is reported INCOMPLETE and only PNG is certified.
- No colour management anywhere in verification.
