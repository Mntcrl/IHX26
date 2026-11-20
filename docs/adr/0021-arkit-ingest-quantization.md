# ADR 0021 — ARKit ingest quantization (`galaxy_evidence::ingest`)

Status: accepted (ImageEvidence)

## Context
Spec §5 and §21 define the evidence model and the GXRW raw capture. The commitments, the
circuit witness and every verifier recompute from the quantized `Evidence`, so the GXRW →
`Evidence` step must be a pure function with identical output on iOS, macOS, Linux and
wasm32. ARKit delivers `Float` / `Double` values (intrinsics, column-major poses, depth in
metres, CoreMotion samples) and YCbCr 4:2:0 full-range planes.

## Decision
- Images: integer-only `ycbcr420_full_to_canonical` (BT.601 full range, chroma at
  `(x>>1, y>>1)`, exact 15:8 box filter) for all three frames; no floating point.
- Floats appear only where §5 prescribes them, as single IEEE-754 operations on `f64`
  without fused multiply-add: depth `round_half_even(d·1000)` mm for finite
  `0 < d ≤ 65.535` (else 0); intrinsics `fx/7.5`, `fy/7.5`, `(c + 0.5)/7.5 − 0.5` then Q16;
  pose `R_cv = R_arkit · diag(1, −1, −1)` (columns 1 and 2 negated, ARKit → OpenCV axes)
  in Q30 and `T` in µm; timestamps `round(ts·10^6)` µs; IMU ω in Q20 and gravity `g0` in
  Q20; `a_peak_mg = round(1000·max √(ax²+ay²+az²))` (correctly rounded `sqrt`).
  Rounding is half away from zero (`f64::round`) except depth (half even, §5).
- Range checks reject, never clamp (§5/§20): non-finite values, Q16 intrinsics outside
  `[0, 2^26)`, any pose or IMU entry outside i32, timestamps ≥ 2^48 → `IngestError`.
- IMU windowing: samples sorted stably by raw timestamp, quantized to µs; a sample whose µs
  timestamp equals the previous kept one is dropped (earliest kept). Window 0 =
  `(ts_0, ts_1]`, window 1 = `(ts_1, ts_2]`, 64 slots each, zero padding at the end, `dt`
  of the first sample measured from the window start. The motion stream must cover
  `[ts_0 − 50 ms, ts_2 + 50 ms]`, otherwise `IngestError::Motion`. `g0` is the gravity of
  the kept sample nearest `ts_1` (earliest on ties).
- Tracking codes are copied (0 not available, 1 limited, 2 normal); values > 2 reject.

## Consequences
- Every quantization step is reproducible from the GXRW bytes alone; the synthetic golden
  vector (`test-vectors/evidence/synthetic-v1.json`) passes on x86_64, aarch64-apple-darwin
  and wasm32 (`scripts/image-golden.sh`).
- Both real device captures (`fixtures/real/capture-ios/`, private) ingest under both
  profiles; aggregates are recorded in `docs/protocol/raw-capture-v1.md` (no raw values).
- A capture whose values leave the representable ranges is refused at ingest rather than
  producing a witness that the circuit would reject.
