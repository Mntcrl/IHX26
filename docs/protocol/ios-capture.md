# iOS capture app (apps/capture-ios)

The capture app acquires synchronized ARKit RGB/LiDAR and motion, obtains a real Apple App Attest assertion, publishes a CAPTURE to XRPL Testnet, proves on the phone against the selected future ledger, and publishes a PROOF before releasing the certified PNG or JPEG. It links a single Rust library (`galaxy-ffi` / GalaxyCore); see [the FFI contract](ios-ffi.md). There is no project server.

## Source map

| file | role |
|---|---|
| `XRPL_GalaxyApp.swift` | entry point; DEBUG launch argument `--app-attest-probe` selects the App Attest diagnostic |
| `CaptureScreen.swift` | portrait/landscape camera, shutter, certification entry and state |
| `CaptureProgressScreen.swift` | live certification timeline, XRPL ledger ticker, sampled-tile reveal and result card |
| `DeveloperScreen.swift` | sensor status, selected video format, depth coverage, current baseline, IMU/AR clock summary, "Export last raw capture" |
| `Acquisition.swift` | ARKit session, rolling buffer, t−/t0/t+ selection, quality gates, freeze, state transitions |
| `FrameSnapshot.swift` | copies the needed parts of an `ARFrame` (never retains it); capture issue codes |
| `MotionWindow.swift` | CoreMotion 100 Hz ring buffer on a dedicated serial `OperationQueue` |
| `BluetoothWindow.swift` | foreground BLE scan → committed aggregate (spec §5) |
| `RawCaptureWriter.swift` | GXRW v1 writer + noise aggregate |
| `CaptureRecord.swift`, `CertificationJournal.swift`, `ProtectedFiles.swift` | protected durable records and private evidence policy |
| `CertificationController.swift`, `CertificationScreen.swift` | single wallet operation, setup, recovery, capture history and result |
| `LedgerClient.swift`, `RegistrationFlow.swift` | capped Testnet JSON-RPC in Swift, wallet setup and serial account preparation / DEVICE publication |
| `CertificationPipeline.swift`, `ProverHost.swift` | capture checkpointing, selection ledger, witness/proof and local verification on a worker |
| `GalleryScreen.swift`, `GalleryDetail.swift`, `GalleryLibrary.swift` | capture grid, calibration previews, result detail and export packaging |
| `ExportService.swift` | byte-preserving Files output and add-only PhotoKit resource import |
| `GalaxyAppAttest.swift`, `AppAttestProbe.swift` | DeviceCheck with GalaxyCore-supplied prehashed inputs; separate DEBUG diagnostic |
| `WalletSeedStore.swift` | Keychain storage of the XRPL Ed25519 seed entropy |

Removed from the legacy app: POV committee client/models/views/crypto/depth shares, backend challenge + `XRPLAnchorService`, `AmbientBackendURL`/`POVCoordinatorURL` keys, local-network usage string, SHA-256 Merkle `EvidenceCommitmentBuilder`, ZIP export of raw evidence, the 1,249-line `ContentView`.

## Acquisition

- `ARWorldTrackingConfiguration` with `frameSemantics = .sceneDepth` (unsmoothed), only if `supportsFrameSemantics(.sceneDepth)`; plane detection, scene reconstruction and environment texturing off; `worldAlignment = .gravity`.
- Video format: the back wide-angle 1920×1440 format at 30 fps is selected explicitly (60 fps fallback); all supported formats are logged (`GXCAPTURE supported formats: …`).
- Rolling buffer: at most one frame per 60 ms (≈ 15 fps of the 30 fps stream), 0.6 s history (≤ 10 entries). Each `FrameSnapshot` copies Y and CbCr planes without row padding, Float32 depth and u8 confidence (256×192), intrinsics, transform, timestamp, tracking state/reason and the YCbCr matrix attachment. `ARFrame` objects are never retained.
- Shutter: take t0 from the next delivered AR frame. Pick t− / t+ within 100–500 ms on either side, preferring usable tracking and planes and then larger baseline. The freeze runs at t0 + 580 ms so that the IMU covers t+ + 50 ms.
- CoreMotion: `CMDeviceMotion` at 100 Hz, reference frame `xArbitraryCorrectedZVertical` (bias-corrected rotation rate; the yaw reference is irrelevant because only `rotationRate`, `gravity` and `userAcceleration` are recorded), 256-sample ring buffer. The snapshot covers [t− − 50 ms, t+ + 50 ms] and is rejected if any gap exceeds 25 ms or a value is non-finite. `CMLogItem.timestamp` and `ARFrame.timestamp` share the uptime clock (measured offsets: docs/experiments/device-capture-sensors.md).
  The CoreMotion callback is explicitly `@Sendable`: it runs on the motion queue and touches only the locked buffer, not MainActor. ARKit and CoreBluetooth delegates are configured on the main queue.
