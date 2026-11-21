# Device capture diagnostics — 2026-11-21

Both iPhone 13 Pro Max verdicts were correct for their validated XRPL Testnet challenges. The same JPEG evidence passed only **1/64** and **0/64** off-ledger challenges, respectively. Change the scene rather than trying to get a more favorable challenge.

## Exact Testnet replay

Both CAPTURE transactions were validated with `tesSUCCESS`. The Linux replay decoded each CAPTURE memo, computed the capture ID from its core, and used the validated hash of `Lsel = L0 + 5`. Both ledger hashes and capture IDs matched the corresponding device checkpoints. The on-ledger cores set the `jpeg-v1` profile and identify `galaxy-pc-v1m` (`4ad25185…67073`). Linux `geometry-tool eval --profile jpeg` on GXRW returned the same predicate verdicts as the device. A separate replay decoded the GXEV payload from the iOS checkpoint and produced the same `GeometryView` and report. Both the GXRW re-ingest and the restored GXEV reproduced the on-ledger `photo_id`, `rgb_root` and `evidence_root` using the checkpoint salts. No profile, pose, frame selection or quantization discrepancy was found for these attempts.

| CAPTURE | Lpre / L0 / Lsel | Device and Linux failures | What missed on the real challenge |
|---|---|---|---|
| `AE15BEB0…C7CBA7` | 22494161 / 22494164 / 22494169 | Geometry, RGB–depth | Central planar support 123/124 (99.2%, **19.2 pp over**); 0 nondegenerate global fits (need ≥1); 0% strong points and 0 strong tiles (need ≥10% and ≥3); 0 depth discontinuities (need ≥6). |
| `61973477…E5ABF8` | 22494173 / 22494176 / 22494181 | Geometry, temporal | Central planar support 122/128 (95.3%, **15.3 pp over**); 1 central nonplanar tile (need ≥3); 4 informative temporal pairs (need ≥8), despite 61/62 depth-consistent pairs and 3/4 photometric matches. |

In the first attempt the engine reports 12 nonplanar tiles (8 central) even though none of the global fits was usable: without a fit the outlier counter counts every valid point. This is not evidence of nonplanarity. The independent central fits were valid and exposed the dominant plane. The second attempt had 71.7% global planarity and 6 nonplanar tiles overall, both passing; the failure was concentrated in the center.

## 64 off-ledger challenges per attempt

Challenge derivation used 64 independent random 256-bit values in place of the selection-ledger hash, holding each real core and selection index fixed. These are diagnostic evaluations, not substituted ledger challenges or new submissions. Failure counts overlap.

| CAPTURE | PASS | Quality failures | Geometry failures | RGB–depth failures | Temporal failures | Motion failures |
|---|---:|---:|---:|---:|---:|---:|
| `AE15BEB0…` | 1/64 (1.6%) | 0 | 63 | 42 | 0 | 0 |
| `61973477…` | 0/64 | 0 | 64 | 18 | 31 | 0 |

Ranges below are **min / median / max**, followed by the count missing the indicated threshold. Percentage summaries exclude samples with zero denominators; the separate minimum-count tests still apply.

