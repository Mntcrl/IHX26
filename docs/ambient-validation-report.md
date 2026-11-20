# Passive ambient evidence implementation and validation report

## Delivered

The existing camera, LiDAR overlays, scan library, JPEG, native camera frames and full AirDrop export are retained. New captures add a foreground BLE fingerprint, spatial rays with surface normals, an ARKit pose trajectory, a salted Merkle commitment, private verification metadata and selective AirDrop export. A configurable App Attest client, verification backend and Testnet signing worker are included. See [architecture, protocol and deployment](ambient-architecture.md).

No XRPL client, signing account or App Attest backend was present in the inspected checkout. The deployed iPhone build was therefore tested in local capture mode. The software does not claim an attestation or ledger anchor for these local captures.

## Physical iPhone observation

The app built, installed and launched via `make run` on the connected iPhone 13 Pro Max, using the installed iOS 26.5 SDK. One user-triggered capture from the first device build was copied over the wired development connection for verification. Only aggregate observations are recorded here.

| Measurement | Observed |
|---|---:|
| Requested / reported observed scan time | 10,000 ms / 10,000 ms |
| Radio completeness | Full window |
| CoreBluetooth callbacks | 1,079 |
| Distinct session-local identifiers | 93 |
| Callbacks with advertised TX power | 407 |
| Quantized median RSSI | −70 dBm |
| Quantized RSSI standard deviation | 15 dB |
| ARKit pose samples / normal | 74 / 62 |
| Depth vectors / with surface normals | 768 / 651 |
| Tracking jumps by the initial heuristic | 0 |
| Reconstructed mesh triangles reported by ARKit | 0 |
| Authenticated infrastructure / secure ranging | None / unavailable |

### Bluetooth disabled on the updated device build

A second user-triggered capture after disabling Bluetooth in Settings exported `poweredOff`, zero scan duration, zero callbacks and an incomplete radio window. Camera and depth still saved, and all four Merkle categories verified on the Mac. The scan had 54 normal pose samples, 768 depth vectors, 647 surface normals and zero mesh triangles. Only three tracked-feature/depth comparisons were available, so the expanded spatial check reported **insufficient data**. Restoring Bluetooth is independent of the scan file.

The radio count does not measure physical devices or people. The RF environment and its devices were not inventoried. ARKit frame uptime aligned with the radio callback clock in this capture. The Mac verifier accepted the full artifact commitments and a radio-only inclusion proof. The initial device build reported its pose and depth heuristic passed. The subsequent build adds tracked-feature-to-LiDAR depth comparisons and has a stricter insufficient-data outcome. The zero-triangle result was recorded despite available single-frame scene depth.

## Inertial and camera residual extension

The iPhone 13 Pro Max was rebuilt and launched through `make run`. A user-triggered capture on the installed build produced **992 accelerometer vectors and 992 gyroscope vectors**, each covering the full window at approximately **99.24 Hz**. The Mac confirmed monotonic sensor offsets within the committed window, verified all four Merkle categories and independently recomputed all **4,147,200 signed residual samples** from the unaltered YUV export. The delivered planes were 1920 × 1440 (Y) and 960 × 720 each (Cb and Cr).

Both nearby frame candidates were retained as diagnostics and excluded from measured temporal statistics. The pre-shutter candidate exceeded the 0.2° rotation gate (approximately 0.259°). The post-shutter candidate exceeded the 4 mm translation gate (approximately 4.028 mm). This run verified the rejection path. It did not exercise the measured temporal path on a stationary fixture.

Reproduce the separate exact residual and timing check on the Mac with `python3 tests/check-camera-evidence.py /path/to/<scan>.noise.json`. This checks file structure and measurements derived from the exported samples. It does not verify pre-ISP noise, physical sensor defects or provenance.

## Capture latency and offline physics

The short-window build initially still took **8,895 ms** and **10,564 ms** from shutter to preparing the final evidence commit on two physical scans. The configured 350 ms tail explained only the collection wait. The remaining work was serial and ran in an unoptimized Swift build.

