# Incident 2026-11-20 — deletion of the capture app's Documents on the test iPhone

## Summary
At ≈ 20:40:40 CET a cleanup step of the on-device proving benchmark (`apps/prover-bench-ios`, run with `--cleanup`) deleted every entry of `Documents/` in the app data container of `utcq.XRPL-Galaxy` on the iPhone 13 Pro Max (CoreDevice E34AE83F-D43C-510A-94EA-8E032BCD853E): console output `GXBENCH CLEANUP removed=163 failures=0`.

## Timeline (CET)
- ≈ 14:05–14:12 — 7-day profile created; original capture app (bundle id utcq.XRPL-Galaxy) built and installed; its container is created (first `Library/Saved Application State` entry 14:12).
- 14:12–20:35 — the original app is used; it stores scans as `Documents/scan-<UUID>.*` (≈ 16 files per scan; JPEG also saved to the Photos library).
- ≈ 20:36 — the benchmark app is installed under the same bundle id (approved by the orchestrator because no Xcode account is configured on the Mac, so no new bundle id could be provisioned). A same-bundle-id install keeps the existing data container.
- 20:37–20:38 — benchmark fixtures copied into `Documents/fixture`, `Documents/bench_500k`; runs; `Documents/results` copied off.
- ≈ 20:40:40 — `devicectl device process launch … utcq.XRPL-Galaxy --cleanup`: the cleanup enumerated `Documents/` and removed every entry (163), not only its 3 directories.
- 20:41 — the benchmark agent reports the deletion; the orchestrator freezes all device access.
- 20:42 — read-only listing: `Documents/` empty; `Library/` intact. No local Finder backups on the Mac (`~/Library/Application Support/MobileSync/Backup` empty); no `scan-*` copies on the Mac or the Linux workstation.

## Impact
Lost: the evidence files of the captures made today with the original app (exact list unknown: names were not logged and the directory had not been listed beforehand; estimate ≈ 10 scans). Not affected: JPEGs in the Photos library, `Library/` (preferences, caches), anything outside the device.

## Root cause
1. The benchmark cleanup deleted by enumerating the parent directory instead of an allowlist of its own paths.
2. The benchmark reused another app's bundle id, and therefore its data container, without a protected-content check.

## Corrective actions
- Benchmark cleanup now deletes only its own namespaced directory and logs removed paths.
- Spec §19 "Device data safety" (mandatory): agents never delete device files they did not create; namespaced directories; allowlist cleanup by exact path; list + copy-off before destructive steps; no "delete everything" code paths; container sharing needs explicit approval; uninstalling the capture app requires the user's approval.
- Device access serialized per agent with explicit orchestrator approval.

## Recovery options (user decision)
Only an iCloud backup of the phone taken after the captures (iCloud keeps only the latest backup) or ZIP exports previously shared from the old app could contain the lost files.
