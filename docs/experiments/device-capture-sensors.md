# Device experiment — capture sensors and first GXRW files

Date: 2026-11-20 20:44–20:45 CET. iPhone 13 Pro Max (`iPhone14,3`), iOS 27.2.1, capture app Debug build (ADR 0071), launched with `scripts/ios.sh logs --static-capture-probe --diagnostic-exit`. The phone was being handled by a person during both runs (not lying still), as the IMU data shows; the "static desk" capture requested for the insufficient-motion path has not been recorded yet.

## Video formats (`ARWorldTrackingConfiguration.supportedVideoFormats`)

All formats reported use `AVCaptureDeviceTypeBuiltInWideAngleCamera`: 1920×1440 @ 60/30 fps, 1920×1080 @ 60/30 fps, 1280×720 @ 60/30 fps (the list is reported twice by ARKit). Selected: 1920×1440 @ 30 fps. `capturedImage` was `420f` (full-range bi-planar) 1920×1440 with YCbCr matrix attachment `ITU_R_601_4`; `sceneDepth` 256×192 Float32 + confidence u8 was present in every selected frame.

## Clocks

| run | nearest IMU sample − ARFrame (t0) | AR delegate lag | IMU handler lag |
|---|---|---|---|
| #1 | −3.07 ms | 65.59 ms | 0.53 ms |
| #2 | −4.95 ms | 64.01 ms | 0.73 ms |

Lag = `CACurrentMediaTime()` at callback − sensor timestamp. With a 10 ms IMU period the nearest-sample offset is bounded by ±5 ms if both timestamps use the same clock; the measured offsets are within that bound, and both timestamps are directly comparable to `CACurrentMediaTime()` (uptime clock). ARKit delivers frames ≈ 65 ms after their capture timestamp, which the shutter logic accounts for by taking t0 from the buffer, not from the tap time. IMU sample spacing in run #1: 10.07–11.33 ms.

## Captures (private, `fixtures/real/capture-ios/`, gitignored)

| file | result | tracking (−,0,+) | frame offsets | baselines |T₀−T₋| / |T₀−T₊| | peak |userAcceleration| | Bluetooth |
|---|---|---|---|---|---|---|
| 15473E68-64B8-4A9B-895D-91B34C09DDCF.gxrw | INCONCLUSIVE (flags 2: t− limited/excessive_motion) | 1, 2, 2 | −267 / 0 / +267 ms | 91 / 228 mm | 595 mg | scanned, 167 adv, 34 unique |
| FB010EC4-2A89-4A7A-B5C0-892C2261784B.gxrw | CANONICALIZING (no issue) | 2, 2, 2 | −267 / 0 / +267 ms | 150 / 220 mm | 378 mg | scanned, 128 adv, 17 unique |

File sizes 13,194,535 and 13,194,191 bytes (headers 15,646 and 15,302 B, payload 13,178,880 B each). Both pass `galaxy_evidence::ingest::from_gxrw(…, PngV1)` (checked on Linux with a scratch binary against the workspace crate). Center-frame depth in #1: 0.215–0.704 m, median 0.405 m; confidence counts low/medium/high = 15,970 / 5,278 / 27,904. IMU spacing in #2: 10.07 ms.

## Observations

- The first capture of run #1 fired 2 s after the session start, while tracking had just become normal; t− was still `excessive_motion`. The probe now waits until the whole 0.6 s history tracks normally (≥ 3 s after start), which gave an all-normal capture in run #2.
- A hand-held capture moved 90–230 mm within ±267 ms here, far above the 3 mm minimum baseline; the "move slightly sideways" guidance should target a few centimetres.
