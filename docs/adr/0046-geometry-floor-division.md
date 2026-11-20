# ADR 0046 — Constrain Euclidean floor division

Status: accepted (CircuitGeo)

## Context
Plane fitting, pose composition and temporal reprojection use floor division. Neither field inversion nor truncation toward zero is equivalent.

## Decision
For a variable positive denominator, range-check `q`, `r` and `den − 1 − r`, and constrain `num = q·den + r`. A witness calculator can supply `q`, but only the unique floor quotient can satisfy the constrained identity. For powers of two, take the high bits of the offset signed decomposition and subtract the shifted offset. For in-raster reprojection, use an unsigned quotient and reuse its bits for block and pixel selection.

## Consequences
A negative numerator, including one exactly divisible by `den`, has the same result as in Rust. An alternative `q,r` pair preserving the identity but not the remainder interval is rejected by the R1CS. Magnitude bounds keep the identity an integer equality rather than only a field equality.
