# Scene-held-out calibration of `galaxy-pc-v1m`

## Protocol (fixed before opening the holdout)

The private curated manifest contains 99 included iPhone captures from one operator and one indoor environment. Its class labels were reviewed before this calibration. Never publish the manifest, identifiers, images, raw sensor values, or opened views. The commands below read private data and emit only class-level aggregates (or public policy values).

1. **Scene split.** Within each curated class, sort by claimed time (capture identifier breaks ties) and group adjacent captures into the same scene proxy when their gap is at most 120 seconds. Hold out 30% of the scenes per class, rounded to the nearest integer and bounded by one and `n − 1` for classes with two or more scenes. Among equally sized scene subsets, choose the one closest to 30% of that class's captures; break ties by earlier chronological scene indices. A one-scene class stays entirely in training rather than leaking near-duplicates into the test set. This is a time/class proxy, not independent verified scene IDs: the same room, surface, or device can recur between bursts or across classes.
2. **Evaluation unit.** Ingest GXRW with the production PNG profile and apply the Rust reference engine to 32 deterministic pseudo-ledger challenges per capture. The challenge domain incorporates the private manifest capture identifier so those 32 challenges are not reused across captures. Use the same capture/challenge pairs for baseline and calibrated policy. `PASS` here is the conjunction of the five §8 policy predicates only, not verification of provenance, commitments, a production ledger challenge, or a proof.
3. **Training objective.** Cap PASS at 2% per challenge for screen recaptures and observable printed photos, and at 5% for each of flat wall and screen on flat surface. A print is observable if at least half the full and half the central t0 raster have depth in the *baseline* 200–5000 mm window with confidence ≥1; the denominator therefore cannot shrink when a candidate raises the depth floor. Among candidates satisfying the caps, maximize the pooled genuine-3d, genuine-scene-with-screen, and dynamic PASS count. Rank infeasible candidates first by their summed excess per-class ppm, then by missed genuine challenges; break score ties in favor of fewer fields changed and then first visited. Record low motion, reflective and no motion separately without fitting to them.
4. **Search and lock.** Use the finite value grid in `crates/galaxy-geometry/src/bin/geometry-tool/study.rs` (no formula or shape changes). Start at each candidate depth floor with its best inlier exponent at the baseline strong band. Refine the three top-scoring depth starts by coordinate search in the fixed field order, stopping on a stable score or after four sweeps per start. This is a bounded search, not a claim of a global optimum. Keep the 500 µm baseline floor and the calibrated device-to-camera matrix fixed. The original fit locked its policy before opening the holdout; it used the synthetic suites only as post-lock diagnostics.

The original fit was rejected: its reduced nonplanar tile counts allowed the synthetic centre-monitor case to pass 19/20 instead of 3/20 in both profiles. The constrained refit uses the same real scene split, objective and value grid, without scoring candidates on the real holdout. It freezes `nonplanar_tiles_min=6`, `nonplanar_tiles_c_min=3`, `plane_global_max_pct=85`, `plane_central_max_pct=80`, and `tile_out_min=4`. It rejects any candidate that increases the number of PASS results over baseline on seeds 0–19 in **either** PNG or JPEG for any of: frontal, tilted, curved or centre monitor; print; flat wall; depth substitution; swapped frames. The guarded synthetic cases are model-selection data for the constrained fit, not an independent evaluation. The real holdout was reused after the first fit, so a fresh external test is still needed.

Reproduce with `geometry-tool --policy <baseline.json> calibration-fit fixtures/real/calibration` (add `--unconstrained` to reproduce the original fit). For each fixed policy, use `geometry-tool --policy <policy.json> calibration-measure fixtures/real/calibration` and the same command with `--test`. Baseline is the policy with ID `b27f0afc072bd1382d49f2313ed3f4dd7de69a00cae0a517b95ab8d20b8a6aee` from `b4700cc`.

## Final constrained policy

`4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073` differs from baseline in four values: `lidar.d_min_mm` 200 → 325, `rgb_depth.grad_min_y` 2560 → 1280, `rgb_depth.edge_match_pct` 50 → 40, and `temporal.occ_pm` 50 → 65. All five frozen geometry thresholds and both planarity band widths remain at baseline. The 325 mm floor stays below the unchanged 5000 mm ceiling and narrows the valid LiDAR input domain. The generated RGB thresholds fit the derived comparator widths; the reduced temporal occlusion multiplier fits the existing width. No formulas or structural constants changed.

