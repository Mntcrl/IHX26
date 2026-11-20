# Real captures under galaxy-pc-v1m: m_cd calibration and first evaluation

Data: the two private GXRW captures in `fixtures/real/capture-ios/` (iPhone 13 Pro Max, `iPhone14,3`; gitignored, never committed). Only aggregates are reported here. Capture descriptions: `device-capture-sensors.md`.

| capture | file | tracking t−/t0/t+ | frame offsets | baselines t−/t+ | relative rotation t−/t+ | max_a abs(vee(R_j1)_a) (rot_max_q30 = 0.700) | depth median (p10–p90) at t0 |
|---|---|---|---|---|---|---|---|
| #1 | 15473E68-…gxrw | 1, 2, 2 | −267 / +267 ms | 91 / 228 mm | 6.9° / 14.9° | 0.211 / 0.326 | 0.40 m (0.31–0.66) |
| #2 | FB010EC4-…gxrw | 2, 2, 2 | −267 / +267 ms | 150 / 220 mm | 25.9° / 14.5° | 0.855 / 0.477 | 0.32 m (0.25–1.69) |

Baselines, rotations and depth percentiles were computed from the GXRW header poses and depth planes with an ad-hoc numpy check (spec §5 conventions: R_cv = R_arkit·diag(1, −1, −1)).

## 1. m_cd fit (ADR 0030)

`geometry-tool imu-frame <gxrw…>` builds each capture's GeometryView, integrates the gyro windows (Θ_w = Σ ω_k·dt_k) and, for all 48 signed permutations, compares vee(R_01) with +2^11·m_cd·Θ_0 and vee(R_21) with −2^11·m_cd·Θ_1 (the §8.5 gyro equations), reporting the RMS and maximum per-axis residual in µrad. Captures are gated only by the view domain and the §8.5 IMU-window rules (tracking state does not enter the comparison, so #1 is included).

| data set | best m_cd | RMS (mrad) | max axis residual (mrad) | 2nd | 3rd | 4th | old policy value [0,1,0, 1,0,0, 0,0,−1] |
|---|---|---|---|---|---|---|---|
| #1 | [0,−1,0, −1,0,0, 0,0,−1] | 36.0 | 74.5 | [0,−1,0, 0,0,−1, −1,0,0] 40.4 | [0,0,−1, 0,−1,0, −1,0,0] 83.2 | [−1,0,0, 0,−1,0, 0,0,−1] 84.6 | 186.0 |
| #2 | [0,−1,0, −1,0,0, 0,0,−1] | 30.3 | 59.8 | [1,0,0, 0,1,0, 0,0,−1] 60.9 | [0,−1,0, 1,0,0, 0,0,−1] 69.9 | [−1,0,0, 0,1,0, 0,0,−1] 78.8 | 120.2 |
| #1 + #2 | [0,−1,0, −1,0,0, 0,0,−1] | 33.3 | 74.5 | [−1,0,0, 0,−1,0, 0,0,−1] 97.0 | [−1,0,0, 0,1,0, 0,0,−1] 103.1 | [0,−1,0, 1,0,0, 0,0,−1] 111.9 | 156.6 |

Independent check with the §8.5 gravity predicate (g0 = CoreMotion gravity at the sample nearest ts_1, gp = −row1(R_1)): cos(m_cd·g0, gp)

| capture | old m_cd | fitted m_cd | grav_cos_milli threshold |
|---|---|---|---|
| #1 | −0.922 | 0.991 | 0.950 |
| #2 | −0.744 | 0.993 | 0.950 |

The fitted matrix is also what the sensor orientation predicts: the capturedImage native orientation is landscape with the device top to the left, so CV x = −device y, CV y = −device x, CV z = −device z. Applied: `motion.m_cd = [0,−1,0, −1,0,0, 0,0,−1]`, new policy_id `ef744a7f28dc606b117dbab34899753a218cd1865b4fc69a9739ac9c10a7afac`.

