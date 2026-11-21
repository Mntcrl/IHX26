# XRPL Galaxy — Progress

Status values: DONE (demonstrated, evidence linked), PARTIAL (some evidence, gaps listed), IN PROGRESS (code exists, not yet demonstrated), INCOMPLETE (not demonstrated; reason given). Measurements come only from real runs; anything else is labelled [INFERENCE].

Normative spec: [`docs/protocol/galaxy-v1-spec.md`](protocol/galaxy-v1-spec.md). Plan: [`docs/PLAN.md`](PLAN.md). Decisions: [`docs/adr/`](adr/README.md). Incidents: [`docs/incidents/`](incidents/).

Last update: 2026-11-21 10:45 CET (orchestrator).

## Modules

| Module | Owner | Status | Evidence / notes |
|---|---|---|---|
| Root workspace, Makefile, toolchain, docs skeleton | Scaffold / orchestrator | DONE | commit 66b4749; `make doctor` exits 0; circom 2.2.3, snarkjs 0.7.6, wasm-bindgen 0.2.130, ptau 2^20/2^21 blake2b-verified (`circuits/ptau.lock.json`) |
| `crates/galaxy-protocol`, `galaxy-crypto` | ProtocolCrypto | DONE | commits ac08964, 75d2d7e; wire codecs, ids, policy, Poseidon vectors vs circomlibjs, trees, sampling, `public_inputs_v1`, Groth16 cross-checked with snarkjs; wasm32 + iOS builds |
| `crates/galaxy-image`, `galaxy-evidence` | ImageEvidence | DONE | commit f45bce6; png-v1/jpeg-v1 rasters identical on x86_64, wasm32, aarch64-apple-darwin; real GXRW ingest |
| `crates/galaxy-geometry` | Geometry / Calibration | DONE | reference engine, synthetic scenes, fixtures, `geometry-tool` incl. `calibration-fit`/`calibration-measure` (commit 1f80c75) |
| `circuits/` core + geometry, `galaxy-witness` | CircuitCore / CircuitGeo / CircuitBuild | DONE | commits 078a254, 1d946ea, f94bf84; main_v1m 876,639 constraints; policy template matches the reference engine on 24 fixtures + 10 real captures; constraint-level negative tests |
| `crates/galaxy-attestation` | Attestation | DONE | commit 04dd7eb; real-device attestation + assertions verified; 60 golden cases |
| `crates/galaxy-xrpl` | XRPL / VerifierHardening | DONE | commits 584ac65, 2bfa7f0; submission with crash recovery, registry, mobile signing; AccountTxnID builders (account preparation SetFlag 5, chained device records) confirmed on Testnet |
| `crates/galaxy-prover` | MobileProver | DONE | commit 98c2584; low-memory development setup, pinned artifacts, iOS staticlib + Swift bindings |
| `crates/galaxy-verifier`, `galaxy-cli` | Verifier / VerifierHardening | DONE | commits 4b462cd, 311a847, 2bfa7f0 + second pass; record signatures, history windows with AccountTxnID walk (third-party Testnet history: verdict unchanged), re-challenge rule, one endpoint per verification, explicit `--lookup` |
| `crates/galaxy-node` | Node / Hardening | DONE | commits 311a847, f345192, d90aafe; Testnet sync with checkpoints, §14 API + OpenAPI, signed discovery, 3-node compose, prefix-fair request budgets, backoff-scheduled verification |
| `crates/galaxy-wasm`, `apps/verifier-web` | WebVerifier | DONE | commit 8148662; CLI/WASM report parity; headless Chromium: three lookup modes, 390 px, CSP |
| `crates/galaxy-devtools` (`galaxy-dev`), e2e scripts | Integration | DONE | commits 8dabc0d + post-hardening run; real-capture CAPTURE→PROOF flows on Testnet with AccountTxnID registrations, image/node/decentralization/history tests |
| `crates/galaxy-ffi` (GalaxyCore) | IOSRust / GalaxyExport | DONE | commits 6768e5a, e87679e; one iOS staticlib + Swift bindings; certified-file export with informational XMP, lossless PNG, calibration previews |
| `apps/capture-ios/` | IOS / IOSDevice / DemoUX / IOSProve | DONE | certified flow on the device (bac026e … 618b482): registration, CAPTURE, on-phone proof, PROOF, export; live progress view; gallery; readiness gates; bounded XRPL requests with failover; developer proving self-test |
| `apps/prover-bench-ios/` | MobileProver | DONE | 500k benchmark on device; superseded by the capture app's own proving self-test for main_v1m |
| `docs/security/`, `galaxy-security-tests` | Security / Hardening | DONE | review 2026-11-21 with two remediation passes; threat model matches the code; 19 decoder property tests |

