# ADR 0020 — jpeg-v1 reference decoder and cross-target determinism

Status: accepted (ImageEvidence). Requires a spec §4 wording update (decoder name).

## Context
For jpeg-v1 the committed raster is the reference-decoder output of the exported JPEG
(spec §4), so every verifier (CLI on x86_64 Linux, browser wasm32, iOS aarch64, macOS
aarch64) must reproduce the decoder output bit for bit. Spec §4 names `jpeg-decoder` with a
scalar integer IDCT and no SIMD. Inspection of `jpeg-decoder 0.3.2` showed:
- `src/arch/mod.rs` selects SSSE3 IDCT and colour conversion at run time with
  `is_x86_feature_detected!("ssse3")`; its changelog states the SIMD path is not bit-for-bit
  compatible with the scalar path. The additive `platform_independent` feature removes it,
  so this part alone would be controllable;
- threads cannot be disabled: besides the default `rayon` feature, `src/worker/mod.rs`
  selects a `std::thread` `MpscWorker` for multithreaded decoding on every target other than
  wasm32/asmjs, with no public option to force the immediate worker, so wasm32 and native
  targets run different worker implementations;
- the 4:2:0 upsampler computes row positions in `f32` (`src/upsampler.rs`).
The brief requires a decoder with SIMD and threads disabled on every target, which
`jpeg-decoder` cannot provide without a vendored fork.

## Decision
- Reference decoder: `zune-jpeg =0.4.21` with `zune-core =0.4.12`, both
  `default-features = false`. Without the `x86` and `neon` features zune-jpeg compiles only
  its scalar integer IDCT, upsampler and YCbCr→RGB conversion; options: strict mode,
  `use_unsafe = false`, maximum dimensions 1024 × 768, RGB output. Pure Rust, no threads.
- Feature unification is a risk (another crate enabling `x86`/`neon` would silently change
  the decoder): `scripts/image-golden.sh` fails if `cargo tree --workspace --target all`
  reports any enabled zune-jpeg feature.
- A strict marker walk (`jpeg_profile.rs`) admits only the §4 variant set (SOF0, 8-bit,
  1024 × 768, three 1 × 1 components, one full-spectrum sequential scan, no restart markers)
  before the decoder runs, which keeps the decoder on a single well-tested path.
- Encoder unchanged: `jpeg-encoder =0.7.1`, quality 92, 4:4:4, baseline, standard Huffman
  tables, no restart interval.
- Determinism is proven by golden vectors, not assumed: `test-vectors/image/jpeg-golden.json`
  (12 cases: 10 synthetic rasters, one 420f sensor scene through the §5 pipeline, one CC0
  street photograph encoded by a third-party encoder) records input, encoded-file and
  decoded-raster SHA-256, `photo_id` and `rgb_root`; `test-vectors/evidence/synthetic-v1.json`
  covers the full ingest → commit pipeline for both profiles.

## Evidence (2026-11-20, `scripts/image-golden.sh linux|wasm|mac`, same committed vectors)
| target | toolchain | `golden.rs` | `evidence.rs` |
|---|---|---|---|
| x86_64-unknown-linux-gnu | 1.91.1 (repo pin), release | 1/1 pass | 9/9 pass |
| wasm32-unknown-unknown (node, wasm-bindgen-test-runner) | 1.91.1, release | 1/1 pass | 9/9 pass |
| aarch64-apple-darwin (Mac build host) | 1.92.0 stable, `--locked`, release | 1/1 pass | 9/9 pass |

A pass means every decoded-raster hash, encoded-file hash (encoder determinism), `photo_id`,
component root and `evidence_root` matched the committed vectors.
aarch64-apple-ios: `galaxy-image` and `galaxy-evidence` build for the target; the decoder has
no `target_os`-dependent code, so it runs the same scalar path as aarch64-apple-darwin.
[INFERENCE] An on-device run of the golden test has not been performed (device access is
granted per window by the orchestrator, spec §19); it is the remaining confirmation step.

## Consequences
- jpeg-v1 certification is not INCOMPLETE on the measured targets (spec §4 fallback not needed).
- Spec §4 should name `zune-jpeg` (scalar build) instead of `jpeg-decoder`.
- Any change of decoder/encoder version or features is a protocol change: regenerate vectors
  with `GALAXY_IMAGE_REGEN=1` on Linux and re-run all targets before release.
- The scalar decoder is slower than SIMD builds ([INFERENCE] a few ms per 1024 × 768 image on
  desktop); acceptable for a single verification decode.
