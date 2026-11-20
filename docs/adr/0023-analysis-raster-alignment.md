# ADR 0023 — Analysis raster alignment

Status: accepted (ImageEvidence)

## Context
The circuit compares image structure with LiDAR depth on a 256 × 192 grid. ARKit depth is
256 × 192 over the 1920 × 1440 sensor image (scale 7.5); the canonical raster is
1024 × 768 (scale 1.875). Spec §4 fixes the analysis raster as
`A[u,v] = raster[4u+2, 4v+2]` and pairs analysis pixel (u,v) with depth pixel (u,v).

## Decision
- `galaxy_image::analysis_raster` samples (no averaging) canonical pixel `(4u+2, 4v+2)`.
  Its centre lies at source coordinate `(4u+2.5)·1.875 = 7.5u + 4.6875`; the depth pixel
  centre lies at `(u+0.5)·7.5 = 7.5u + 3.75`. The fixed offset is 0.9375 source pixels =
  0.125 depth pixels to the right and down, identical for every pixel and every capture.
- Intrinsics are expressed on the depth / analysis grid with the pixel-centre convention
  `c_a = (c + 0.5)/7.5 − 0.5` (ADR 0021); the 0.125-pixel offset is not compensated in the
  intrinsics, so geometry code sees the documented grid exactly as committed.
- For jpeg-v1 the frame-1 analysis raster is sampled from the reference-decoder output of
  the exported JPEG; frames 0 and 2 are sampled from their own box-downscaled rasters.

## Consequences
- Sampling is a pure index operation, trivially identical across targets and cheap to open
  in the circuit (one canonical leaf per analysis pixel).
- The sub-pixel misalignment is below the policy's edge tolerances (one analysis pixel);
  [INFERENCE] its effect on edge-statistics thresholds is absorbed by the policy constants
  calibrated on real captures (Geometry workstream).