- Bluetooth: `CBCentralManager` foreground scan, no connections, `allowDuplicates`. Peripheral identifiers are HMAC-SHA256'd with a random per-session key; only the HMAC lives in memory (≤ 12,000 observations, 6 s), and the key and observations are discarded after each freeze. Aggregate per spec §5 over [max(scan start, t0 − 5 s), freeze]: flags (bit0 powered on, bit1 authorized, bit2 scanned), `scan_duration_ms`, `n_adv`, `n_unique`, 16-bin RSSI histogram of 5 dB from −100 dBm (RSSI clamped to [−100, 0], 127 = unavailable dropped), `rssi_mean_q8`, `rssi_var_q8`, `persistence_pct` (share of identifiers seen in both halves of the window). Bluetooth off/denied → zero aggregate with the corresponding flags; it never blocks a capture.
- Noise: 16 i16 statistics of the center frame's luma (per quadrant: mean |residual|, RMS, signed mean, % |residual| > 8, all ×64) over a sparse 8-pixel grid, residual = Y − round(mean of 4 neighbours). No residual image is formed or stored; no PRNU.

The certification shutter remains disabled until ARKit tracking has been normal for at least one second and the recent depth frames have contained a confident in-policy sample for at least one second. The camera shows which condition is not ready. Tracking can still change during the shutter window: the three selected frames are checked again, and their tracking reasons are recorded locally. This readiness gate is not used for calibration.

### Quality gates (capture issue bits, `quality.flags` in GXRW)

| bit | issue | outcome |
|---|---|---|
| 1 | no frame in a shutter window | INCONCLUSIVE, no raw |
| 2 | tracking not normal in all three frames | raw frozen, INCONCLUSIVE |
| 4 | missing/invalid depth or confidence | INCONCLUSIVE, no raw |
| 8 | not 1920×1440 `420f` | INCONCLUSIVE, no raw |
| 16 | insufficient motion on either side of t0; minimum from the pinned policy (`baselineMinUm`) | INCONCLUSIVE |
| 32 | IMU window unavailable/incomplete | INCONCLUSIVE, no raw |
| 64 | camera/LiDAR unavailable | capture disabled |
| 128 | no confident LiDAR samples in the pinned depth envelope in the center frame | INCONCLUSIVE |
| 256 | relative rotation outside the preflight envelope | INCONCLUSIVE |

The depth and rotation checks are sensor preflight, not a verdict. Only the policy result for the actual Lsel can authorize proving. **Developer → Enable private calibration captures and overlays** uses the separate raw-only flow, with the scene class picker and optional auto-export of GXRW and labels. Neither this mode nor the App Attest and static capture diagnostics can publish a capture.

## GXRW writer

`RawCaptureWriter.write` writes spec §21 GXRW from the frozen frames, IMU and aggregates. The file is streamed into `.capture.gxrw.in-progress` under the attempt UUID using complete file protection, then atomically moved to `capture.gxrw`. Canonicalization and commitments use the local Rust ingestion path; the raster remains sensor-native, even if the UI was held in portrait.

## Capture state machine and storage

States: READY, ACQUIRING, CANONICALIZING, COMMITTING, CHECKING_SCENE, ATTESTING, SUBMITTING_MANIFEST, WAITING_MANIFEST_VALIDATION, WAITING_SELECTION_LEDGER, GENERATING_WITNESS, GENERATING_PROOF, LOCAL_PROOF_VERIFICATION, SUBMITTING_PROOF, WAITING_PROOF_VALIDATION, CERTIFIED, INCONCLUSIVE, FAILED.

Before the shutter, read Testnet's latest validated index and its exact header; persist the header, nonce, wallet/DEVICE/circuit and selected image profile in the ACQUIRING record. Persist the acquisition outcome before publishing the raw file. On recovery, an attempt interrupted at the shutter with no complete raw becomes INCONCLUSIVE; one with complete raw and successful preflight resumes.

