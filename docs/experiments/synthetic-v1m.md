# Synthetic scenario suite under galaxy-pc-v1m

All numbers below are measurements on **synthetic, ray-cast data** produced by `galaxy-geometry::synthetic` (development only; not device data). They say how the §8 predicates behave on these simulated scenes and nothing about real captures or real adversaries.

Policy: `policies/galaxy-pc-v1m.json`, policy_id `ef744a7f28dc606b117dbab34899753a218cd1865b4fc69a9739ac9c10a7afac` (after the m_cd calibration of ADR 0030). The synthetic IMU is derived through the policy m_cd, so the suite was run before and after the change; both runs produced identical tables.

Reproduce (release build, ~25 s per profile on the shared 8-CPU box):

```
CARGO_TARGET_DIR=target/Geometry scripts/with-slot.sh cargo build --release -p galaxy-geometry --features cli --bin geometry-tool
target/Geometry/release/geometry-tool report --seeds 20                 # PNG profile
target/Geometry/release/geometry-tool report --seeds 20 --profile jpeg  # JPEG profile
```

## Method
- Each scenario × seed 0..19 goes through the full path: render → canonical image encode (PNG, or JPEG with the committed reference decode replacing the frame-t0 analysis raster, as on device) → `build_view(&Evidence, &synthetic_challenge(seed))` → `evaluate`. The challenge (tiles, blocks, temporal picks, hypotheses) therefore differs per seed.
- Camera: 256×192 analysis grid, fx 210, fy 209, cx 127.5, cy 95.5 (iPhone-13-Pro-Max-wide-like after the §5 scaling); frames at −150/0/+150 ms.
- Hand motion (all scenarios except static/pure-rotation): translation 2–15 mm per side along a random direction, rotation 0.015–0.065 rad per side about a random axis, plus a random centre orientation. Pure rotation: translation 0. Static: no motion.
- Depth: LiDAR-like noise and confidence model (low confidence at depth edges, far range and low reflectance). IMU: 100 Hz gyro from the pose deltas through m_cd, noise 3 mrad/s, bias 1.5 mrad/s σ, gravity noise, user acceleration 25 mg σ.
- Scenes: room/desk (back wall, tabletop, cabinet, 18 boxes/spheres at 1.2–2.6 m with procedural checker textures); flat wall; monitors (frontal, tilted, curved, centre monitor surrounded by objects) showing a rendered 3D room photo as their texture; printed photo on a wall; low-texture (uniform grey) room; reflective (missing-depth) regions; a dynamic object moving between frames. Substitutions: depth+confidence of all frames from a different room layout with the same RGB; t−/t+ frame data swapped; frame-t0 RGB rendered from another viewpoint; IMU from another trajectory.

## Outcomes (PNG profile, n = 20 seeds per scenario)

| scenario | n | PASS | not PASS | quality false | geometry false | rgb_depth false | temporal false | motion false | sole failing predicate (counts) |
|---|---|---|---|---|---|---|---|---|---|
| room | 20 | 16 | 4 | 0 | 0 | 0 | 0 | 4 | motion 4 |
| flat-wall | 20 | 0 | 20 | 0 | 20 | 20 | 0 | 4 | - |
| frontal-monitor | 20 | 0 | 20 | 0 | 20 | 20 | 1 | 4 | - |
| tilted-monitor | 20 | 0 | 20 | 0 | 20 | 19 | 1 | 4 | geometry 1 |
| curved-monitor | 20 | 0 | 20 | 0 | 10 | 20 | 1 | 4 | rgb_depth 7 |
| centre-monitor | 20 | 2 | 18 | 0 | 16 | 3 | 1 | 4 | geometry 10, rgb_depth 1, motion 1 |
| print | 20 | 0 | 20 | 0 | 20 | 20 | 0 | 4 | - |
| pure-rotation | 20 | 0 | 20 | 0 | 0 | 0 | 0 | 20 | motion 20 |
| static | 20 | 0 | 20 | 0 | 0 | 0 | 0 | 20 | motion 20 |
| low-texture | 20 | 0 | 20 | 0 | 0 | 20 | 20 | 4 | - |
| reflective | 20 | 0 | 20 | 0 | 20 | 10 | 15 | 4 | geometry 1 |
| dynamic | 20 | 0 | 20 | 0 | 17 | 6 | 20 | 4 | temporal 2 |
| depth-substitution | 20 | 16 | 4 | 0 | 0 | 0 | 0 | 4 | motion 4 |
| swapped-frames | 20 | 1 | 19 | 0 | 0 | 0 | 19 | 4 | temporal 15 |
| viewpoint-substitution | 20 | 5 | 15 | 0 | 0 | 0 | 13 | 4 | temporal 11, motion 2 |
| imu-substitution | 20 | 0 | 20 | 0 | 0 | 0 | 0 | 20 | motion 20 |

