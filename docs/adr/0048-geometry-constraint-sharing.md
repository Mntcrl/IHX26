# ADR 0048 — Share geometry selection and residual arithmetic

Status: accepted (CircuitGeo)

## Context
Most policy constraints come from repeated plane support and temporal reprojection, and the v1m policy is budgeted for at most 450,000 constraints under the production optimizer.

## Decision
Pack hypothesis vertex fields into one 33-bit wire per point, mux that wire by index, and decompose only the selected entry. Reuse each global plane's residual decomposition for both the inlier and strong half-open bands. Select those already computed predicates using the first maximum support index. In reprojection, reuse quotient bits to select the block and pixel. Compute t0 depth validity only for non-corner pixels, since no t0 validity predicate reads a corner.

## Consequences
These changes do not remove a policy check or change its inputs. They rely on the caller's exact raster, layout and entry ranges, and on deterministic first-maximum semantics. The cost accounting and reproducible measurement are in [GEOMETRY.md](../../circuits/GEOMETRY.md).