The proving actor ingests the frozen GXRW and publishes one protected atomic checkpoint containing the immutable GXEV, core, capture ID, assertion digest and deadline. No App Attest call or transaction uses a core until the checkpoint is written. Recovery reuses this checkpoint; if killed before that write, there is no published core or signed transaction. Only an ingested handle builds a statement; decoded GXEV handles are used to evaluate and export.

Before App Attest or CAPTURE signing, the phone estimates scene suitability using 48 independently randomized local selection-ledger hashes and the committed evidence. At least 36 draws must pass. A lower score stops the attempt without a fee or transaction and shows a hint for the most frequently failing predicate. The journal keeps the counts and elapsed time, not the sampled hashes. A passing scene check is not certification and does not affect how the real Testnet challenge is selected.

The app persists the assertion, the signed CAPTURE before first submission, validated CAPTURE inclusion L0, then the exact Lsel = L0 + 5 header. The selection header and CAPTURE are immutable inputs on recovery. Run reference policy evaluation, witness generation, proving and verification locally. A failed verdict ends the attempt with predicate names and no PROOF. For an eligible attempt, journal the proof/memo, sign one PROOF, and require its validated `tesSUCCESS` in the allowed window. Rust writes the corresponding XMP into the committed image format. Only after packaging and saving that file does the app record CERTIFIED.

Pause, network failure, an RPC history gap or a lock/unlock never authorize a new CAPTURE or challenge. A journaled transaction is queried by hash first and only the same blob may be resubmitted while valid. Validated `tec` or conclusively searched expiry is terminal. Proving may be restarted on the same inputs before signing the PROOF. **Certification & setup** exposes status, resume and pause; its history includes attempts that produced no image.

Testnet HTTP requests use an eight-second per-request budget. Transient transport failures can retry across the three pinned public Testnet servers; the app checks `server_info` network ID before using a fallback and the progress card shows **Waiting for XRPL (retry n)**. If a submit times out, the app looks up the already-journaled transaction hash again before it may resend the same blob. Exhausted transport attempts pause certification without replacing any signed transaction.

## Live certification view

Press the certification shutter to open the progress view. The phase timeline follows the saved attempt; phase times are elapsed between journal checkpoints and can include network waits or pauses. The proof substeps report their actual callback timings, and the constraint count comes from the bundled circuit descriptor.

After the CAPTURE validates, the app reads the validated Testnet index about once a second until the challenge ledger is available. The countdown measures ledgers, not seconds. The reveal highlights the actual sampled tiles on the sensor-native image. The phone stays awake while the attempt is running in the foreground. After the shutter window freezes, ARKit, motion and Bluetooth pause until the attempt ends; the camera then reacquires tracking and depth. **Hide** closes the progress view without pausing certification; reopen it from the progress or last-attempt chip.

The **CERTIFIED → Verify** link opens that photo’s ID, Testnet transaction references and export actions. Export the original and verify it independently with the browser verifier or CLI; a transaction explorer page alone does not verify an image. **INCONCLUSIVE** names the checks that failed and suggests a way to retry; it does not publish a PROOF.

## Gallery and exports

- Tap the photo icon on the camera screen to open **Gallery**. Certification attempts with a saved image are shown newest first, with state and failure reasons. **Calibration captures** lists matching `<uuid>.gxrw` / `<uuid>.label.json` pairs in `Documents/galaxy-fixtures` separately as **Not certified**. Gallery reads these fixtures without changing or sharing either file.
- Tap a photo for the full image. Certified detail shows the device-indicated capture time and L0 close time, L0/Lsel/Lproof, photo/circuit/policy IDs, the attestation environment, and CAPTURE/PROOF IDs with copy buttons and explorer links from the pinned network config. The single-party circuit is labelled **Development setup (Testnet)**.
- New captures default to **JPEG (jpeg-v1)**; PNG remains selectable in **Certification & setup → Capture settings**. For a certified JPEG, Gallery preserves the original JPEG scan data and offers a lossless PNG of the exact reference-decoded raster. Both carry the same certification and informational XMP (spec §4). For a PNG-profile capture, only the certified PNG is available: re-encoding such a raster as JPEG may change its photo ID.
- For each supported format, choose **Share**, **Save to Photos**, or **Save to Files**. GalaxyCore packages certified exports from the stored file; the gallery never exports a preview or JPEG re-encoded by UIKit. Photos receives the exported file as an original photo resource, but sending it from Photos or using a sharing service may transcode it. Use Files to preserve bytes for independent verification. For any uncertified photo, Gallery offers only **Plain JPEG · not certified**, without Galaxy metadata.
- Thumbnails and export staging files use protected, backup-excluded directories under `Library/Caches/GalaxyThumbnails-v1/` and `Library/Caches/GalaxyExports/`. The full certified original remains under `Application Support/GalaxyCaptures/`. There is no Gallery delete action.