Open: the best-fit residual (RMS 33 mrad, max 75 mrad) exceeds gyro_tol_urad = 20 mrad. Both captures are far outside the intended hand-held envelope (relative rotations up to 26°, i.e. #2 exceeds rot_max_q30 on the t− side; baselines 91–228 mm vs the synthetic model's 2–15 mm; IMU/frame timestamp offsets of −3.1 / −5.0 ms), and Σω·dt is a first-order approximation of the rotation for such large non-commuting motion. gyro_tol_urad, rot_max_q30 and the baseline envelope stay unchanged until a **small-motion capture** (a few mm to a few cm, a few degrees, scene at 0.5–3 m) is available to calibrate them.

## 2. Capture #2 evaluated with random challenges

Ingest: `galaxy_evidence::ingest::from_gxrw(…, PngV1)` → `build_view` → `evaluate`, via `geometry-tool eval <gxrw> --capture-id … --lsel … --lsel-hash …` with capture_id and lsel_hash drawn from /dev/urandom (development-only random challenges; no ledger involved). Policy after calibration.

One random challenge (capture_id `b0906cd1378bdd3b88463bc6e6369a4ecbbc92466fb2fd069aff1baf69112049`, Lsel 1021745, lsel_hash `178c39f110fd9c000c0bb013cced5e32e7946d18e33de25ab915f253064f4a06`):

| V_all | V_c | n_hyp_ok_g | n_hyp_ok_c | max_all | max_c | j_star | tiles_cov | tiles_nonplanar | tiles_nonplanar_c | n_strong | tiles_strong | N_disc | N_match | N_usable | N_occ | N_dok | N_inf | N_pok |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 185 | 128 | 1 | 4 | 53 | 53 | 0 | 11 | 12 | 7 | 67 | 8 | 15 | 0 | 33 | 20 | 2 | 0 | 0 |

quality_ok 1, geometry_ok 0 (tiles_cov 11 < 12), rgb_depth_ok 0, temporal_ok 0, motion_ok 0, pass 0.

16 further random challenges: predicate true counts quality 16, geometry 16, rgb_depth 4, temporal 0, motion 0, PASS 0. Statistics (min / median / max): V_all 188/202/234, V_c 120/126/128, n_hyp_ok_g 1/2/4, n_hyp_ok_c 3/4/4, max_all 15/57/105, max_c 23/62/97, tiles_cov 12/13/15, tiles_nonplanar 11/14/16, tiles_nonplanar_c 6/8/8, n_strong 29/84/150, tiles_strong 6/10/14, N_disc 0/10/19, N_match 0/1/9, N_usable 26/31/35, N_occ 14/20/26, N_dok 0/0/1, N_inf 0/0/2, N_pok 0/0/2.

Capture #1 with 3 random challenges: quality false (t− tracking limited), temporal and motion false in all three.

### Findings
- Geometry (3D desk scene) behaves as on the synthetic room: low planarity, many non-planar and strong tiles.
- motion_ok fails because the capture exceeds rot_max_q30 (#2 t−) and gyro_tol (see §1); the gravity and IMU-window checks pass with the calibrated m_cd [INFERENCE: inferred from the gravity cosines above and the fit gating; the tool does not report sub-checks].
- temporal_ok fails systematically: most usable pairs are occluded (N_occ ≈ 20 of ≈ 31) and almost none is depth-consistent (N_dok ≈ 0). An ad-hoc numpy reprojection of a dense frame-t0 grid into t−/t+ (independent of the Rust engine) also finds ≤ 30 % of points within 5 % depth agreement under the spec conventions and under 32 variants (axis-flip conventions, transposed rotations, translation sign, image flips, frame/pose permutations). [INFERENCE] With 26°/15 cm motion at a 0.3 m scene, parallax, disocclusion and LiDAR behaviour at close range likely dominate, so this capture cannot tell whether a convention problem remains in the pose/depth chain. A small-motion capture at 0.5–3 m is needed before any temporal calibration; until then no temporal value is changed.
- rgb_depth_ok is challenge-dependent (4/16): N_match is low relative to N_disc [INFERENCE: real luminance steps at the sampled depth discontinuities fall below grad_min_y at 256×192 on this scene; not measured per pixel].
