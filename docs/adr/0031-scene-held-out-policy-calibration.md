# ADR 0031 — Scene-held-out mobile policy calibration

Status: accepted for the development policy; not a deployment-acceptance result.

## Context

The baseline policy's RGB–depth predicate frequently rejected real 3-D scenes. Close screen recaptures and a mostly planar table with a phone also sometimes passed. The available 99 captures are from one device and one indoor environment; many are near-duplicates. Individual challenges from a capture are repeated evaluations, not independent captures.

## Decision

Reject unconstrained fit `f3999a9faa4d5ff2af9e6cf9be7ebd5da3122bc4ab7475db9be63bb220016638`. It reduced global/central nonplanar tile minima from 6/3 to 4/2, and synthetic centre-monitor passes increased from 3/20 to 19/20 in both image profiles. Its 412/448 held-out genuine-3D passes do not outweigh this regression.

Repeat the same training-only fit with the same scene split, 32 challenges per capture, class caps, and search grid. Freeze the five global/central planarity thresholds (`nonplanar_tiles_min=6`, `nonplanar_tiles_c_min=3`, `plane_global_max_pct=85`, `plane_central_max_pct=80`, `tile_out_min=4`). Impose a hard non-regression cap per scenario and per image profile for frontal, tilted, curved and centre monitors; print; flat wall; depth substitution; and swapped frames. The real holdout was not used to score candidates. The guarded synthetic suite informed the refit and is not independent validation. Full results are in the [calibration report](../experiments/calibration-v1m.md).

| Group | Key | Baseline | Final |
|---|---|---:|---:|
| lidar | `d_min_mm` | 200 | 325 |
| rgb_depth | `grad_min_y` | 2560 | 1280 |
| rgb_depth | `edge_match_pct` | 50 | 40 |
| temporal | `occ_pm` | 50 | 65 |

Keep all §8 formulas, structural values, the 500 µm baseline floor, and the device-to-camera matrix. Derive temporal boundary fixture depths from policy values so that both inclusive depth agreement and strict occlusion remain tested.

## Consequences

The 325 mm floor excludes more close views regardless of content. A 65‰ occlusion margin labels fewer pairs as occluded; pairs between the depth-consistency and occlusion bounds can therefore count against agreement. Neither change establishes that the scene is a recapture. The data do not justify claims about unseen printed targets, flat tables, or other devices or environments.

The final policy meets all four real training caps and all eight synthetic non-regression caps in both profiles. On the reused real holdout, genuine 3-D PASS improved from 389/448 (86.83%) to 395/448 (88.17%); held-out screen recapture and flat wall passed 0/64 and 0/32, each from one scene proxy. Final synthetic centre-monitor PASS is still 3/20 per profile, and depth substitution still passes 20/20. This decision is not deployment acceptance.

The policy ID changes from `b27f0afc072bd1382d49f2313ed3f4dd7de69a00cae0a517b95ab8d20b8a6aee` to `4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073`. Regenerate constants and vectors from JSON, rebuild the circuit and setup, and bind the new artifact IDs explicitly; prior proofs remain tied to their original policy/circuit.
