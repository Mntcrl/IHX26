# XRPL Galaxy passive ambient evidence

## Trust model

A scan is a private collection of sensor observations. An inclusion proof checks disclosed bytes against a root obtained independently. App Attest authenticates an app installation and the assertion bound to the root. The backend verifies one-time challenge consumption and an increasing assertion counter. A validated XRPL transaction timestamps the existence of the public commitment on that network. These are separate results. The root and assertion do not establish the physical truth of ARKit, camera or CoreBluetooth values.

The app works with Bluetooth unavailable, an empty scan window, no infrastructure anchors, or no backend. Lack of observations does not imply nobody was nearby. An observed identifier is a temporary CoreBluetooth handle, not a person, a verified device, or a stable hardware identity. RSSI bins are descriptive and have no distance interpretation.

## Data flow and ownership

1. `CaptureSessionManager` starts foreground scanning while the camera interface is active. It samples ARKit poses at roughly 8 Hz. A configured backend supplies short-lived challenges in advance, refreshing them while the camera is open.
2. `AmbientRadioScanner` requests all services with duplicate callbacks enabled. It reads only the peripheral identifier, callback uptime, RSSI and advertised transmit power. It never connects. The identifier is immediately transformed with HMAC-SHA256 under a fresh 256-bit key. The rolling buffer is time bounded and capped at 12,000 entries. A foreground gap or dropped samples reduces completeness. The key and temporaries are discarded after aggregation, and a new collection starts.
3. The shutter freezes one ARFrame. On the observed iPhone, ARKit frame time and Core Animation callback uptime were aligned. The app checks their proximity and labels a shutter-time fallback if needed. It waits out the remaining post window, then `RadioFingerprintBuilder` emits only histograms, counts, one-second callback bins and coarse statistics. Raw timestamps, raw RSSI samples, temporary handles and advertised values do not enter the archive or network requests. By default the capture uses 5 seconds of rolling history and 0.35 seconds after the shutter. Native frame processing, JPEG encoding, and shutter LiDAR preparation run concurrently with this short post-shutter window.
4. `SpatialEvidenceCollector` records relative timestamps, world camera positions, XYZW quaternions, depth rays, world hit positions, depth confidence and local surface normals. Normals use neighboring samples only when they are on an approximately continuous depth surface. It also compares the observed depth to the inferred axial depth of reprojected ARKit features and checks pose continuity. The resulting status is a heuristic with an insufficient-data state. The existing scan holds a JPEG, native YUV where supported, depth, camera calibration, confidence, point cloud, planes and available ARKit mesh anchors. ARKit may yield zero mesh anchors while valid single-frame depth remains available.
5. `TrustedAnchorProvider` is separate from the ambient Bluetooth scanner. The default provider yields nothing. A deployer can inject an infrastructure-specific source and pinned Ed25519 keys. The app verifies signatures and bounded freshness and deduplicates tokens within the capture. A captured signed broadcast can still be replayed or relayed during its valid window. No secure ranging provider is implemented, so the UI reports it as unavailable.
6. `InertialEvidenceCollector` records the device-axis accelerometer (g, including gravity) and gyro (rad/s) through Core Motion during the same foreground window. The requested rate is 100 Hz per stream. It checks that sensor uptime is close to the shared host uptime on delivery, stores sensor time relative to the capture center, and reports measured rates, gaps and availability independently. No integration to a world pose or gravity subtraction is applied.
7. `CameraNoiseAnalysis` works on the unrotated pre-JPEG ARKit frame. It records a signed full-resolution high-pass residual for every luma and chroma sample, with per-channel row and column profiles, extrema and RMS. Up to one in-memory frame on each side of capture is evaluated. Comparisons run only when format, pose, tracking, intrinsics and exposure checks pass. They export 64×64 tile statistics. A separate physics bundle preserves the neighboring luminance planes and their depth maps, when available, without requiring the noise gates. Residuals are indexed in the delivered YUV grid, which supports per-capture comparison when formats match. ARKit exposes processed YUV, so scene detail, filtering and stabilization can affect residuals. Raw Bayer data, hidden calibration, and a complete silicon noise or defect model are outside this capture interface.
8. `EvidenceCommitmentBuilder` creates salted leaves for the photo, LiDAR, radio, metadata and, if present, verified infrastructure tokens. The JPEG is published to the local library after every committed attachment and the proof have been saved. Any save failure removes partially written files.
9. `CaptureAttestationService` enrolls the device-generated App Attest key with the configured backend. It submits a fresh assertion over the root and its session-scoped challenge. The key identifier stays off-chain and out of AirDrop exports. If a fresh challenge was unavailable when the shutter fired, the saved scan remains local and unverified.
10. The backend checks the Apple certificate chain, nonce, app identity and environment on enrollment. Submission verifies the assertion signature, root binding, app identity and counter. A SQLite transaction consumes the unexpired challenge, advances the counter and enqueues the root together. The worker uses a dedicated Testnet account when configured. `XRPLAnchorService` fetches backend verification status. The worker marks an anchor verified only after a successful transaction appears in a validated ledger with the expected signer and memo.
11. `EvidenceVerifier` checks digests and Merkle paths. The scan view displays integrity, App Attest, replay protection, XRPL anchoring, observation presence, spatial consistency, infrastructure and secure ranging separately. The local verifier does not accept a receipt as proof of the sensor claims.