The final build starts JPEG encoding, native camera and spatial residual processing, and shutter LiDAR preparation concurrently with the collection tail. The physical Makefile build enables Swift optimization. A new iPhone 13 Pro Max capture reached the same timing marker in **631 ms**, approximately **14–17× faster** than the two baseline attempts. The marker precedes the final atomic metadata, proof and JPEG writes and does not measure Photos or network completion. The JPEG and all proof files were found on the phone. The Mac verified the Merkle proof and all **4,147,200 camera residuals**, with **531 samples from each motion sensor**.

The OpenCV verifier used physical-device archives and no network. It reported physically consistent when the frames contained enough parallax, insufficient evidence on the low-motion scan, and inconsistent when a copied gyro series was deliberately changed. On the final build, the passing pair had 92 held-out depth matches, 0.65 pixels median reprojection error and 0.057° visual/gyro angular disagreement. The other pair had insufficient translation. These are observed consistency results, not a provenance claim.

## Automated and integration checks

| Check | Result |
|---|---|
| Swift Merkle construction, both four and five leaves | Passed |
| Cross-language Swift → Node verification and a single disclosable category | Passed |
| Changed disclosed bytes | Rejected |
| Full and radio-only ZIPs made from a physical capture with the app's archive writer | Both extracted and verified. The radio-only ZIP contained only the aggregate and inclusion proof. |
| Synthetic radio fixture with powered-off Bluetooth and no observations | Unavailable reported separately from a full zero-callback window |
| Radio deduplication, quantization, partial coverage, and out-of-window filtering | Passed |
| Backend challenge scope, expiration, and already-consumed challenge | Rejected outside policy |
| Synthetic P-256 assertion, changed root/challenge, wrong App ID, disallowed extension, repeated counter | Valid fixture accepted and adversarial fixtures rejected |
| Ledger receipt predicate | Rejected wrong account/root, failed outcome, and unvalidated transaction |
| Malformed App Attest enrollment certificate chains | Rejected |
| Local Node 24 HTTP smoke check | Issued a fresh challenge and refused an unauthenticated commitment |
| Live XRPL Testnet with faucet funds and a synthetic root | Validated `AccountSet`, correct signer and 33-byte memo. Transaction `C4CEC4D1D2D24C554D64472590E88DDF5286B11D8831B2950725E8BE39927EA3`. Only synthetic test data was published. |

To reproduce automated checks from the project folder, install backend dependencies first:

```sh
cd backend && npm ci && npm test
cd ..
./tests/check-commitment.sh
./tests/check-radio.sh
```

The synthetic assertion verifies the backend logic with a generated P-256 key. It is not an Apple-generated assertion and does not establish a live App Attest deployment.

## Outstanding acceptance tests

- **Live Apple App Attest and capture-to-ledger:** Needs the project’s App Attest provisioning capability, a reachable HTTPS backend with app identity configured, and a funded dedicated Testnet signer. No physical scan was submitted to any ledger.
- **Per-device discovery and repeatability:** No controlled peer inventory was available for locked, unlocked, idle or active iPhones, Android phones, AirPods, or watches. The open-environment sample cannot establish which devices were visible, undetected, or advertising. Do not infer coverage from it.
- **Distance, obstructions and durations:** No controlled distance or obstruction measurements were taken. To test, change one variable at a time, log device states separately from scan bytes, and repeat at least five times per condition at 3, 5 and 10 seconds per side. Report proportions of full windows, callback-count variation and zero-observation intervals. With privacy-preserving aggregates enabled, test each device class in an isolated setup against a separate ambient baseline; individual attribution is unavailable.
- **Infrastructure:** There was no trusted transmitter, so token freshness, replay and independent proximity have not been tested over the air. Signed tokens alone do not demonstrate secure ranging.
- **AirDrop:** The Mac built and verified both ZIP variants using the app's archive writer and physical scan files. An actual AirDrop handoff for this build was not observed.