The winning training score was zero excess over the four real negative caps, 419 missed genuine evaluations, and four value changes. The synthetic caps were hard eligibility criteria, not extra weighted objectives.

## Rejected original fit

`f3999a9faa4d5ff2af9e6cf9be7ebd5da3122bc4ab7475db9be63bb220016638` changed nine values:

| Group | Before → after |
|---|---|
| LiDAR | `d_min_mm` 200 → 350; `eps_log2` 26 → 25; `nonplanar_tiles_min` 6 → 4; `nonplanar_tiles_c_min` 3 → 2 |
| RGB–depth | `disc_pct` 4 → 3; `grad_min_y` 2560 → 1792; `disc_min` 6 → 3; `edge_match_pct` 50 → 45 |
| Temporal | `occ_pm` 50 → 65 |

Training search returned zero excess over the four class caps, 272 missed genuine evaluations and nine changes. The new depth floor remains below the unchanged 5000 mm maximum and does not expand the set of vertices used in the bounded LiDAR arithmetic. The inlier exponent 25 and strong exponent 28 use the existing `GeoBand(58, …)` decomposition; no new bit range is needed. RGB comparison widths are derived from the generated thresholds. The occlusion factor `1000 − occ_pm` decreases from 950 to 935 and stays within its existing comparator bound. No formulas or structural constants changed.

## Scene split

23 captures in seven scene proxies were held out; 76 captures in 22 scene proxies were used for training. Of the 25 proxies in classes that could be split, seven (28%) were held out.

| Class | Chronological scene sizes (`*` held out) |
|---|---|
| genuine-3d | 2, 5*, 3, 9*, 2, 2, 2, 19 |
| genuine-scene-with-screen | 4*, 2, 2, 2 |
| dynamic | 1*, 1, 1 |
| screen-recapture | 2*, 1, 1, 10 |
| printed-photo | 6 |
| flat-wall | 3, 1*, 1 |
| screen-on-flat-surface | 10 |
| genuine-3d-low-motion | 1 |
| reflective | 3, 1*, 1 |
| no-motion | 1 |

The held-out screen proxy is one phone-screen setup; the laptop screen showing a photo is in training. The printed photo and phone-on-table classes each have one chronological burst. The two baseline-valid prints are not two independent scenes.

## Real captures: baseline and both fits

PASS rates for genuine labels are true-accept rates; the complement is the false-reject rate. PASS rates for recaptures and flat controls are false-accept rates for this experiment. Reflective and no-motion are diagnostic controls. `Reject` and `Any/all` refer to the final policy; the latter counts captures passing at least one/all 32 challenges. `Failures` counts false predicates in quality/geometry/RGB–depth/temporal/motion order, including evaluations with multiple failures; `Sole failures` counts only evaluations with exactly one false predicate. A dash with zero captures means unmeasured.

### Training

| Class | Captures | Baseline PASS | Original PASS (rejected) | Final PASS | Reject | Any/all | Failures | Sole failures |
|---|---:|---:|---:|---:|---:|---:|---|---|
| genuine-3d | 30 | 570/960 (59.38%) | 738/960 (76.88%) | 603/960 (62.81%) | 357 | 30/1 | 0/85/282/61/0 | geometry 34, RGB–depth 232, temporal 27 |
| genuine-scene-with-screen | 6 | 133/192 (69.27%) | 145/192 (75.52%) | 139/192 (72.40%) | 53 | 6/1 | 0/24/39/2/0 | geometry 12, RGB–depth 27, temporal 2 |
| dynamic | 2 | 54/64 (84.38%) | 61/64 (95.31%) | 55/64 (85.94%) | 9 | 2/0 | 0/1/8/0/0 | geometry 1, RGB–depth 8 |
| screen-recapture | 12 | 24/384 (6.25%) | 0/384 (0.00%) | 0/384 (0.00%) | 384 | 0/0 | 0/384/364/384/0 | None |
| printed-photo (all) | 6 | 25/192 (13.02%) | 0/192 (0.00%) | 0/192 (0.00%) | 192 | 0/0 | 0/192/168/192/0 | None |
| ↳ baseline-valid depth | 2 | 25/64 (39.06%) | 0/64 (0.00%) | 0/64 (0.00%) | 64 | 0/0 | 0/64/42/64/0 | None |
| ↳ insufficient baseline-valid depth | 4 | 0/128 (0.00%) | 0/128 (0.00%) | 0/128 (0.00%) | 128 | 0/0 | 0/128/126/128/0 | None |
| flat-wall | 4 | 0/128 (0.00%) | 6/128 (4.69%) | 1/128 (0.78%) | 127 | 1/0 | 0/117/120/68/0 | geometry 2, RGB–depth 7 |
| screen-on-flat-surface | 10 | 63/320 (19.69%) | 14/320 (4.38%) | 16/320 (5.00%) | 304 | 3/0 | 0/294/255/170/0 | geometry 46, RGB–depth 10 |
| genuine-3d-low-motion | 1 | 17/32 (53.12%) | 19/32 (59.38%) | 17/32 (53.12%) | 15 | 1/0 | 0/8/13/0/0 | geometry 2, RGB–depth 7 |
| reflective | 4 | 4/128 (3.12%) | 4/128 (3.12%) | 4/128 (3.12%) | 124 | 2/0 | 0/66/0/119/0 | geometry 5, temporal 58 |
| no-motion | 1 | 0/32 (0.00%) | 0/32 (0.00%) | 0/32 (0.00%) | 32 | 0/0 | 0/32/32/32/32 | None |

