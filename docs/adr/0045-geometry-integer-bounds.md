# ADR 0045 — Bound geometry arithmetic for every admitted input

Status: accepted (CircuitGeo)

## Context
Circom works in BN254, but the Rust policy uses mathematical integers. A field residue does not have an integer sign without a bound; in particular, observations that fail quality checks can have extreme pose and sensor values.

## Decision
Use §20's entry ranges, not policy acceptance bounds, to bound intermediate integers and comparison differences. Decompose `x + 2^n` to represent `−2^n ≤ x < 2^n`, and read its high bit for the sign. Use comparison widths derived from count limits and the compile-time thresholds where appropriate. Document bounds by stage in [GEOMETRY.md](../../circuits/GEOMETRY.md).

## Consequences
Signed comparisons and interval checks implement the Rust boundary semantics without aliasing. The decomposition widths remain valid for failing observations. Changes to wire ranges or formulae require rechecking the bounds; changing a threshold is not permission to narrow an input range.