## On-device file schema

The standard capture uses a shared `scan-<UUID>` stem. Existing scans remain readable.

| Extension | Contents | Leaf |
|---|---|---|
| `.jpg`, `.yuv` | Oriented JPEG and original ARKit YUV planes where available | photo |
| `.noise.json`, `.noise.i16` | Frame context, residual statistics, and headerless signed Int16LE Y, Cb, Cr residual planes at native plane resolutions | photo |
| `.physics.json`, `.ref0.y`, `.ref1.y` and optional reference `.fdepth` files | Up to two neighboring frame luminance planes with native intrinsics, capture offsets, and available depth. The offline check also uses the shutter frame and `.imu.json`. | photo |
| `.json`, `.fdepth`, `.spatial.json` | Camera calibration, EXIF, mesh, depth confidence, native depth and trajectory with vectors | lidar |
| `.imu.json` | Sensor uptime offsets and three-axis Core Motion samples, with per-stream coverage | lidar |
| `.radio.json` | Quantized ambient summary | radio |
| `.session.json` | AmbientCaptureEvidence, challenge where obtained, claimed Unix milliseconds, window milliseconds and availability | metadata |
| `.anchors.json` | Optional verified signed infrastructure tokens | trustedAnchors |
| `.proof.json` | Root, salts, per-file digests and inclusion paths for a full local export | outside the tree |

In the residual file the planes are packed sequentially with byte offsets and row widths in the JSON. The kernel is `4 × center − left − right − up − down`, with edge coordinates clamped. For 10-bit input, the six packing bits are excluded from computation and retained in the original YUV artifact. Each temporal frame must be within 4 mm, 0.2 degrees, 1 pixel of intrinsics drift, and 3% of the shutter exposure duration. Scene motion may still affect tile statistics. Mac verification: `python3 tests/check-camera-evidence.py /path/to/<scan>.noise.json`.

The LiDAR category discloses the scan JSON, including its camera metadata. It does not disclose the JPEG, native camera frame, radio or other categories. The radio histogram has ten bins of 10 dB starting at −110 dBm, with underflow and overflow folded into the edge bins. Median and standard deviation use 5 dB buckets. Persistence bins are single callback, span under one second, span under three seconds, and span of at least three seconds. A scanning window with zero callbacks is distinct from unavailable Bluetooth.

## Commitment protocol v1

