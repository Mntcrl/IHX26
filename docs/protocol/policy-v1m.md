# Policy `galaxy-pc-v1m` — field reference

Source of truth for values: `policies/galaxy-pc-v1m.json`. Formulas: spec §8 (this page only
names fields, units and the validation bounds enforced by
`galaxy_protocol::policy::Policy::validate`). The policy id is
SHA256("GALAXY/POLICY/v1\0" ‖ canonical JSON) (`Policy::id`); the initial file's canonical
bytes and id are pinned in `test-vectors/protocol/policy.json`
(id `ef744a7f28dc606b117dbab34899753a218cd1865b4fc69a9739ac9c10a7afac`). Any value change yields
a new policy id and therefore a new circuit id. Circom constants are generated from the JSON
(`scripts/circuit-gen-policy.mjs`), never copied by hand.

Units: `_mm` millimetres, `_um` micrometres, `_us` microseconds, `_pct` percent (integer),
`_pm` per mille, `_y` luma units of Y = 77R + 150G + 29B (max 65280), `_qN` fixed point with N
fractional bits, `_log2` exponent of a power of two, `_milli` thousandths.

## Header
| key | value | rule |
|---|---|---|
| name | `galaxy-pc-v1m` | non-empty ASCII |
| version | 1 | must be 1 |
| profile | `galaxy-pc-v1m` | must equal the profile |

## structural (informational, fixed by the circuit; must equal these values)
| key | value | meaning |
|---|---|---|
| p | 2^32 | plane coefficient scale P (§8.2) |
| coefficient_bound | 2^36 | degenerate-plane bound on \|a\|,\|b\|,\|c\| |
| slots | 16 | sampled tiles |
| central_slots | 8 | central-stratum tiles |
| blocks_per_slot | 4 | opened blocks per tile (one per quadrant) |
| global_hypotheses | 4 | plane hypotheses over all 256 points |
| central_hypotheses | 4 | plane hypotheses over central points 0..127 |
| temporal_quadrants | [0, 3] | quadrants used as temporal candidates |
| imu_max | 64 | IMU samples per window |

## lidar (§8.2)
| key | value | bounds | meaning |
|---|---|---|---|
| d_min_mm | 200 | 1..=d_max_mm | minimum valid depth |
| d_max_mm | 5000 | 1..=8191 | maximum valid depth (< 8192 required) |
| conf_min | 1 | 0..=3 | minimum ARKit confidence |
| area_min | 64 | 1..=65535 | minimum \|Δ\| of a hypothesis triangle (px²·2) |
| eps_log2 | 26 | 0..=sep_log2 | inlier residual band ±2^eps_log2 |
| sep_log2 | 28 | 0..=36 | strong-outlier band ±2^sep_log2 |
| v_all_min | 128 | 0..=256 | valid points overall |
| v_c_min | 64 | 0..=128 | valid central points |
| tile_valid_min | 8 | 0..=16 | valid points for a covered tile |
| tiles_cov_min | 12 | 0..=16 | covered tiles |
| plane_global_max_pct | 85 | 0..=100 | max share of valid points on the best global plane |
| plane_central_max_pct | 80 | 0..=100 | same for central hypotheses |
| tile_out_min | 4 | 0..=16 | outliers for a non-planar tile |
| nonplanar_tiles_min | 6 | 0..=16 | non-planar tiles |
| nonplanar_tiles_c_min | 3 | 0..=8 | non-planar central tiles |
| strong_min_pct | 10 | 0..=100 | strong outliers as share of valid points |
| strong_tiles_min | 3 | 0..=16 | tiles with ≥ 1 strong outlier |

## rgb_depth (§8.3)
| key | value | bounds | meaning |
|---|---|---|---|
| disc_pct | 4 | 0..=200 | relative depth jump for a discontinuity |
| grad_min_y | 2560 | 0..=65280 | luma gradient for an edge |
| disc_min | 6 | 0..=512 | minimum discontinuities N_disc |
| edge_match_pct | 50 | 0..=100 | discontinuities matched by an RGB edge |

## temporal (§8.4)
| key | value | bounds | meaning |
|---|---|---|---|
| occ_pm | 50 | 0..=1000 | occlusion margin |
| dcons_pm | 30 | 0..=1000 | depth consistency tolerance |
| g_inf_y | 2560 | 0..=65280 | luma contrast for an informative point |
| photo_tol_y | 5120 | 0..=65280 | photometric tolerance |
| t_usable_min | 24 | 0..=64 | usable reprojected pairs |
| occ_max_pct | 40 | 0..=100 | max occluded share of usable pairs |
| dok_min_pct | 70 | 0..=100 | depth-consistent share of non-occluded pairs |
| inf_min | 8 | 0..=64 | informative pairs |
| pok_min_pct | 60 | 0..=100 | photometrically consistent share of informative pairs |

## quality (§8.5)
| key | value | bounds | meaning |
|---|---|---|---|
| dt_min_us | 80000 | 1..=dt_max_us | minimum frame spacing |
| dt_max_us | 600000 | 1..2^48 | maximum frame spacing |
| fx_min_q16 | 9830400 (150 px) | 1..=fx_max_q16 | focal length lower bound (fx and fy) |
| fx_max_q16 | 17039360 (260 px) | 1..2^26 | focal length upper bound (fx and fy) |
| cx_min_q16 | 6553600 (100 px) | 0..=cx_max_q16 | principal point x |
| cx_max_q16 | 10223616 (156 px) | 0..2^26 | |
| cy_min_q16 | 4980736 (76 px) | 0..=cy_max_q16 | principal point y |
| cy_max_q16 | 7602176 (116 px) | 0..2^26 | |
| ortho_tol_q60 | 2^50 | 0..2^53 | rotation orthonormality tolerance (Q60) |

## motion (§8.5)
| key | value | bounds | meaning |
|---|---|---|---|
| baseline_min_um | 3000 | 0..=u32 | minimum translation between frame pairs |
| rot_max_q30 | 751619277 (≈ 0.7 in Q30) | 0..=u32 | max \|vee(R_j1)\| component |
| imu_min_samples | 5 | 0..=64 | non-padding samples per window |
| imu_dt_max_us | 25000 | 1..=u32 | maximum IMU sample spacing |
| imu_cover_tol_us | 30000 | 0..=u32 | window coverage tolerance |
| gyro_tol_urad | 20000 | 0..=u32 | gyro vs. ARKit rotation tolerance |
| m_cd | [0,−1,0, −1,0,0, 0,0,−1] | signed permutation | device → CV camera axes (calibrated on device) |
| grav_norm_tol_q40 | 109951162778 (≈ 0.1 in Q40) | 0..2^53 | gravity norm tolerance |
| grav_cos_milli | 950 | −1000..=1000 | min cosine between IMU and ARKit gravity |
| acc_max_mg | 2000 | 0..=u32 | maximum acceleration peak |

## windows (§7)
| key | value | rule | meaning |
|---|---|---|---|
| w_cap | 20 | > 0 | Lpre < L0 ≤ Lpre + w_cap |
| w_proof | 300 | > 0 | Lsel < Lproof ≤ Lsel + w_proof |
| sel_offset | 5 | > 0 | Lsel = L0 + sel_offset |

Unknown keys are rejected at every level (`deny_unknown_fields`). Test-only policies (spec §8.2)
must carry their own name/id and are never listed in a shipped network configuration.