JPEG profile: identical in every row except viewpoint-substitution (PASS 4, temporal false 14, sole temporal 12).

Medians (PNG):

| scenario | V_all | V_c | max_all | max_c | tiles_nonplanar | tiles_nonplanar_c | n_strong | tiles_strong | N_disc | N_match | N_usable | N_occ | N_dok | N_inf | N_pok |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| room | 254 | 127 | 24 | 21 | 16 | 8 | 196 | 16 | 32 | 30 | 60 | 0 | 60 | 57 | 57 |
| flat-wall | 256 | 128 | 232 | 120 | 2 | 1 | 0 | 0 | 0 | 0 | 61 | 0 | 60 | 61 | 61 |
| frontal-monitor | 256 | 128 | 233 | 120 | 2 | 1 | 0 | 0 | 0 | 0 | 61 | 0 | 60 | 54 | 54 |
| tilted-monitor | 256 | 128 | 232 | 119 | 2 | 1 | 0 | 0 | 0 | 0 | 61 | 0 | 60 | 53 | 53 |
| curved-monitor | 256 | 128 | 124 | 98 | 12 | 6 | 33 | 4 | 0 | 0 | 61 | 0 | 60 | 51 | 51 |
| centre-monitor | 254 | 128 | 137 | 119 | 11 | 3 | 80 | 7 | 11 | 10 | 61 | 0 | 60 | 59 | 59 |
| print | 256 | 128 | 233 | 121 | 2 | 1 | 0 | 0 | 0 | 0 | 61 | 0 | 60 | 61 | 61 |
| pure-rotation | 254 | 128 | 24 | 20 | 16 | 8 | 206 | 16 | 32 | 31 | 61 | 0 | 57 | 57 | 57 |
| static | 255 | 127 | 27 | 16 | 16 | 8 | 191 | 16 | 40 | 39 | 63 | 0 | 61 | 60 | 60 |
| low-texture | 254 | 127 | 24 | 21 | 16 | 8 | 196 | 16 | 32 | 0 | 60 | 0 | 60 | 0 | 0 |
| reflective | 98 | 8 | 0 | 0 | 8 | 1 | 0 | 0 | 6 | 5 | 20 | 0 | 20 | 20 | 20 |
| dynamic | 255 | 128 | 174 | 118 | 8 | 2 | 54 | 5 | 9 | 9 | 61 | 5 | 21 | 41 | 22 |
| depth-substitution | 255 | 127 | 26 | 19 | 16 | 8 | 200 | 15 | 36 | 33 | 61 | 0 | 59 | 58 | 58 |
| swapped-frames | 254 | 127 | 24 | 21 | 16 | 8 | 196 | 16 | 32 | 30 | 61 | 15 | 24 | 43 | 36 |
| viewpoint-substitution | 254 | 127 | 24 | 21 | 16 | 8 | 196 | 16 | 32 | 27 | 60 | 0 | 60 | 57 | 30 |
| imu-substitution | 254 | 127 | 24 | 21 | 16 | 8 | 196 | 16 | 32 | 30 | 60 | 0 | 60 | 57 | 57 |

