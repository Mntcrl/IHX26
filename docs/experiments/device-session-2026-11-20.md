# Device capture session — 2026-11-20 (iPhone 13 Pro Max, iOS 27.2.1)

Data: `fixtures/real/capture-ios/` (private, gitignored): 77 attempt labels, 55 raw GXRW captures (2 earlier unlabelled captures from 20:44 included), pulled with `scripts/ios.sh pull-fixtures raw` at 23:09; the device and Linux file lists match. All figures below are aggregates; no images or raw values are published.

## Acquisition
- App builds used: 22:31 (labels), 22:39 (overlays), 22:50 (t0 = first frame after the press), 22:54 (neighbour frames with the largest displacement in 100–500 ms), 22:57/23:04 (motion floor 0.5 mm per side).
- Attempts by outcome: 30 frozen without issue; 24 "no camera frame" (all before the 22:50 fix); 21 "insufficient motion" under the former 3 mm rule (before 23:04); 2 legacy captures with very large motion (15–22 cm, 26°).
- With the 23:04 build: 8 of 8 attempts frozen without issue; per-side camera displacement 1.0–40 mm (natural hand wobble). Orientation: 75 portrait, 2 landscape.
- All 55 GXRW files ingest through `galaxy-evidence` (no rejection).

## Reference engine (policy galaxy-pc-v1m, id b27f0afc…6aee), 8 synthetic challenges per capture
Command: `geometry-tool calibrate <dir-by-class> --challenges 8` (classes from the label sidecars).

| class (captures) | PASS / evaluations | main failing predicates |
|---|---|---|
| 3-D indoor scene (15) | 87 / 120 (72.5 %) | rgb_depth 22, temporal 12, geometry 9 |
| unlabelled (9) | 56 / 72 | rgb_depth 14 |
| laptop screen (5) | 23 / 40 | geometry 8, rgb_depth 14 |
| phone screen held in hand, filling ~60 % of the frame (6) | 0 / 48 | geometry 48, temporal 45 |
| screen among objects (4) | 28 / 32 | rgb_depth 4 |
| genuine flat wall (5) | 0 / 40 | geometry 37, rgb_depth 38 |
| reflective surfaces (5) | 2 / 40 | temporal 37, geometry 17 |
| low motion, hand-held (1) | 2 / 8 | rgb_depth 5, geometry 3 |
| dynamic scene (3) | 17 / 24 | rgb_depth 7 |
| legacy large-motion (2) | 0 / 16 | temporal 16, motion 16 |

Visual inspection of a sample: the "laptop screen" and "screen among objects" captures show laptops on desks with UI content inside real 3-D surroundings (keyboard, bags, people), i.e. genuine scenes containing a screen, not recaptures of a displayed 3-D photograph filling the frame. Passing them is the intended behaviour; they do not test acceptance criterion 19. The phone-screen captures do frame a displayed image and are rejected (0/48).

## Sensor-consistency results
- quality_ok and motion_ok held on every evaluation except the two legacy large-motion captures.
- Gyro vs ARKit relative rotation (`geometry-tool imu-frame`, 53 captures): best device→camera matrix [0,-1,0,-1,0,0,0,0,-1] (the calibrated policy value); RMS residual 0.56 mrad, maximum axis residual 1.88 mrad; runner-up candidate 7.4 mrad. The policy tolerance (20 mrad) has a large margin; a tighter value is a calibration candidate.
- Temporal depth consistency on 3-D scenes with natural motion: median N_dok 56 of N_usable 59 — the reprojection chain (poses, intrinsics, depth) is consistent on real data, which the two earlier large-motion captures could not establish.

## Gaps before the acceptance experiments (criterion 19)
- Recapture configurations not yet captured: monitor or laptop showing a full-screen photograph of a 3-D scene filling most of the frame (frontal and tilted 20–45°), printed photograph (table, wall), screen showing a 3-D photograph with real objects in front filling 40–70 % of the frame, projected image.
- Missing classes: outdoor, low-texture; only one hand-held low-motion capture; no phone-resting-on-support capture (needed to confirm the 0.5 mm floor rejects it).
- Calibration candidates for the next policy revision (with these data): rgb_depth thresholds (dominant false-negative cause on 3-D scenes), gyro_tol_urad, reflective-surface behaviour.