Pooled genuine objective PASS was 757/1216 (62.25%) at baseline, 944/1216 (77.63%) for the rejected fit, and 797/1216 (65.54%) for the final fit (419/1216, or 34.46%, false rejects). Both fits met all four training caps; the final phone-on-flat-surface result is exactly at its cap. Wall false accepts were 0 at baseline, 6 for the rejected fit, and 1 for the final fit. Zero passes on close screen and print samples does not establish planarity-based discrimination: both geometry and temporal depend on depth validity, so the higher floor can reject a sample before meaningful planarity or agreement checks.

### Held out (each fit measured after lock)

| Class | Captures | Baseline PASS | Original PASS (rejected) | Final PASS | Reject | Any/all | Failures | Sole failures |
|---|---:|---:|---:|---:|---:|---:|---|---|
| genuine-3d | 14 | 389/448 (86.83%) | 412/448 (91.96%) | 395/448 (88.17%) | 53 | 14/4 | 0/11/23/20/0 | geometry 11, RGB–depth 22, temporal 19 |
| genuine-scene-with-screen | 4 | 63/128 (49.22%) | 64/128 (50.00%) | 63/128 (49.22%) | 65 | 2/1 | 0/64/58/63/0 | RGB–depth 1 |
| dynamic | 1 | 16/32 (50.00%) | 21/32 (65.62%) | 16/32 (50.00%) | 16 | 1/0 | 0/6/12/0/0 | geometry 4, RGB–depth 10 |
| screen-recapture | 2 | 0/64 (0.00%) | 0/64 (0.00%) | 0/64 (0.00%) | 64 | 0/0 | 0/64/61/64/0 | None |
| printed-photo (all) | 0 | — | — | — | — | — | — | — |
| ↳ baseline-valid depth | 0 | — | — | — | — | — | — | — |
| ↳ insufficient baseline-valid depth | 0 | — | — | — | — | — | — | — |
| flat-wall | 1 | 0/32 (0.00%) | 0/32 (0.00%) | 0/32 (0.00%) | 32 | 0/0 | 0/32/30/25/0 | None |
| screen-on-flat-surface | 0 | — | — | — | — | — | — | — |
| genuine-3d-low-motion | 0 | — | — | — | — | — | — | — |
| reflective | 1 | 0/32 (0.00%) | 0/32 (0.00%) | 0/32 (0.00%) | 32 | 0/0 | 0/26/0/31/0 | geometry 1, temporal 6 |
| no-motion | 0 | — | — | — | — | — | — | — |

Held-out genuine-3d improved by six challenge passes (86.83% → 88.17%, +1.34 percentage points); false rejects were 53/448 (11.83%) compared with 59/448 (13.17%) at baseline and 36/448 (8.04%) for the rejected fit. Pooled genuine PASS was 468/608 (76.97%) at baseline, 497/608 (81.74%) for the rejected fit, and 474/608 (77.96%) for the final fit (134/608, or 22.04%, false rejects). The held-out screen and wall classes each represented only one scene proxy. There were no held-out prints or phone-on-table scenes.

## Synthetic suites: baseline and both fits

`geometry-tool report --seeds 20 --profile png` and `--profile jpeg`; all 16 scenarios, seeds 0–19, same seeds/challenge derivation for all three policies. PASS counts are out of 20 for each profile. Failure notation is the same as above. The constrained fit was eligible only if each of the eight guarded scenarios did not increase its PASS count in either profile.