## Device data safety (spec §19)

- Records, signed journals, the App Attest identifier, GXRW, GXEV and the certified image live under `Application Support/GalaxySetup/` or `Application Support/GalaxyCaptures/<UUID>/` with complete file protection and backup exclusion. Only this attempt's own checkpoint paths are replaced. Pre-existing raw-only records are not enrolled in certification.
- After a certified image exists, only that attempt’s exact raw/evidence files are removed, unless **Keep private evidence after certification** was selected before the shutter. Retention does not affect the certified image or transaction history. No migration or installation deletes user captures, Keychain items, developer fixtures, or an earlier artifact slot.
- A private fixture export is an explicit Developer action/opt-in, not part of certification. `fixtures/real/` is gitignored. Pull it only with the photographer’s consent and never publish unredacted GXRW. Certified files also contain the user’s photo; share with consent. No seeds, key IDs, salts, witness values, raw sensor data or RPC bodies are logged.

## Testnet wallet and device setup

Create one wallet from 16 SecRandom bytes; GalaxyCore derives its Ed25519 identity. The entropy is stored once with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, never copied to the journal or the network. The official Testnet faucet is an explicit action sending only the public address; its response body is not read. Validate the actual balance against base and owner reserves and a fee budget before preparing the account or starting registration.

Registration starts with a memo-free AccountSet `SetFlag = 5` (`asfAccountTxnID`). Validated preparation creates the all-zero predecessor sentinel; the first DEVICE chunk uses it. Each following transaction uses the previous `tesSUCCESS` own transaction hash. The app journals that head and checks it against validated `account_info` before signing. A validated `tec` does not advance it; incoming XRP top-ups do not change it. Do not send unrelated outgoing transactions from this delegated Testnet wallet.

Generate an App Attest key, read the exact validated registration ledger, and journal the key and nonce before calling Apple with GalaxyCore’s prehashed digest. Publish the resulting receipt-stripped bundle chunks serially, then the DEVICE manifest, all within the original registration window. Save its tx hash as the registration reference only after validated success. An interrupted attempt never changes its challenge and never replaces a signed tx with an unresolved outcome. A definitively expired or failed attempt may start a fresh registration; account preparation is not repeated if it previously validated.

## App Attest client

The default `capture-off` build omits the App Attest entitlement and uses the observed development environment (ADR 0070). Explicit `capture-development` or `capture-production` builds use the corresponding entitlement and matching Info.plist value and require a compatible profile. The build’s app ID is checked against the Rust-pinned Testnet list; it is not inferred from an error or the RPC. A single-party proving setup is shown as **Development setup (Testnet)**; certification is not a production trust claim.

## Offline proving self-test (Debug only)

Open **Developer → Proving self-test → Run proving self-test**. The app pauses the capture sensors and reads one existing genuine calibration GXRW without modifying it. The test runs on the phone without network requests, signing, or transaction publication; it cannot certify a photo. It records only wall-clock timings, memory footprint, thread count and thermal state in `Documents/galaxy-proving-diagnostics/<UUID>.log`, with complete file protection and backup exclusion.

## Morning checklist (Mac and physical iPhone)