| Policy statistic | Threshold | `AE15BEB0…` range (misses) | `61973477…` range (misses) |
|---|---:|---:|---:|
| Valid sampled depth, all / central | ≥128 / ≥64 | 164/188/228 and 120/128/128 (0 each) | 233/253.5/256 and 128/128/128 (0 each) |
| Covered tiles | ≥12 | 10/12/15 (20) | 16/16/16 (0) |
| Valid global hypotheses | ≥1 | 0/2/4 (12) | 2/4/4 (0) |
| Valid central hypotheses | ≥1 | 2/4/4 (0) | 2/4/4 (0) |
| Global plane support | ≤85% | 0/57.1/100% (13) | 6.8/73.6/90.6% (11) |
| Central plane support | ≤80% | 35.2/93.8/100% (60) | 85.2/96.1/100% (64) |
| Nonplanar tiles, total | ≥6 | 0/6.5/15 (29) | 2/5/16 (35) |
| Nonplanar tiles, central | ≥3 | 0/4/8 (30) | 0/1/8 (47) |
| Strong points | ≥10% | 0/11.2/87.0% (29) | 9.1/25.0/90.5% (2) |
| Strong tiles | ≥3 | 0/3/13 (24) | 2/5/16 (5) |
| Depth discontinuities | ≥6 | 0/3/15 (42) | 0/8/35 (18) |
| RGB edge match among discontinuities | ≥40% | 66.7/100/100% (0; 18 undefined) | 0/100/100% (1; 6 undefined) |
| Usable temporal pairs | ≥24 | 39/48/60 (0) | 59/63/64 (0) |
| Occluded fraction of usable pairs | ≤40% | 0/0/2.4% (0) | 0/0/5.0% (0) |
| Depth-consistent among usable, unoccluded pairs | ≥70% | 94.0/100/100% (0) | 93.2/100/100% (0) |
| Informative temporal pairs | ≥8 | 30/40/50 (0) | 0/8/16 (31) |
| Photometric match among informative pairs | ≥60% | 97.5/100/100% (0) | 81.8/100/100% (0; 1 undefined) |

The first attempt was usually too planar in the center (median **13.8 pp over** the cap) and lacked sampled depth edges (median **3 below** the minimum), not low RGB-edge agreement when a depth edge existed. The second attempt was **at least 5.2 pp over** the central planarity cap on all 64 draws. Its temporal failures came from too few local luminance contrasts to make a pair informative, not from the depth reprojection itself.

| Aggregate | `AE15BEB0…` | `61973477…` |
|---|---:|---:|
| Center-frame policy-valid depth coverage | 68.1% | 98.7% |
| Valid depth p05 / p50 / p95 | 0.33 / 0.43 / 0.63 m | 0.72 / 1.60 / 1.87 m |
| Translation baselines, minus / plus (each ≥0.50 mm) | 1.32 / 0.99 mm | 3.39 / 8.46 mm |
| Relative rotation, minus / plus | 0.13° / 0.15° | 0.62° / 0.72° |
| Largest normalized rotation skew component (limit 0.70) | 0.0046 | 0.0167 |
| Largest per-axis gyro residual (limit 20 mrad) | 0.59 mrad | 0.37 mrad |
| Exact depth residual p50 / p95 (per-pair limit 30‰) | 1.54 / 6.15‰ | 0.95 / 8.79‰ |
| Off-ledger depth residual p50 / p95 | 1.47 / 13.68‰ | 0.87 / 10.29‰ |
| Exact photometric residual p50 / p95 (per-pair limit 20 luma levels) | 0.41 / 2.82 | 8.62 / 34.77 |
| Off-ledger photometric residual p50 / p95 | 1.00 / 5.23 | 0.93 / 9.91 |

Depth residuals use the policy's absolute projected-vs-observed depth error divided by observed depth, over usable unoccluded pairs. Photometric residuals use the minimum luma difference in the target block over informative pairs (4 pairs for the second exact challenge). Off-ledger residuals pool selected pairs; a pixel can be sampled on more than one draw. Motion and full-frame coverage are invariant across draws.

## Ranked genuine 3-D calibration captures

Used all 44 included `genuine-3d` GXRWs selected by the private calibration manifest, re-ingested as `jpeg-v1` and evaluated 64 times each under the same policy. Calibration replays used the canonical photo ID as a diagnostic capture identifier, a fixed selection index, and 64 random 256-bit mock ledger hashes shared across the fixture set. **2,010 / 2,816 (71.4%)** passed. The scene-level rank is summarized below as `PASS out of 64 × number of captures`, in descending order:

`64×2, 63×4, 62×2, 60×3, 59, 58×3, 57, 56, 55×2, 54, 52, 51, 49×3, 48, 46, 44, 41, 38, 37, 36×2, 35, 33, 31×2, 29, 20, 18, 15×2, 9, 4`.

