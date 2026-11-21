# XRPL Galaxy — Progress

Status values: DONE (demonstrated, evidence linked), PARTIAL (some evidence, gaps listed), IN PROGRESS (code exists, not yet demonstrated), INCOMPLETE (not demonstrated; reason given). Measurements come only from real runs; anything else is labelled [INFERENCE].

Normative spec: [`docs/protocol/galaxy-v1-spec.md`](protocol/galaxy-v1-spec.md). Plan: [`docs/PLAN.md`](PLAN.md). Decisions: [`docs/adr/`](adr/README.md). Incidents: [`docs/incidents/`](incidents/).

Last update: 2026-11-21 05:00 CET (orchestrator).

## Modules

| Module | Owner | Status | Evidence / notes |
|---|---|---|---|
| Root workspace, Makefile, toolchain, docs skeleton | Scaffold / orchestrator | DONE | commit 66b4749; `make doctor` exits 0; circom 2.2.3, snarkjs 0.7.6, wasm-bindgen 0.2.130, ptau 2^20/2^21 blake2b-verified (`circuits/ptau.lock.json`) |
| `crates/galaxy-protocol`, `galaxy-crypto` | ProtocolCrypto | DONE | commits ac08964, 75d2d7e; wire codecs, ids, policy, Poseidon vectors vs circomlibjs, trees, sampling, `public_inputs_v1`, Groth16 cross-checked with snarkjs; wasm32 + iOS builds |
| `crates/galaxy-image`, `galaxy-evidence` | ImageEvidence | DONE | commit f45bce6; png-v1/jpeg-v1 rasters identical on x86_64, wasm32, aarch64-apple-darwin; real GXRW ingest |
| `crates/galaxy-geometry` | Geometry / Calibration | DONE | reference engine, synthetic scenes, fixtures, `geometry-tool` incl. `calibration-fit`/`calibration-measure` (commit 1f80c75) |
| `circuits/` core + geometry, `galaxy-witness` | CircuitCore / CircuitGeo / CircuitBuild | DONE | commits 078a254, 1d946ea, f94bf84; main_v1m 876,639 constraints; policy template matches the reference engine on 24 fixtures + 10 real captures; constraint-level negative tests |
| `crates/galaxy-attestation` | Attestation | DONE | commit 04dd7eb; real-device attestation + assertions verified; 60 golden cases |
| `crates/galaxy-xrpl` | XRPL / VerifierHardening | DONE, hardening IN PROGRESS | commit 584ac65 (submission with crash recovery, registry, mobile signing, measured sizes); AccountTxnID builders and own-chain walk in progress (security review S-01) |
| `crates/galaxy-prover` | MobileProver | DONE | commit 98c2584; low-memory development setup, pinned artifacts, iOS staticlib + Swift bindings |
| `crates/galaxy-verifier`, `galaxy-cli` | Verifier / VerifierHardening | DONE, hardening IN PROGRESS | commit 4b462cd + 311a847 (§14 lookup); history windows, record signature filter, endpoint pinning, CLI `--lookup` in progress (S-01..S-04, S-06) |
| `crates/galaxy-node` | Node / Hardening | DONE | commits 311a847, f345192; Testnet sync with checkpoints, §14 API + OpenAPI, signed discovery, 3-node compose, /64 request budgets |
| `crates/galaxy-wasm`, `apps/verifier-web` | WebVerifier | DONE | commit 8148662; CLI/WASM report parity; headless Chromium: three lookup modes, 390 px, CSP |
| `crates/galaxy-devtools` (`galaxy-dev`), e2e scripts | Integration | DONE | commit 8dabc0d; real-capture CAPTURE→PROOF flows on Testnet, image/node/decentralization tests |
| `crates/galaxy-ffi` (GalaxyCore) | IOSRust | DONE | commit 6768e5a; one iOS staticlib + Swift bindings; 8 tests; build guard on accepted circuit pins |
| `apps/capture-ios/` | IOS / IOSSwift | IN PROGRESS | acquisition, labels, overlays, GXRW export on device (b4700cc); GalaxyCore integration written 2026-11-21 night without a compiler (Mac offline) — compile and device test pending |
| `apps/prover-bench-ios/` | MobileProver | PARTIAL | 500k benchmark proved on device; main_v1m on-device proving pending |
| `docs/security/`, `galaxy-security-tests` | Security / Hardening | IN PROGRESS | review 2026-11-21 (962e3aa): no critical, 1 high (S-01) being fixed; 19 decoder property tests (f345192); threat-model corrections pending re-review |

