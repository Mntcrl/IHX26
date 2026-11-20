# ADR 0042 — Constraint-level negative tests

Status: accepted (CircuitCore)

## Context
A witness calculator rejecting an invalid input only shows that the honest witness
code refuses it. Soundness requires that no assignment of the prover-controlled
wires satisfies the R1CS. Negative tests must therefore check the constraint system
directly, with every dependent intermediate value recomputed, and must show that the
rejection comes from the intended check (not from stale hash values).

## Decision
`circuits/tests/core/core.test.mjs` (node:test + circom_tester, `--O0` so every
signal is a witness wire):
1. (a) Witness generation for the invalid input fails (`Assert Failed`).
2. (b) A *trace calculator* is compiled from a copy of the same sources with every
   `===` statement removed (comments stripped first). Its `.sym` file must be
   byte-identical to the audited circuit's, so its witness has the same wire layout.
   The trace of the invalid input is computed (all `<==`/`<--` values, including
   Poseidon, bit decompositions and Merkle paths, recomputed), copied over the valid
   witness wire by wire, and checked against the audited circuit's own R1CS:
   `checkConstraints` must fail and the number of unsatisfied rows must equal the
   expected count (1 for every case: the intended equality only).
3. Cases where the trace is not reachable by an input (non-canonical bit
   decomposition h + r) are built by hand: the AliasCheck sub-trace is computed by a
   separate `AliasTrace` harness and spliced in, all downstream picks are recomputed,
   and exactly one row (the strict canonical check) fails.

Covered: incorrect sibling, one depth value changed, RGB of another image, CAMERA
element changed (evidence_root mismatch), signed range overflow, IMU packing
overflow, RGB limb overflow (out-of-range bytes), tile ≥ 48, wrong stratum (central
and peripheral), non-ascending tiles, seed limb ≥ 2^128, a different seed limb with
the original ctx-derived values, non-canonical decomposition, padding-row and
wrong-frame FRAME openings.

## Consequences
- Tests compile small harnesses only (~3 min total). On the full circuit,
  `scripts/circuit.sh witness` checks every R1CS row of the passing synthetic
  witness in Rust and requires that changing any of the public wires rgb_root,
  evidence_root, seed_lo or sampled_tiles[0] leaves a row unsatisfied; the
  end-to-end proof is `scripts/circuit.sh e2e`.
- Any new `===` in core components is automatically exercised by the same method.
