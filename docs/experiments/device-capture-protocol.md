# Capture protocol for physical-device experiments

Purpose: collect labelled real GXRW captures for the scene classes of the user brief §23.3, so that Geometry/ImageEvidence can measure false-positive, false-negative and inconclusive rates separately. No accuracy is claimed before these measurements exist.

## Setup

1. Build and install the Debug capture app: `scripts/ios.sh install` (holds `/tmp/galaxy-device.lock` on the Mac only for the install).
2. On the phone: open XRPL Galaxy in portrait or landscape (both are supported; the committed raster is always the sensor-native landscape image and the label records `ui_orientation`). Allow camera, motion and Bluetooth when asked (Bluetooth is optional).
3. Overlay toggles (bottom row): tile grid (8×6 commitment tiles, central stratum in cyan), LiDAR dots (Turbo colour = depth 0.2–5 m, size = confidence), rays from the principal point, colorized depth map, level/horizon line, top-down motion trail (last 2 s, 10 mm grid), ARKit feature points. They are display-only and never enter the committed data.

## Taking one capture

1. Pick the scene class in the top-left menu (it is remembered between shots).
2. Frame the subject at 0.3–3 m (depth chip ideally > 50 %).
3. Hold the phone naturally and press the shutter; no deliberate sweep is needed. Normal hand wobble is enough: the policy needs ≥ 0.5 mm of camera displacement on each side of t0 within 100–500 ms, and the app picks the neighbouring frames with the largest displacement. The motion chip is informational (yellow "still" when the phone rests on a support, red when moving too fast for tracking).
4. Keep framing for about half a second after the press (t+ is collected up to 0.5 s after the shutter).
5. Every attempt is saved automatically: `Documents/galaxy-fixtures/<uuid>.gxrw` when the raw capture was frozen, and always `<uuid>.label.json` (class, state, reasons, motion and depth summary). Do not retry silently: every attempt counts in the statistics.

After a session: `scripts/ios.sh pull-fixtures raw` copies the exports to `fixtures/real/capture-ios/` (gitignored). Nothing on the phone is deleted by this step.

## Scene classes (user brief §23.3)

Target ≥ 10 captures per class, from several positions and distances. "Expected" is the intended outcome of the physical-consistency policy; it is a hypothesis to measure, not a claim.

| # | class | how | expected |
|---|---|---|---|
| 1 | 3-D indoor scene | desk with objects at different depths, room corner, shelves | consistent |
| 2 | outdoor scene within sensor range | façade/vegetation/objects within ~5 m, daylight | consistent |
| 3 | desktop monitor | photo of a 3-D scene shown full-screen, phone 0.4–1 m from the monitor | not consistent |
| 4 | laptop screen | same as 3 on a laptop | not consistent |
| 5 | phone screen | photo on another phone held/propped at 0.2–0.4 m | not consistent |
| 6 | printed photograph | A4 print on a table and taped to a wall | not consistent |
| 7 | projected image | projector onto a wall/screen, room dimmed | not consistent |
| 8 | tilted screen | monitor/laptop tilted 20–45° relative to the phone | not consistent |
| 9 | screen surrounded by physical objects | monitor with real objects in front/around it, screen filling 30–70 % of the frame | not consistent / inconclusive |
| 10 | genuine flat wall | plain or textured real wall, no screen | consistent or inconclusive (planar real scene) |
| 11 | reflective surfaces | mirror, glass cabinet, glossy floor | measure |
| 12 | low-texture scene | white wall, uniform tabletop | measure (tracking/inconclusive expected) |
| 13 | low-motion capture | (a) hand-held as still as possible; (b) phone resting on a desk or support | (a) passes the 0.5 mm floor (measured ~3 mm); (b) INCONCLUSIVE (no motion) |
| 14 | dynamic scene | people walking, moving leaves, a fan | measure |

Controlled substitution of committed depth or RGB observations is done offline on copies of these fixtures by the evidence/geometry tests; the app never produces such captures.

## Log sheet (one row per attempt)

| capture UUID | class # | distance (m) | lighting | motion (as performed) | app result + reasons | notes |
|---|---|---|---|---|---|---|

## Rules

- Captures contain unredacted images, depth, motion and Bluetooth aggregates: shoot only scenes and people who agreed, keep files in `fixtures/real/` only, never publish them.
- Device access from the Mac is serialized with the device lock; keep windows short (the phone is shared).
- Do not delete captures or records on the phone; they are user data (spec §19).