## Acceptance criteria (brief §25)

Desktop evidence below comes from `galaxy-dev` on Linux publishing real iPhone captures (GXRW) to Testnet with a dev-unattested credential, so the top verdict there is `UNSUPPORTED` by design; `VERIFIED` requires the App Attest-registered phone flow.

| # | Criterion | Status | Reason / evidence |
|---|---|---|---|
| 1 | Launch XRPL Galaxy on the physical iPhone 13 Pro Max | PARTIAL | capture app installed and launched 2026-11-20; certified flow (GalaxyCore) integrated in Swift overnight, not yet compiled |
| 2 | Capture a synchronized photograph with actual LiDAR and complementary sensor evidence | DONE | 101 device GXRW captures (t−/t0/t+ RGB, depth, confidence, poses, IMU, Bluetooth/noise aggregates; private, `fixtures/real/capture-ios/`) |
| 3 | Generate the canonical image and evidence commitments | PARTIAL | computed from real device captures on Linux with the same Rust code linked into GalaxyCore; on-device run pending |
| 4 | Authenticate the capture using the implemented attestation mechanism | PARTIAL | App Attest attestation + assertions generated on the device and verified (04dd7eb); not yet bound to a published CAPTURE |
| 5 | Publish a validated CAPTURE transaction to XRPL Testnet | PARTIAL | desktop: CAPTURE [3DB9B6D1…](https://testnet.xrpl.org/transactions/3DB9B6D178054422A2477E73AEF2F21E2610EF5AFBD2A7CE94DFD3182B94A971) from a real capture; phone pending |
| 6 | Derive the sampling challenge from the prescribed future ledger | DONE | live: L0 22,486,144 → Lsel 22,486,149 (L0 + 5), challenge derived by prover and verifier ([measurements](measurements.md)) |
| 7 | Generate a real Groth16 proof locally on the iPhone | PARTIAL | 500k-constraint benchmark on device 3.7 s; main_v1m (876,639) on device pending |
| 8 | Verify the proof locally before publication | PARTIAL | desktop flow verifies before PROOF; GalaxyCore exposes local verify; on-device pending |
| 9 | Publish a validated PROOF transaction | PARTIAL | desktop: PROOF [4F1DEC5B…](https://testnet.xrpl.org/transactions/4F1DEC5B73FB09938626B6AE0760728DD061E17A84E0F9C6945476F4F2C97434); phone pending |
| 10 | Export the certified photograph with embedded XRPL references | PARTIAL | desktop PNG and JPEG exports with XMP; on-device export pending |
| 11 | Open the photograph in the public verification application on another device | PARTIAL | browser verifier verified both real exports in headless Chromium (reports equal to the CLI); test from a second physical device pending |
| 12 | Retrieve the required public cryptographic material | DONE | VK and policy from the XRPL CIRCUIT registration, digest-checked against the pinned accepted circuit |
| 13 | Independently verify the photograph and proof | PARTIAL | CLI, browser and 3 independent nodes: image binding + Groth16 pass on real Testnet captures; overall `UNSUPPORTED` (dev credential) until the phone flow |
| 14 | Display the correct verification result | PARTIAL | `UNSUPPORTED` / `INVALID` / `NOT_FOUND` / `UNAVAILABLE` shown correctly; `VERIFIED` pending phone flow |
| 15 | Modify the photograph and demonstrate verification failure | DONE | substituted XMP, one changed RGB channel, JPEG recompression → `INVALID` ([measurements](measurements.md)) |
| 16 | Remove Galaxy metadata and recover the proof through photo identifier lookup | DONE | stripped XMP recovered via a real node lookup and via manual references |
| 17 | Stop all Galaxy discovery nodes and verify through embedded XRPL references | DONE | all nodes stopped: direct XMP verification still passes Groth16 (`UNSUPPORTED` for the dev credential) |
| 18 | Ordinary unregistered photograph classified as NOT_FOUND, not fake | DONE | unregistered image → `NOT_FOUND` with the node's coverage stated |
| 19 | Reference engine and circuit decline to certify representative screen-recapture cases | PARTIAL | held-out real screens 0/64, flat wall 0/32; synthetic monitors 0/20 (centre monitor 3/20); depth substitution still passes 20/20 (known v1m weakness); physical recaptures at 0.35–1.5 m pending |
| 20 | Report actual transaction counts, memo sizes, latency, constraints, proving memory and fees | PARTIAL | measured: 2 tx and 20 drops per photo, sizes, latencies, 876,639 constraints, desktop proving memory; on-device main_v1m memory pending |

The two-transaction target (CAPTURE + PROOF per photo) is confirmed on Testnet (desktop flow); registration is one-time (account preparation + device chunks + DEVICE manifest).

## Measurements

| Quantity | Value | Source |
|---|---|---|
| Circuit constraints (`galaxy-pc-v1m`, `main_v1m.circom`, circom 2.2.3 `--O2`, final policy 4ad25185…) | 876,639 (22 public inputs); 868,094 witness wires | CircuitBuild1 2026-11-21, [CONSTRAINTS.md](../circuits/CONSTRAINTS.md) |
| On-device proving, benchmark circuit 500k constraints (iPhone 13 Pro Max, iOS 27.2.1) | witness 0.29 s + prove 3.4 s = 3.7 s; peak phys_footprint 462–542 MB; thermal nominal; proofs verified by snarkjs on Linux | `apps/prover-bench-ios/artifacts/results/` (gitignored), 2026-11-20 20:38 |
| Desktop snarkjs `groth16 setup` feasibility at ~1M constraints (13 GB Linux box) | not feasible (`r1cs info` alone ~5 GB RSS; setup estimated ~10 GB) → Rust low-memory development setup | MobileProver 2026-11-20 20:44 |
| App Attest artefact sizes (development environment) | attestation object 5,830 B; assertions 141 / 142 B | IOS2 device run 2026-11-20 20:44 |
| Transactions per certified photo | 2 (CAPTURE + PROOF), 20 drops; registration one-time | Integration2 Testnet runs 2026-11-21, [measurements.md](measurements.md) |
| Certified-photo timeline, final circuit (desktop prover) | CAPTURE submit → validated 4.4 s; CAPTURE → selection ledger 20 s; selection → PROOF 12 s; PROOF submit → validated 4.8 s | Integration2, [measurements.md](measurements.md) |
| Development setup of the final main_v1m | 863.6 s, 1.09 GB peak RSS | CircuitBuild1 2026-11-21 |
| Desktop proving of the final main_v1m (Linux) | witness 0.42 s + prove 9.6 s; whole capture command 1.40 GB peak RSS | Integration2, [measurements.md](measurements.md) |
| CAPTURE AccountSet, signed blob / Memos field | 949 / 803 B with a 450 B assertion; 1,099 / 953 B with a 600 B assertion (Memos ≤ 1,024 B) | XRPL5, serialized with galaxy-xrpl, [xrpl.md](protocol/xrpl.md) |
| PROOF AccountSet, signed blob / Memos field | 557 / 411 B (361 B payload) | XRPL5, [xrpl.md](protocol/xrpl.md) |
| Fee per transaction (Testnet) | 10 drops (10 consecutive galaxy/test transactions) | XRPL5 Testnet run 2026-11-21 |
| Submit → validated latency (Testnet) | median 6.29 s over 10 transactions | XRPL5 Testnet run 2026-11-21 |
| Development setup of main_v1m, previous policy ef744a7f… (`galaxy-prover dev-setup`, Linux) | 1,275 s, 1.0 GiB peak, 5 threads | MobileProver3 / CircuitCore4 |
| Desktop proving of main_v1m, previous policy ef744a7f… (Linux, 4 rayon threads) | witness 0.34 s + prove 8.62 s, 1.48 GB peak; snarkjs and galaxy-prover verify OK | MobileProver3 |
| Policy template `GalaxyPolicyV1m` alone | 390,865 constraints (`--O2`) | CircuitGeo4 |
| Browser verification time (headless Chromium, offline replay of public XRPL results) | 2.16 s (PNG), 2.22 s (JPEG) from Verify click to report; 27 RPC calls per image | Integration2, [measurements.md](measurements.md) |
| Live CLI verification with RPC recording | 7.7 s (PNG, final circuit) | Integration2 |
| WASM bundle | 4.29 MB raw, 1.26 MB gzip | WebVerifier3 |

## Known limitations

- **Measured weakness of v1m (synthetic, [docs/experiments/synthetic-v1m.md](experiments/synthetic-v1m.md)):** depth taken from another scene with the same RGB passes v1m in 16/20 seeds, as often as genuine room scenes. The RGB–LiDAR edge check is satisfied by textures with edges everywhere, and the temporal photometric check (any pixel of the opened 4×4 block within tolerance) cannot discriminate depth when parallax is sub-pixel at the 256×192 analysis resolution (simulated hand baselines 2–15 mm). arch.docx §07 expects this case to fail; v1m does not reliably achieve it. Candidate v2 changes (new policy/circuit ids, not implemented): larger minimum baseline, exact predicted-pixel photometric checks on more candidates, full-resolution RGB patch commitments, edge-coincidence statistics against a control offset.
- Other synthetic results: flat wall, monitors (frontal/tilted/curved), print, pure rotation, static camera, low texture, reflective surfaces, dynamic scenes and foreign IMU are rejected in 20/20 seeds; under the final policy a centre monitor surrounded by objects passes 3/20, swapped t−/t+ 1/20, depth substitution 20/20 ([calibration-v1m.md](experiments/calibration-v1m.md)).
- **Real captures ([docs/experiments/real-captures-v1m.md](experiments/real-captures-v1m.md)):** the device→camera gyro axis matrix `m_cd` was calibrated from both captures (RMS 33 mrad vs 157 mrad for the initial value; gravity cosines 0.991/0.993) → policy id `ef744a7f…afac`. Capture #2 does not pass (0/17 challenges): it moved 26° and 15 cm in 267 ms at ~0.3 m, outside the motion envelope; the temporal reprojection chain is not yet validated on real data. Small-motion captures of 3D scenes at 0.5–3 m are needed to validate conventions and calibrate gyro_tol / rot_max / baseline.
- **Policy calibration 2026-11-21 ([ADR 0031](adr/0031-scene-held-out-policy-calibration.md), [calibration-v1m.md](experiments/calibration-v1m.md)):** motion floor `baseline_min_um` 500 per side (hand wobble; deviation 7) kept; four thresholds fitted with a scene-level held-out split under the constraint that no synthetic recapture scenario may pass more often than before (an unconstrained fit raised the centre-monitor case to 19/20 and was rejected). Held-out genuine 3-D PASS 395/448 (88.2 %) vs 389/448; held-out screens 0/64, flat wall 0/32; prints and phone-on-table had no held-out scenes. Not a deployment acceptance: independent recapture scenes (monitor/laptop at 0.35–1.5 m frontal and tilted, projector, print at 0.3–0.6 m, phone on a support) are still to be captured.
- **Account history (security review S-01, fix in progress):** `account_tx` also lists third-party transactions that reference an account (Testnet: TrustSet, EscrowCreate, SetRegularKey), so a long third-party history could exhaust the verifier's scan budget (`UNAVAILABLE`, never a false `VERIFIED`). Fix: AccountTxnID own-transaction chain (spec §12/§13); runs published before the fix use registrations without it and must be redone.
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