| Scenario | Baseline PASS PNG/JPEG | Original PASS (rejected) | Final PASS PNG/JPEG | Final PNG failures | Final JPEG failures |
|---|---:|---:|---:|---|---|
| room | 20 / 20 | 20 / 20 | 20 / 20 | 0/0/0/0/0 | 0/0/0/0/0 |
| flat-wall | 0 / 0 | 0 / 0 | 0 / 0 | 0/20/20/0/0 | 0/20/20/0/0 |
| frontal-monitor | 0 / 0 | 0 / 0 | 0 / 0 | 0/20/20/1/0 | 0/20/20/1/0 |
| tilted-monitor | 0 / 0 | 0 / 0 | 0 / 0 | 0/20/19/1/0 | 0/20/19/1/0 |
| curved-monitor | 0 / 0 | 0 / 0 | 0 / 0 | 0/10/20/1/0 | 0/10/20/1/0 |
| centre-monitor | 3 / 3 | 19 / 19 | 3 / 3 | 0/16/3/1/0 | 0/16/3/1/0 |
| print | 0 / 0 | 0 / 0 | 0 / 0 | 0/20/20/0/0 | 0/20/20/0/0 |
| pure-rotation | 0 / 0 | 0 / 0 | 0 / 0 | 0/0/0/0/20 | 0/0/0/0/20 |
| static | 0 / 0 | 0 / 0 | 0 / 0 | 0/0/0/0/20 | 0/0/0/0/20 |
| low-texture | 0 / 0 | 0 / 0 | 0 / 0 | 0/0/20/20/0 | 0/0/20/20/0 |
| reflective | 0 / 0 | 0 / 0 | 0 / 0 | 0/20/10/15/0 | 0/20/10/15/0 |
| dynamic | 0 / 0 | 0 / 0 | 0 / 0 | 0/17/6/20/0 | 0/17/6/20/0 |
| depth-substitution | 20 / 20 | 20 / 20 | 20 / 20 | 0/0/0/0/0 | 0/0/0/0/0 |
| swapped-frames | 1 / 1 | 1 / 1 | 1 / 1 | 0/0/0/19/0 | 0/0/0/19/0 |
| viewpoint-substitution | 7 / 6 | 7 / 6 | 7 / 6 | 0/0/0/13/0 | 0/0/0/14/0 |
| imu-substitution | 1 / 1 | 1 / 1 | 1 / 1 | 0/0/0/0/19 | 0/0/0/0/19 |

Centre-monitor geometry failures fell from 16/20 at baseline to 0/20 in the rejected fit and returned to 16/20 in the final fit, in both profiles. RGB–depth failures for this case were 3/20 at baseline, 0/20 in the rejected fit and 3/20 in the final fit. Final centre-monitor passes are back to baseline, not zero. The final policy also does not detect coherent depth from another scene paired with the original RGB (20/20 PASS, identical to the genuine room for every fit). The separately moved large object in the synthetic dynamic scene failed temporal 20/20 in both profiles, even though genuine dynamic captures passed in both real splits.

## Limitations and next device session

- One device, one indoor environment, one operator/label review. Time proxies do not certify independent scene identity, and neither multiple captures in one scene nor 32 challenges on one capture give independent scene trials. The empirical caps are not population error bounds.
- The constrained synthetic gates prevent a measured regression on these exact fixtures; they do not establish the real-world false-accept rate. The depth-substitution gate is vacuous on a baseline that already passes all 20 seeds per profile.
- Before any deployment acceptance, collect independent device scenes and reserve unseen target images, devices and rooms for testing. In particular: recaptured laptop/monitor photos at 0.35–1.5 m (frontal and 20–45° tilted; full frame and centred with genuine surroundings), phone screens across distance/brightness/fill, and prints on a table and wall at 0.35–0.60 m with valid lidar coverage. Include paired genuine scenes at the same distances (especially around 0.35 m), fresh flat walls and tables, screens on planar surfaces and projected images.
- Add more outdoor, low-texture, reflective and moving 3-D scenes, multiple orientations, normal hand wobble and a phone resting on a support. Use new scenes, not more challenges on these same captures, to measure generalization. No additional device run was done for this calibration.
- A formula change would require a new contract and separately held-out evaluation. Candidate directions—not implemented or measured—are depth-conditioned noise bands plus a spatially coherent centre-plane test (to test the centre-monitor failure) and locally distinctive, bidirectional RGB/depth edge agreement rather than counting a luminance edge anywhere within the current tolerance (to test depth substitution). Compare against genuine low-texture and near-planar backgrounds. The 500 µm motion requirement does not supply reliable pixel-scale parallax at the analysis resolution.