1. Ensure a logged-in Mac Aqua session, valid profile and ≥ 3 GiB available in the build mirror. Respect [spec §19](galaxy-v1-spec.md#19-mac-build-host-and-device): no access to the user's original workspace; no uninstall, blanket device cleanup or private fixture pulls. Get orchestrator approval for the device window and keep ProverBench out of it.
2. Build GalaxyCore on Linux with the current accepted Testnet config and final descriptor. The generated archive, Swift, header and module map must come from the same build, including the `AccountTxnID` surface. `scripts/ios.sh` checks `BUILD-INFO.txt` before any Mac sync. The default artifact paths below are used by the Xcode resource and installer:

   ```sh
   CARGO_TARGET_DIR="$PWD/target/GalaxyCore" scripts/ios-rust-lib.sh
   scripts/ios.sh sync-build
   apps/capture-ios/tools/mac-gui-build.sh capture-compile   # compile/link only
   apps/capture-ios/tools/mac-gui-build.sh capture-off       # default signed build (no App Attest entitlement)
   ```

3. With the phone unlocked, use the device lock via the commands below. The first `run` installs in place and launches; open Certification & setup so the app creates the namespaced artifact directories. It must show “proving files are missing” and leave the certification shutter disabled. `push-artifacts` stages a matching pair into a new immutable per-circuit slot and writes `READY` last. Relaunch to open and digest-check both mapped files in GalaxyCore. No graph/zkey goes into DerivedData or the signed app.

   ```sh
   scripts/ios.sh run
   scripts/ios.sh push-artifacts
   scripts/ios.sh relaunch
   ```

4. On the first Swift 6/Xcode build, check the generated C module resolves `galaxy_ffiFFI` and that only `libgalaxy_ffi.a` links. Inspect the expanded Info.plist `GalaxyAppID`/`GalaxyAppAttestEnvironment`, effective signing entitlements and add-only Photos usage description. Pay particular attention to SDK-imported concurrency declarations for `DCAppAttestService`, `URLSessionTaskDelegate`, `URLSession.AsyncBytes`, PhotoKit’s asynchronous change block, SwiftUI’s file exporter and the ARKit delegate. Check `devicectl` creates/copies the pair directory and `READY` at the documented paths. These are device/Xcode validation points, not claims of a Linux Swift build.
5. Create a Testnet wallet. Confirm registration is disabled before validated funds, request faucet XRP, refresh, then start registration. Observe account preparation, all chunks and DEVICE validate. Check the prep tx is memo-free SetFlag 5, first chunk carries the ledger zero sentinel, and subsequent chunks/DEVICE link their validated heads. Record per-tx size and fee, count (preparation + chunks + manifest), and App Attest environment. Top-ups need no authorization step.
6. Test a textured three-dimensional scene with edges and multiple depths and ordinary hand motion, not a supported phone. Follow the [physical capture protocol](../experiments/device-capture-protocol.md) for diagnostic scenes only. On a real certifying capture record the two tx hashes, Lpre/L0/Lsel/Lproof, witness/prove durations, app size, peak device memory and end-to-end time. From Gallery, verify a calibration preview and save a plain JPEG to Files. For a certified JPEG capture, save both JPEG and PNG through Files, and use **Export Unmodified Original** for the equivalent Photos check. On Linux, with the photographer’s consent and private copies under `fixtures/real/`, verify each format independently:

   ```sh
   CARGO_TARGET_DIR="$PWD/target/GalaxyCLI" scripts/with-slot.sh cargo run --locked -p galaxy-cli -- \
     --network-config deployments/networks/xrpl-testnet.toml verify /path/to/private/Galaxy-image.png
   ```

   Record the independent verifier’s actual state and reasons. Photos display/share operations may transcode; Files is the reference byte-preserving path. Check that unknown or mismatched artifacts do not enable capture, and an inconclusive scene produces no PROOF.
7. Exercise pause/kill and relaunch at the signed-but-not-submitted CAPTURE, validated CAPTURE, prior to Lsel, proving/cancellation, and after signing/submitting the PROOF. Check unchanged hashes, exact Lsel and no duplicate tx or new nonce. Check lock/unlock and backup exclusion. For replay failures, verify that ambiguous RPC history pauses rather than expiring; do not delete signed journals to reset an attempt.

Use the existing diagnostic commands only in an approved device window:
```
scripts/ios.sh build                      # sync + signed Aqua build
scripts/ios.sh install                    # build + in-place devicectl install under lock
scripts/ios.sh run [app args…]            # build + install + launch under lock
scripts/ios.sh relaunch                   # relaunch the installed app without rebuild or reinstall
scripts/ios.sh logs [app args…]           # launch with --console (device lock), prints app stdout
scripts/ios.sh pull-fixtures raw          # Documents/galaxy-fixtures → fixtures/real/capture-ios/
scripts/ios.sh pull-fixtures appattest    # Documents/appattest → fixtures/real/appattest/
```

`APP_ATTEST=development|production` requires the corresponding provisioning capability. DEBUG launch arguments: `--app-attest-probe` (isolated App Attest diagnostic), `--static-capture-probe` (raw-only static acquisition and local auto-export), `--diagnostic-exit` (exit on diagnostic completion).