File digests are SHA-256 over the exact stored bytes. The ordered tuple digest is SHA-256 over UTF-8 `xrpl-galaxy/files/v1`, a zero byte, then each ASCII filename in ascending order followed by a zero byte and its 32 raw digest bytes. The files themselves are committed byte for byte. Typed summaries use sorted-key compact JSON and the full proof carries filenames and hexadecimal digests.

A leaf is SHA-256 over byte `00`, the UTF-8 category, byte `00`, 32 random salt bytes and the 32-byte tuple digest. Internal nodes use SHA-256 over byte `01`, then the left and right 32-byte hashes. Category order is photo, lidar, radio, metadata, trustedAnchors when present. A level with an odd node count duplicates its last node. A selective archive contains only the chosen category's files, salt and sibling hashes with side flags. Keep the full archive private if you intend to disclose only one category.

The assertion client payload is these UTF-8 fields joined by zero bytes: `xrpl-galaxy/assertion/v1`, canonical lowercase session UUID, challenge UUID, server nonce string, and lowercase root hex. App Attest receives SHA-256 of that payload. Enrollment hashes the server nonce string exactly as UTF-8. The server submits an `AccountSet` without account setting changes, with one memo whose `MemoData` is byte `01` plus the 32-byte root. XRPL transaction envelope fields such as the signing account, fee and time are naturally public. No capture data or attestation ID is in the memo.

## Public iOS API feasibility

The local iOS 26.5 SDK includes CoreBluetooth, Network, MultipeerConnectivity, WiFiAware, NearbyInteraction, CoreLocation, UIKit and DeviceCheck. Framework presence alone does not grant access to system discovery. No special entitlement or private wireless API is used for ambient scanning.

