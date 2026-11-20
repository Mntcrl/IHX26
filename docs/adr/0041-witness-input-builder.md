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
- The builder fails closed before witness generation: `Evidence::validate` (§20
  ranges, never clamps), challenge recomputed from seed and capture id, view layout
  equal to the challenge picks, `(0,0)` only for not-in-bounds pairs, salts < 2^248,
  components and evidence root recomputed, every opening re-hashed to its root.
- Leaf limbs come from `galaxy_crypto::tree::{pack_rgb, pack_depth}` and siblings
  from `FrameTree::opening`; no second implementation of packing or trees.

## Consequences
- A view or challenge that does not belong to the evidence yields a typed error
  rather than an unsatisfiable witness.
- The JSON must never be logged; callers serialize it only to pass it to the prover.
