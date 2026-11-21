# ADR 0041 — Witness input schema and `galaxy-witness`

Status: accepted (CircuitCore)

## Context
The circuit input JSON (spec §9) is consumed by the WASM witness calculator (desktop,
snarkjs) and by the iden3 witness graph (`main_v1m.graph.bin`, galaxy-prover on iOS).
Both parse decimal strings. The input contains private evidence.

## Decision
- Schema in `circuits/INPUTS.md`: exactly the main template's input names, decimal
  strings in [0, r), signed values as r − |v|, arrays nested as declared.
- `galaxy_witness::build_input(&Evidence, &Salts, &Commitments, &Challenge,
  &capture_id, &GeometryView) -> Result<serde_json::Value, WitnessError>` and
  `build_input_from_evidence` (builds the view with `galaxy_geometry::build_view`).
  `public_inputs(&Commitments, &capture_id, &Challenge) -> [Fr; 22]` delegates to
  `galaxy_crypto::public::public_inputs_v1` (single layout source).
- The builder checks `Evidence::validate` (§20 ranges, never clamps), sampling
  values derived from the supplied seed and capture id, view layout, frame
  ordering, temporal coordinates in the real grid (with `(0,0)` required for
  out-of-bounds pairs), and salts < 2^248. It recomputes all 16 component
  commitments, including the fixed padding slots, and the evidence root.
  Every opening is re-hashed to its root.
- Leaf limbs come from `galaxy_crypto::tree::{pack_rgb, pack_depth}` and siblings
  from `FrameTree::opening`; no second implementation of packing or trees.

## Consequences
- An inconsistent input detected by the builder yields a typed error.
  In-bounds temporal coordinates from a supplied view are still checked against
  the actual projection by `GalaxyPolicyV1m`; callers without a view can use
  `build_input_from_evidence`.
- The JSON must never be logged; callers serialize it only to pass it to the prover.
