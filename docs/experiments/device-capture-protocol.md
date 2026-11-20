# Capture protocol for physical-device experiments

Purpose: collect labelled real GXRW captures for the scene classes of the user brief §23.3, so that Geometry/ImageEvidence can measure false-positive, false-negative and inconclusive rates separately. No accuracy is claimed before these measurements exist.

## Setup

1. Build and install the Debug capture app: `scripts/ios.sh install` (holds `/tmp/galaxy-device.lock` on the Mac only for the install).
2. On the phone: open XRPL Galaxy and hold it in landscape with the top edge of the phone (camera side) to your left, i.e. the camera-native orientation; the UI only allows this orientation. Allow camera, motion and Bluetooth when asked (Bluetooth is optional).
3. Open **Developer** to check: camera "Tracking normal", video format 1920×1440 @ 30 fps, depth coverage, CoreMotion "Receiving", current baseline.

## Taking one capture

1. Frame the subject at 0.3–3 m (LiDAR range; depth coverage ideally > 50 %).
2. Start moving the phone slowly and steadily sideways (parallel to the subject), about 3–6 cm per second; keep the subject in view.
3. While moving, press the shutter once and keep moving for one more second (t+ is collected up to 0.5 s after the shutter).
4. Read the result: CANONICALIZING (raw frozen, no issue) or INCONCLUSIVE with reasons. Do not retry silently: every attempt counts in the statistics.
5. Developer → **Export last raw capture** (writes `Documents/galaxy-fixtures/<uuid>.gxrw`).
6. Note the label in the log sheet below immediately (capture UUID, scene class, distance, lighting, motion, anything unusual).

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
| 13 | low-motion capture | phone held as still as possible while pressing the shutter; also lying still on a desk | INCONCLUSIVE (insufficient motion) |
| 14 | dynamic scene | people walking, moving leaves, a fan | measure |

Controlled substitution of committed depth or RGB observations is done offline on copies of these fixtures by the evidence/geometry tests; the app never produces such captures.

## Log sheet (one row per attempt)

| capture UUID | class # | distance (m) | lighting | motion (as performed) | app result + reasons | notes |
|---|---|---|---|---|---|---|

## Rules

- Captures contain unredacted images, depth, motion and Bluetooth aggregates: shoot only scenes and people who agreed, keep files in `fixtures/real/` only, never publish them.
- Device access from the Mac is serialized with the device lock; keep windows short (the phone is shared).
- Do not delete captures or records on the phone; they are user data (spec §19).