| Scene characteristic | Top 11 (≥60 PASS) | Bottom 11 (≤35 PASS) |
|---|---:|---:|
| Median near / middle / far valid depth (per-capture p05 / p50 / p95) | 0.80 / 2.04 / 2.94 m | 1.61 / 2.35 / 4.31 m |
| p95−p05 valid depth span, median (range) | 2.26 m (0.72–3.07) | 2.53 m (1.25–3.81) |
| Median valid coverage (range) | 91.8% (87.9–95.7%) | 90.5% (70.7–94.5%) |
| Typical sampled nonplanar tiles, total / central | 16/16 and 8/8 | 14/16 and 7/8 |
| Typical sampled depth discontinuities | 19.5 | 6 |
| Typical informative temporal pairs | 27 | 14 |
| Per-side baseline, median (range) | 5.21 mm (1.04–55.29) | 8.52 mm (2.66–85.91) |
| Per-side rotation, median (range) | 0.63° (0.27–7.61°) | 1.05° (0.21–9.13°) |

“Typical” is the median of per-capture medians over the 64 draws. Calibration sidecars have class labels, not per-shot scene notes. The manifest confirms ten top captures as indoor 3-D controls and describes the remaining one as an indoor capture with people and furniture. These results favor visible depth and luma boundaries distributed across the view. A wide overall depth range or good depth coverage alone did not separate the top and bottom groups. The calibration set helped select this policy, so these are descriptive rates for the available captures, not a new held-out accuracy estimate or a guarantee of certification.

## How to frame the demo capture

1. Aim **across** a shelf, counter or desk, not straight down onto a bare tabletop. Put multiple large objects at different depths across the **center**, with more on either side so varied depth falls in both sampling strata.
2. Start with a foreground object roughly **0.6–1 m** away and leave room for textured middle and background surfaces out to **about 3 m**. Avoid filling the view with a single plane, even one with good LiDAR coverage.
3. Use matte objects with visible contrasting outlines and surface details, e.g. books or boxes of different heights in front of a textured background. Avoid glass, mirrors, screens and a large plain patch in the center.
4. Light the scene evenly, keep it static, and wait for normal tracking and broad depth coverage before taking the shot. A contrast boundary that coincides with a depth boundary is more useful than adding texture to a single flat surface.
5. Hold the phone in your hands and drift slightly sideways through the shot without a fast pan, shake or large pivot. Don't brace it on a support: the capture still needs measurable translation on both sides of the shutter.

The two authorized source captures, raw evidence checkpoints and per-capture ranking remain private; no images, witness data, salts or challenge seeds are included here.

## Demo scene rehearsal

These private calibration-mode captures do not submit transactions. Each replay uses `jpeg-v1`, the final policy and 64 independent off-ledger challenges. A high PASS count supports the scene choice but does not guarantee the live outcome.

Production rehearsal did not produce any calibration exports for this table. To run a calibration rehearsal on the demo workstation:

1. On the phone, enable **Developer → Enable private calibration captures and overlays**, then **Automatically export each fixture and label**. Take a calibration shot and wait for its export to complete.
2. **Only after the user consents to copying the rehearsal files**, run `bash fixtures/real/DemoDiag/rehearse.sh --consented` from the repository root. It takes the device lock, reads only new calibration raw-and-label pairs from the designated phone after 10:20 CET, and saves them privately under `fixtures/real/DemoDiag/rehearsal/`. It emits one `REHEARSAL` summary per new pair; a successful run with no summaries found no new complete pairs. It does not launch, install, delete or write anything on the device.
3. For an already-authorized local GXRW, use `bash fixtures/real/DemoDiag/rehearse.sh --local <local.gxrw>`; this never contacts the phone. The command uses the demo workstation's private replay driver and cached release libraries.

| Rehearsal | PASS | Failing predicates and most frequent threshold misses | Suggested adjustment |
|---|---:|---|---|
