# ADR 0022 — BLUETOOTH / NOISE / META component layouts

Status: accepted (ImageEvidence)

## Context
Spec §6 fixes the CAMERA (54) and IMU (133) element layouts and leaves BLUETOOTH, NOISE and
META to this workstream. These components are committed only (hashed by the circuit, never
evaluated by the v1m policy), so the layouts must be fixed-size, bounded, privacy-preserving
and stable across app versions.

## Decision
- Fixed element counts: BLUETOOTH 24, NOISE 18, META 9 (tables in
  `docs/protocol/evidence-v1.md`). Element 0 of each component is a layout version; negative
  values enter the field as `x mod r` (`fr_from_i128`).
- BLUETOOTH v1: flags (poweredOn / authorized / scanned), scan duration, advertisement count,
  unique count (deduplicated with a per-session HMAC key that is discarded), a 16-bin RSSI
  histogram (5 dB bins from −100 dBm), mean and variance in Q8, persistence percentage.
  Version 0 = no aggregate (all zero). Denied or unavailable Bluetooth is version 1 with
  cleared flags and zero counts and never invalidates a proof. No identifier or address of a
  peripheral is part of the aggregate.
- NOISE v1: up to 16 i16 residual statistics chosen by the capture app, zero-filled; never
  PRNU or another persistent sensor fingerprint (spec §19).
- META v1: image profile code (must equal the canonical image profile), app build, device
  model code (append-only table), frame offsets in ms relative to frame 1, YCbCr matrix code,
  informational quality flags.
- `Evidence::validate` enforces every bound and cross-field rule (e.g. `Σ histogram = n_adv`,
  `n_unique ≤ n_adv`, version-0 all-zero) before commitment; the GXEV codec stores the same
  fields with fixed widths.

## Consequences
- The components can be opened later (selective disclosure) without layout ambiguity.
- New fields require a new layout version; existing codes and device model codes never change.
- The aggregates carry no per-device or per-peripheral identifier.