## Acceptance criteria (brief §25)

Phone evidence (2026-11-21): the iPhone 13 Pro Max app, registered with App Attest (development environment), certified real photos end to end on Testnet; the first one ([CAPTURE 7DEA7F30…](https://testnet.xrpl.org/transactions/7DEA7F30DBCEBE5461CDC1E57B2E3E66A4F5617D6708F6B0250796B3BF4C369D), PROOF in ledger 22,494,467) is `VERIFIED` by the Linux and macOS CLIs and by Chromium/Chrome ([measurements](measurements.md#first-phone-certified-photo)). Desktop evidence from `galaxy-dev` uses a dev-unattested credential and is `UNSUPPORTED` by design.

| # | Criterion | Status | Reason / evidence |
|---|---|---|---|
| 1 | Launch XRPL Galaxy on the physical iPhone 13 Pro Max | DONE | signed builds installed in place and run on the device (launch crash fixed: CoreMotion handler isolation) |
| 2 | Capture a synchronized photograph with actual LiDAR and complementary sensor evidence | DONE | t−/t0/t+ RGB, depth, confidence, poses, IMU, Bluetooth/noise aggregates; readiness gates for tracking and depth |
| 3 | Generate the canonical image and evidence commitments | DONE | on the device via GalaxyCore (ingest + commitments ~0.9–1.0 s); the CAPTURE commitments match the Linux recomputation |
| 4 | Authenticate the capture using the implemented attestation mechanism | DONE | App Attest (development environment) registration + per-capture assertion; verifier checks `registration.attestation`, `capture.assertion`, `capture.counter` |
| 5 | Publish a validated CAPTURE transaction to XRPL Testnet | DONE | from the phone, e.g. CAPTURE 7DEA7F30… (ledger 22,494,316); 597 B-class AccountSet, 10 drops |
| 6 | Derive the sampling challenge from the prescribed future ledger | DONE | Lsel = L0 + 5 on the phone and in every verifier; all three Testnet endpoints returned the same selection ledger |
| 7 | Generate a real Groth16 proof locally on the iPhone | DONE | main_v1m (876,639 constraints) on the phone: witness 0.27–0.38 s, Groth16 6.2–9.1 s, peak footprint ~1.0 GiB |
| 8 | Verify the proof locally before publication | DONE | the app verifies the proof against the challenge before building the PROOF memo |
| 9 | Publish a validated PROOF transaction | DONE | from the phone, PROOF ledgers 22,494,467 and 22,494,772 |
| 10 | Export the certified photograph with embedded XRPL references | DONE | certified jpeg-v1 with Galaxy XMP (+ informational `galaxyinfo`), lossless PNG; Gallery → Share / Photos / Files |
| 11 | Open the photograph in the public verification application on another device | DONE | macOS Chrome (live Testnet) `VERIFIED`; presenter package runs the browser verifier locally on a separate Mac (AirDrop from the iPhone) |
| 12 | Retrieve the required public cryptographic material | DONE | VK and policy from the XRPL CIRCUIT registration, digest-checked against the pinned accepted circuit |
| 13 | Independently verify the photograph and proof | DONE | Linux CLI, macOS CLI, Linux Chromium and macOS Chrome all `VERIFIED` (39 checks) |
| 14 | Display the correct verification result | DONE | `VERIFIED`, `INVALID`, `UNSUPPORTED`, `NOT_FOUND`, `UNAVAILABLE` each demonstrated |
| 15 | Modify the photograph and demonstrate verification failure | DONE | substituted XMP, one changed RGB channel, JPEG recompression → `INVALID` ([measurements](measurements.md)) |
| 16 | Remove Galaxy metadata and recover the proof through photo identifier lookup | DONE | stripped XMP recovered via a real node lookup and via manual references |
| 17 | Stop all Galaxy discovery nodes and verify through embedded XRPL references | DONE | all nodes stopped: direct XMP verification still passes Groth16 (`UNSUPPORTED` for the dev credential) |
| 18 | Ordinary unregistered photograph classified as NOT_FOUND, not fake | DONE | unregistered image → `NOT_FOUND` with the node's coverage stated |
| 19 | Reference engine and circuit decline to certify representative screen-recapture cases | PARTIAL | held-out real screens 0/64, flat wall 0/32; synthetic monitors 0/20 (centre monitor 3/20); depth substitution still passes 20/20 (known v1m weakness); physical recaptures at 0.35–1.5 m pending |
| 20 | Report actual transaction counts, memo sizes, latency, constraints, proving memory and fees | DONE | 2 tx / 20 drops per photo; registration 5 tx / 50 drops (3 device chunks); 876,639 constraints; on-phone proof 6.2–9.1 s at ~1.0 GiB; shutter→certified 35–46 s |

The two-transaction target (CAPTURE + PROOF per photo) is confirmed on Testnet from the phone; registration is one-time (account preparation + device chunks + DEVICE manifest).

## Measurements

| Quantity | Value | Source |
|---|---|---|
| Circuit constraints (`galaxy-pc-v1m`, `main_v1m.circom`, circom 2.2.3 `--O2`, final policy 4ad25185…) | 876,639 (22 public inputs); 868,094 witness wires | CircuitBuild1 2026-11-21, [CONSTRAINTS.md](../circuits/CONSTRAINTS.md) |
| On-device proving, benchmark circuit 500k constraints (iPhone 13 Pro Max, iOS 27.2.1) | witness 0.29 s + prove 3.4 s = 3.7 s; peak phys_footprint 462–542 MB; thermal nominal; proofs verified by snarkjs on Linux | `apps/prover-bench-ios/artifacts/results/` (gitignored), 2026-11-20 20:38 |
| Desktop snarkjs `groth16 setup` feasibility at ~1M constraints (13 GB Linux box) | not feasible (`r1cs info` alone ~5 GB RSS; setup estimated ~10 GB) → Rust low-memory development setup | MobileProver 2026-11-20 20:44 |
| App Attest artefact sizes (development environment) | attestation object 5,830 B; assertions 141 / 142 B | IOS2 device run 2026-11-20 20:44 |
| Transactions per certified photo | 2 (CAPTURE + PROOF), 20 drops; one-time registration 3 txs / 30 drops (account preparation, device chunk, DEVICE manifest) | Integration3 Testnet run 2026-11-21, [measurements.md](measurements.md) |
| Certified-photo timeline, final circuit (desktop prover) | CAPTURE submit → validated 4.4 s; CAPTURE → selection ledger 20 s; selection → PROOF 12 s; PROOF submit → validated 4.8 s | Integration2, [measurements.md](measurements.md) |
| Development setup of the final main_v1m | 863.6 s, 1.09 GB peak RSS | CircuitBuild1 2026-11-21 |
| Desktop proving of the final main_v1m (Linux) | witness 0.42 s + prove 9.6 s; whole capture command 1.40 GB peak RSS | Integration2, [measurements.md](measurements.md) |
| CAPTURE AccountSet, signed blob / Memos field | 597 / 418 B on Testnet with a development credential and AccountTxnID (33 B); 949 / 803 B estimated with a 450 B App Attest assertion, 1,099 / 953 B with 600 B (Memos ≤ 1,024 B) | Integration3; XRPL5, [xrpl.md](protocol/xrpl.md) |
| PROOF AccountSet, signed blob / Memos field | 590 / 411 B (361 B payload, 33 B AccountTxnID) | Integration3, [measurements.md](measurements.md) |
| Fee per transaction (Testnet) | 10 drops (10 consecutive galaxy/test transactions) | XRPL5 Testnet run 2026-11-21 |
| Submit → validated latency (Testnet) | median 6.29 s over 10 transactions | XRPL5 Testnet run 2026-11-21 |
| Development setup of main_v1m, previous policy ef744a7f… (`galaxy-prover dev-setup`, Linux) | 1,275 s, 1.0 GiB peak, 5 threads | MobileProver3 / CircuitCore4 |
| Desktop proving of main_v1m, previous policy ef744a7f… (Linux, 4 rayon threads) | witness 0.34 s + prove 8.62 s, 1.48 GB peak; snarkjs and galaxy-prover verify OK | MobileProver3 |
| Policy template `GalaxyPolicyV1m` alone | 390,865 constraints (`--O2`) | CircuitGeo4 |
| Browser verification time (headless Chromium, offline replay of public XRPL results) | 1.26 s (PNG), 1.43 s (JPEG) from Verify click to report, 31 RPC calls per image (post-hardening run); earlier run 2.16 / 2.22 s | Integration3 / Integration2, [measurements.md](measurements.md) |
| Live CLI verification with RPC recording | 7.7 s (PNG, final circuit) | Integration2 |
| WASM bundle | 4.29 MB raw, 1.26 MB gzip | WebVerifier3 |

## Known limitations

- **Measured weakness of v1m (synthetic, [docs/experiments/synthetic-v1m.md](experiments/synthetic-v1m.md)):** depth taken from another scene with the same RGB passes v1m in 16/20 seeds, as often as genuine room scenes. The RGB–LiDAR edge check is satisfied by textures with edges everywhere, and the temporal photometric check (any pixel of the opened 4×4 block within tolerance) cannot discriminate depth when parallax is sub-pixel at the 256×192 analysis resolution (simulated hand baselines 2–15 mm). arch.docx §07 expects this case to fail; v1m does not reliably achieve it. Candidate v2 changes (new policy/circuit ids, not implemented): larger minimum baseline, exact predicted-pixel photometric checks on more candidates, full-resolution RGB patch commitments, edge-coincidence statistics against a control offset.
- Other synthetic results: flat wall, monitors (frontal/tilted/curved), print, pure rotation, static camera, low texture, reflective surfaces, dynamic scenes and foreign IMU are rejected in 20/20 seeds; under the final policy a centre monitor surrounded by objects passes 3/20, swapped t−/t+ 1/20, depth substitution 20/20 ([calibration-v1m.md](experiments/calibration-v1m.md)).
- **Real captures ([docs/experiments/real-captures-v1m.md](experiments/real-captures-v1m.md)):** the device→camera gyro axis matrix `m_cd` was calibrated from both captures (RMS 33 mrad vs 157 mrad for the initial value; gravity cosines 0.991/0.993) → policy id `ef744a7f…afac`. Capture #2 does not pass (0/17 challenges): it moved 26° and 15 cm in 267 ms at ~0.3 m, outside the motion envelope; the temporal reprojection chain is not yet validated on real data. Small-motion captures of 3D scenes at 0.5–3 m are needed to validate conventions and calibrate gyro_tol / rot_max / baseline.
- **Policy calibration 2026-11-21 ([ADR 0031](adr/0031-scene-held-out-policy-calibration.md), [calibration-v1m.md](experiments/calibration-v1m.md)):** motion floor `baseline_min_um` 500 per side (hand wobble; deviation 7) kept; four thresholds fitted with a scene-level held-out split under the constraint that no synthetic recapture scenario may pass more often than before (an unconstrained fit raised the centre-monitor case to 19/20 and was rejected). Held-out genuine 3-D PASS 395/448 (88.2 %) vs 389/448; held-out screens 0/64, flat wall 0/32; prints and phone-on-table had no held-out scenes. Not a deployment acceptance: independent recapture scenes (monitor/laptop at 0.35–1.5 m frontal and tilted, projector, print at 0.3–0.6 m, phone on a support) are still to be captured.
- **Account history (security review S-01/S-10):** `account_tx` also lists third-party transactions that reference an account (Testnet: Payment, TrustSet, EscrowCreate, SetRegularKey), so verifiers fall back to the account's own `AccountTxnID` chain when a scan budget is exhausted (spec §12/§13, ADRs 0092–0093). On Testnet, 201 third-party Payments left the verdict unchanged. Residual: once a registration account's history is padded, verification cost grows with the owner's own transactions since the capture (default walk budget 4,096 steps, CLI up to 65,536).
- App Attest runs in the development environment (free/personal team, development-signed build); production attestation needs a paid team and an App Store/TestFlight-distributed build.
- Trusted setup is development single-party only (not production-trusted); a multi-party phase-2 ceremony is required before any production use.
- Physical consistency is a property of committed sensor data under a public policy, not proof that a scene is real; a fully synthetic but internally consistent sensor dataset can satisfy it if the acquisition pipeline is compromised.

## Deviations

Normative in [spec §18](protocol/galaxy-v1-spec.md#18-deviations-from-archdocx-each-has-an-adr); each has an ADR in [`docs/adr/`](adr/README.md).

## Blockers and operational issues

- Subagent model limits: GPT 6.2 Sol usage limit reached 2026-11-20 ~20:40, 2026-11-21 ~00:00 and ~04:15 (agents pause until reset); Anthropic account rate limit hit at ~20:55 with 13 concurrent agents → work continues in waves of ≤ 5–8 agents.
- Mac build host and iPhone offline during the night of 2026-11-20/21: Swift integration written without a compiler; compile, install and on-device runs (main_v1m proving, full certified flow, recapture scenes) scheduled for the morning session.
- Mac link: SSH over Wi-Fi drops intermittently (ping RTT 60–1650 ms); `scripts/lib-mac.sh` retries with backoff and multiplexing.
- Code signing over SSH fails (`errSecInternalComponent`, keychain locked in SSH sessions; no Xcode account configured) → builds are signed by a GUI-session LaunchAgent runner (`apps/capture-ios/tools/`). No new bundle ids can be provisioned (no Xcode account), so the prover benchmark temporarily reuses `utcq.XRPL-Galaxy`.
- Xcode 26.6 / iOS SDK 26.5: scheme/destination builds fail ("iOS 26.5 is not installed"); `-target` builds work. iPhone runs iOS 27.2.1.
- 7-day provisioning profiles expire 2026-11-27.
- Device data incident 2026-11-20 20:40 (capture app Documents deleted by a benchmark cleanup): [report](incidents/2026-11-20-device-container-deletion.md); mandatory rules in spec §19.
