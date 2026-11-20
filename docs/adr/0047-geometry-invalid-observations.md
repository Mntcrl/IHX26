# ADR 0047 — Compute well-defined outputs for rejected observations

Status: accepted (CircuitGeo)

## Context
The policy template reports statistics even when geometry, camera or motion checks fail. Invalid observations can yield zero plane area, zero or negative projection depth, and positive-depth projections arbitrarily far outside the raster.

## Decision
Evaluate pre-degeneracy before fitting. Select `num,den = 0,1` on that branch; after division, zero a plane's coefficients if the coefficient bound fails. Mask invalid depths to zero for residual arithmetic, and always gate residual predicates with validity and non-degeneracy. For reprojection, use signed numerator comparisons to decide raster membership before division. Inactive projection coordinates come from `0 / 1` and constrain both opened block coordinates to zero.

## Consequences
Bad quality, excessive depth, degenerate planes and nonpositive or offscreen projections do not cause witness generation to fail just by being observed. The computed flags still reject these observations as prescribed by the policy. The caller must enforce the §20 ranges and authenticate the opened blocks; it cannot override a branch by choosing a different block.
