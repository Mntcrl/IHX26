# iOS capture app (apps/capture-ios)

Phase 1 scope: sensors → frozen GXRW raw capture (spec §21) with a persistent capture record, App Attest client calls, Keychain wallet seed storage and developer diagnostics. Canonicalization, commitments, proving and XRPL submission run in the Rust core (Phase 2, `crates/galaxy-ffi`).

## Source map

| file | role |
|---|---|
| `XRPL_GalaxyApp.swift` | entry point; DEBUG launch argument `--app-attest-probe` selects the App Attest diagnostic |
| `CaptureScreen.swift` | landscape-right capture UI (camera-native orientation, spec §4), guidance text, state + reasons |
| `DeveloperScreen.swift` | sensor status, selected video format, depth coverage, current baseline, IMU/AR clock summary, "Export last raw capture" |
| `Acquisition.swift` | ARKit session, rolling buffer, t−/t0/t+ selection, quality gates, freeze, state transitions |
| `FrameSnapshot.swift` | copies the needed parts of an `ARFrame` (never retains it); capture issue codes |
| `MotionWindow.swift` | CoreMotion 100 Hz ring buffer on a dedicated serial `OperationQueue` |
| `BluetoothWindow.swift` | foreground BLE scan → committed aggregate (spec §5) |
| `RawCaptureWriter.swift` | GXRW v1 writer + noise aggregate |
| `CaptureRecord.swift` | 16-state machine, persistent record store, recovery, fixture export |
| `ProtectedFiles.swift` | the only file writer (complete protection, excluded from backup, never overwrites foreign files) |
| `GalaxyAppAttest.swift` | spec §3/§11 client data and `DCAppAttestService` calls |
| `AppAttestProbe.swift` | DEBUG device diagnostic for App Attest (ADR 0070) |
| `WalletSeedStore.swift` | Keychain storage of the XRPL Ed25519 seed entropy |

Removed from the legacy app: POV committee client/models/views/crypto/depth shares, backend challenge + `XRPLAnchorService`, `AmbientBackendURL`/`POVCoordinatorURL` keys, local-network usage string, SHA-256 Merkle `EvidenceCommitmentBuilder`, ZIP export of raw evidence, the 1,249-line `ContentView`.

## Acquisition

- `ARWorldTrackingConfiguration` with `frameSemantics = .sceneDepth` (unsmoothed), only if `supportsFrameSemantics(.sceneDepth)`; plane detection, scene reconstruction and environment texturing off; `worldAlignment = .gravity`.
- Video format: the back wide-angle 1920×1440 format at 30 fps is selected explicitly (60 fps fallback); all supported formats are logged (`GXCAPTURE supported formats: …`).
- Rolling buffer: at most one frame per 60 ms (≈ 15 fps of the 30 fps stream), 0.6 s history (≤ 10 entries). Each `FrameSnapshot` copies Y and CbCr planes without row padding, Float32 depth and u8 confidence (256×192), intrinsics, transform, timestamp, tracking state/reason and the YCbCr matrix attachment. `ARFrame` objects are never retained.
- Shutter: t0 = newest buffered frame (≤ 120 ms old). t− = buffered frame nearest to t0 − 250 ms within [t0 − 500, t0 − 100] ms; t+ = nearest to t0 + 250 ms within [t0 + 100, t0 + 500] ms, collected after the shutter. The freeze runs at t0 + 580 ms so that the IMU covers t+ + 50 ms.
- CoreMotion: `CMDeviceMotion` at 100 Hz, reference frame `xArbitraryCorrectedZVertical` (bias-corrected rotation rate; the yaw reference is irrelevant because only `rotationRate`, `gravity` and `userAcceleration` are recorded), 256-sample ring buffer. The snapshot covers [t− − 50 ms, t+ + 50 ms] and is rejected if any gap exceeds 25 ms or a value is non-finite. `CMLogItem.timestamp` and `ARFrame.timestamp` share the uptime clock (measured offsets: docs/experiments/device-capture-sensors.md).
- Bluetooth: `CBCentralManager` foreground scan, no connections, `allowDuplicates`. Peripheral identifiers are HMAC-SHA256'd with a random per-session key; only the HMAC lives in memory (≤ 12,000 observations, 6 s), and the key and observations are discarded after each freeze. Aggregate per spec §5 over [max(scan start, t0 − 5 s), freeze]: flags (bit0 powered on, bit1 authorized, bit2 scanned), `scan_duration_ms`, `n_adv`, `n_unique`, 16-bin RSSI histogram of 5 dB from −100 dBm (RSSI clamped to [−100, 0], 127 = unavailable dropped), `rssi_mean_q8`, `rssi_var_q8`, `persistence_pct` (share of identifiers seen in both halves of the window). Bluetooth off/denied → zero aggregate with the corresponding flags; it never blocks a capture.
- Noise: 16 i16 statistics of the center frame's luma (per quadrant: mean |residual|, RMS, signed mean, % |residual| > 8, all ×64) over a sparse 8-pixel grid, residual = Y − round(mean of 4 neighbours). No residual image is formed or stored; no PRNU.

### Quality gates (capture issue bits, `quality.flags` in GXRW)