## Reading the results
- **Common motion failures (4 per scenario).** All scenarios share the trajectory generator, so seeds 1, 2, 16, 17 have one side with a baseline below baseline_min_um = 3 mm (measured: 2.078, 2.233, 2.998, 2.140 mm). These are the only room failures; room PASS on the remaining 16/20. Seed 0 is the room fixture `test-vectors/policy/synthetic-room-pass.json`.
- **Planar scenes** (flat wall, frontal/tilted monitor, print): geometry_ok false on 20/20 (max_all ≈ 232 of 256 valid points on one plane, plane_global_max_pct 85; tiles_nonplanar 2 < 6) and rgb_depth_ok false (no depth discontinuities, N_disc 0). Temporal is consistent (a real plane moving rigidly), as expected.
- **Curved monitor**: never PASS, but geometry alone catches only 10/20 (the curvature makes the inverse-depth plane fit leave outliers); rgb_depth catches 20/20 (smooth surface, N_disc 0). Seven seeds are rejected only by rgb_depth.
- **Centre monitor surrounded by objects**: 2/20 PASS. Geometry (central planarity) rejects 16/20, rgb_depth 3/20. The two passing seeds are cases where the challenge-sampled central tiles still see enough of the surrounding 3D objects; this is the weakest of the planar-display cases under v1m.
- **Pure rotation / static**: rejected 20/20 by motion only (baseline); all other predicates pass.
- **Low texture**: rgb_depth (N_match 0) and temporal (N_inf 0 < inf_min 8) reject 20/20; geometry passes (3D scene).
- **Reflective / missing depth**: geometry rejects 20/20 (V_c 8 ≪ v_c_min 64).
- **Dynamic object**: temporal rejects 20/20 (N_dok 21 of 61 usable); geometry also fails on 17/20 because the large moving object dominates the centre.
- **Swapped t−/t+**: temporal rejects 19/20 (N_occ 15, N_dok 24); 1/20 PASS where the two sides' motions are nearly mirror-symmetric.
- **RGB from another viewpoint** (frame t0): temporal rejects 13/20 (PNG) / 14/20 (JPEG) via photo_ok (median N_pok 30 of N_inf 57 vs pok_min_pct 60); 5/20 (PNG) and 4/20 (JPEG) PASS. photo_ok accepts a match against any of the 16 pixels of the opened frame-j block within photo_tol_y (20 luma levels), which is lenient on dense textures.
- **IMU from another capture**: motion (gyro) rejects 20/20.
- **Depth from another scene with the same RGB**: **not detected** — 16/20 PASS, exactly the room outcome. The substituted depth is a different but internally consistent 3D layout for all three frames and the same poses, so geometry and temporal depth consistency hold; rgb_depth only checks that sampled depth discontinuities coincide with luminance edges (N_match 33 of N_disc 36), and the procedural textures have luminance steps above grad_min_y almost everywhere. Photo consistency uses RGB only. Under v1m this substitution is caught only when the substitute's discontinuities fall on texture-free regions.

## Limits of these measurements
- 20 seeds per scenario; differences of 1–2 seeds are within sampling noise.
- The renderer has no motion blur, rolling shutter, exposure change or LiDAR temporal smoothing; real captures may behave differently (see `real-captures-v1m.md`).
- No §8.7 value was tuned for these results.

## Calibration notes (proposals, not applied)
- The depth-substitution and viewpoint-substitution results come from texture density relative to grad_min_y/photo_tol_y and from the one-directional rgb_depth predicate; raising thresholds would not close them on dense textures. A formula-level change (e.g. requiring luminance edges to be matched by depth discontinuities in a planar-texture-insensitive way) would be a contract change and needs its own experiment.
- The 4/20 room motion failures come from the 2 mm lower bound of the synthetic hand-motion model versus baseline_min_um 3 mm, not from the predicate.