| Mechanism | What the API exposes | Use here |
|---|---|---|
| CoreBluetooth | Foreground callbacks from advertising peripherals visible to iOS. Nil service list and duplicate delivery are supported, with OS and RF filtering. | Passive ambient aggregates. [Scan options](https://developer.apple.com/documentation/corebluetooth/cbcentralmanagerscanoptionallowduplicateskey) |
| Apple peer-to-peer Wi-Fi and Multipeer Connectivity | Discovery of participating advertised services, including an app's matching service. | No arbitrary device inventory. [Apple peer-to-peer overview](https://developer.apple.com/documentation/technotes/tn3111-ios-wifi-api-overview) |
| AirDrop | A system share activity; public API does not enumerate its candidate devices or return their discovery data. | User-initiated ZIP sharing only. [UIActivity](https://developer.apple.com/documentation/uikit/uiactivity) |
| Wi-Fi Aware | Supported hardware includes iPhone 13 Pro Max. Public app flows require declared services, entitlement, and authorized pairing. | Excluded from ambient observation. [Wi-Fi Aware](https://developer.apple.com/documentation/WiFiAware) |
| Network framework | Browses published services such as Bonjour; peer-to-peer support changes transport, not discovery scope. | No ambient Wi-Fi station enumeration. [Network migration guidance](https://developer.apple.com/documentation/technotes/tn3213-moving-from-multipeer-connectivity-to-network-framework) |
| NearbyInteraction | Peer configuration and token exchange, or a participating compatible accessory. | No passive UWB measurements. [Initiating a session](https://developer.apple.com/documentation/NearbyInteraction/initiating-and-maintaining-a-session) |
| CoreLocation beacon detection | Known iBeacon UUID, optionally narrower major/minor, with location authorization. | Potential infrastructure use only. [Beacon constraints](https://developer.apple.com/documentation/corelocation/clbeaconidentityconstraint) |

Find My, Continuity and AirDrop internal discovery data are outside the public API surface used here.

## Development and deployment

From `XRPL_Galaxy`, `make run` builds, installs and launches on the configured iPhone. This is a complete offline capture flow. To adjust the pre-shutter history and post-shutter wait, use `make run AMBIENT_HALF_WINDOW_SECONDS=3 AMBIENT_POST_WINDOW_SECONDS=0.35`. The Makefile uses optimized Swift for physical builds. Set `CAPTURE_SWIFT_OPTIMIZATION=-Onone` if you need an unoptimized debugger build.

No backend, ledger account or test funds existed in this checkout. The new adapter defaults to **Testnet only** and activates signing only when the operator supplies a funded Testnet seed on the backend. Never put the seed in the app or source tree.

1. Use Node 24 and `cd backend && npm ci`.
2. Run `APP_ID_PREFIX=<Apple App ID prefix> APP_ATTEST_ENV=development APP_BUNDLE_ID=utcq.XRPL-Galaxy APP_BUNDLE_VERSIONS=1 npm start`. Bind a trusted HTTPS reverse proxy to loopback port 8787. Rate-limit enrollment, challenge issuance and publication at that edge. Set a persistent `DB_PATH` and protect the database, wallet seed and backups. The backend stores attestation enrollment receipts locally; continuous Apple fraud-metric assessment is outside this version.
3. Configure the provisioned App ID for App Attest. Build with `make run AMBIENT_BACKEND_URL=https://<backend> APP_ATTEST_ENVIRONMENT=development`. Both development and production attestation policies are isolated; use separate databases and backend URLs. A valid hostname, TLS certificate, signing capability and a physical device are required to test live Apple enrollment.
4. To publish on Testnet, provide `XRPL_TESTNET_SEED` to the backend process. Use one worker with a dedicated signer. It persists the signed blob and transaction hash before broadcast. A submission that expires without reliable validation moves to reconciliation rather than automatically spending again. Verify via `GET /v1/commitments/<root>`. No Mainnet submission path is configured.

The backend needs a backup/recovery plan, logging policy and abuse limits before public hosting. Development allows older assertions without extension fields unless `APP_ATTEST_REQUIRE_EXTENSIONS=1`; production requires allowed version/category extensions. See [Apple App Attest server validation](https://developer.apple.com/documentation/devicecheck/validating-apps-that-connect-to-your-server) and [XRPL reliable submission](https://xrpl.org/docs/concepts/transactions/reliable-transaction-submission).

## Verify and share

On the phone: **Scans → open a scan → Share scan**. Choose **Share full scan** or one category under **Selective disclosure**. AirDrop receives an ordinary ZIP. **Refresh verification** checks the configured backend for a receipt.

On the Mac, unzip the scan and obtain the root independently. Then run:

```sh
node backend/verify-scan.mjs /path/to/inclusion-proof.json <trusted-root>
node backend/verify-scan.mjs /path/to/inclusion-proof.json <trusted-root> <testnet-tx-hash> <trusted-signer-account>
```

The CLI checks the disclosed bytes and their inclusion. With transaction and signer arguments it also independently queries Testnet. The App Attest state comes from the trusted backend and is separate from those checks. See [implementation and validation report](ambient-validation-report.md) for actual results and outstanding device tests.

## Offline physics check

Run this on an unpacked full-scan archive. Python, NumPy, and OpenCV are only needed on the verifying computer. The verifier takes no credentials and does not use the network.

```sh
python3 -m pip install -r backend/requirements-physics.txt
python3 backend/verify-physics.py /path/to/<scan>.physics.json
```

The script finds and tracks features directly in the delivered pixels, uses calibrated shutter depth to fit a rigid motion, tests reprojection and reference depth on held-out tracks, and compares its estimated rotation angle to integrated gyroscope data. It does not use ARKit pose output to fit the motion. It reports `physicallyConsistent`, `inconsistent` or `insufficientEvidence` under documented heuristic gates in the script. It can withhold judgment for a stationary phone, missing gyro or depth, limited camera coverage, or too little parallax. The recording may contain private imagery in addition to the main shot. Selective radio disclosure excludes all frames.

The offline check measures physical agreement. It cannot authenticate the source of those bytes. The separate Merkle verifier checks integrity relative to a root you already trust. Neither is needed to press the shutter.