| bit | issue | outcome |
|---|---|---|
| 1 | no frame in a shutter window | INCONCLUSIVE, no raw |
| 2 | tracking not normal in all three frames | raw frozen, INCONCLUSIVE |
| 4 | missing/invalid depth or confidence | INCONCLUSIVE, no raw |
| 8 | not 1920×1440 `420f` | INCONCLUSIVE, no raw |
| 16 | insufficient motion: min(|T₀ − T₋|, |T₀ − T₊|) < 3 mm | raw frozen, INCONCLUSIVE |
| 32 | IMU window unavailable/incomplete | INCONCLUSIVE, no raw |
| 64 | camera/LiDAR unavailable | capture disabled |

Raw captures with issues 2/16 are kept for diagnostics and calibration only; they never proceed to CANONICALIZING.

## GXRW writer

`RawCaptureWriter.write` produces exactly spec §21: `"GXRW" ‖ 0x01 ‖ u32be(header_len) ‖ header JSON (JSONEncoder, ≤ 1 MiB) ‖ payload`. Payload order: for minus, center, plus: Y (1920×1440), CbCr (1920×720), depth (256·192 Float32 LE), conf (256·192 u8); regions are contiguous, non-overlapping and cover the payload. `intrinsics = [m[0][0], m[1][1], m[2][0], m[2][1]]`, `transform` column-major, `device.app_build` = CFBundleVersion (decimal), `claimed_time_ms` = wall clock at the shutter. The file is written to `<capture dir>/.capture.gxrw.in-progress` (new file, complete protection) and atomically moved to `capture.gxrw`; recovery therefore never sees a partial capture. Both device captures of 2026-11-20 are accepted by `galaxy_evidence::ingest::from_gxrw`.

## Capture state machine and storage

States: READY, ACQUIRING, CANONICALIZING, COMMITTING, ATTESTING, SUBMITTING_MANIFEST, WAITING_MANIFEST_VALIDATION, WAITING_SELECTION_LEDGER, GENERATING_WITNESS, GENERATING_PROOF, LOCAL_PROOF_VERIFICATION, SUBMITTING_PROOF, WAITING_PROOF_VALIDATION, CERTIFIED, INCONCLUSIVE, FAILED.

Transitions (`CaptureState.permits`): the forward chain READY → ACQUIRING → … → WAITING_PROOF_VALIDATION → CERTIFIED; any non-terminal state → INCONCLUSIVE or FAILED with at least one reason; CERTIFIED/INCONCLUSIVE/FAILED are terminal. Phase 1 drives READY → ACQUIRING → CANONICALIZING | INCONCLUSIVE | FAILED.

Storage: `Application Support/GalaxyCaptures/<capture UUID>/record.json` (JSON `{id, claimedTimeMs, state, reasons}`) and `capture.gxrw`, complete file protection, excluded from backup. The acquisition decision (reasons) is persisted before the raw file is published. Recovery at launch: a record left in ACQUIRING becomes CANONICALIZING (raw present, no reasons), INCONCLUSIVE with its saved reasons (raw present), or INCONCLUSIVE "Acquisition was interrupted before the raw capture was frozen." (no raw). Logs never contain sensor values, residuals or witness data; only states, reasons, format names and timing summaries.

## Device data safety (spec §19)

- The app writes only under `Application Support/GalaxyCaptures/<new UUID>/`, `Documents/galaxy-fixtures/<capture UUID>.gxrw` (explicit export) and `Documents/appattest/<new UUID>/` (DEBUG probe). Existing directories are used as they are; existing files are never overwritten (`ProtectedFiles.create` uses `.withoutOverwriting`; `replace` is used only for `record.json` inside the capture's own UUID directory).
- The only deletion is the writer's own in-progress file after a failed write. There is no code that enumerates and deletes Documents, captures, records or the wallet.

## Wallet seed storage

`WalletSeedStore.store(_:)` adds the 16-byte XRPL Ed25519 family-seed entropy (generated by the Rust core) as a generic-password item (service `utcq.XRPL-Galaxy.wallet`, account `xrpl-testnet-ed25519`, `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, not synchronizable). A second `store` fails (`errSecDuplicateItem`): a wallet is never rotated implicitly. `load()` returns the entropy or nil. No deletion API exists (the wallet is user data).

## App Attest client

`GalaxyAppAttest` builds the spec §3 preimages (`registrationClientData`, `assertionClientData` for the 299-byte capture core) and calls `generateKey`, `attestKey(SHA256(clientData))`, `generateAssertion(SHA256(clientData))`. The default build has no App Attest entitlement and uses the development environment (ADR 0070).

## Build, run, diagnostics

```
scripts/ios.sh build                      # rsync + signed build in the Mac Aqua session (ADR 0071)
scripts/ios.sh install                    # build + devicectl install (device lock)
scripts/ios.sh run [app args…]            # build + install + launch
scripts/ios.sh logs [app args…]           # launch with --console (device lock), prints app stdout
scripts/ios.sh pull-fixtures raw          # Documents/galaxy-fixtures → fixtures/real/capture-ios/
scripts/ios.sh pull-fixtures appattest    # Documents/appattest → fixtures/real/appattest/
```

`APP_ATTEST=development|production` builds with `AppAttest.entitlements` (needs a team profile with the capability). DEBUG launch arguments: `--app-attest-probe` (ADR 0070 experiment), `--static-capture-probe` (automatic capture once tracking has been normal for the whole 0.6 s history and ≥ 3 s after start, or after 15 s; exports the raw file and prints `GXCAPTURE` lines), `--diagnostic-exit` (exit after the diagnostic). `fixtures/real/` is gitignored; GXRW files contain unredacted camera, depth and motion data and are pulled only with the photographer's consent.
