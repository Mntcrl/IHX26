# XRPL Galaxy — Progress

Status values: DONE (demonstrated, evidence linked), PARTIAL (some evidence, gaps listed), IN PROGRESS (code exists, not yet demonstrated), INCOMPLETE (not demonstrated; reason given). Measurements come only from real runs; anything else is labelled [INFERENCE].

Normative spec: [`docs/protocol/galaxy-v1-spec.md`](protocol/galaxy-v1-spec.md). Plan: [`docs/PLAN.md`](PLAN.md). Decisions: [`docs/adr/`](adr/README.md). Incidents: [`docs/incidents/`](incidents/).

Last update: 2026-11-20 21:45 CET (orchestrator).

## Modules

| Module | Owner | Status | Evidence / notes |
|---|---|---|---|
| Root workspace, Makefile, toolchain, docs skeleton | Scaffold / orchestrator | DONE | commit 66b4749; `make doctor` exits 0; circom 2.2.3, snarkjs 0.7.6, wasm-bindgen 0.2.130, ptau 2^20/2^21 blake2b-verified (`circuits/ptau.lock.json`) |
| `crates/galaxy-protocol` | ProtocolCrypto | IN PROGRESS | commit ac08964 (wip) |
| `crates/galaxy-crypto` | ProtocolCrypto | IN PROGRESS | commit ac08964 (wip); Poseidon vectors vs circomlibjs, trees, sampling, public inputs |
| `crates/galaxy-image` | ImageEvidence | IN PROGRESS | commit c34897e (wip); JPEG reference decoder switched to zune-jpeg (jpeg-decoder spawns threads unconditionally) |
| `crates/galaxy-evidence` | ImageEvidence | IN PROGRESS | commit c34897e (wip) |
| `crates/galaxy-geometry` | Geometry | IN PROGRESS | commit bfd575f (wip) |
| `crates/galaxy-witness` | CircuitCore | IN PROGRESS | commit 65caa70 (wip) |
| `circuits/` core + geometry | CircuitCore / CircuitGeo | IN PROGRESS | commit 65caa70; `main_v1m.circom` compiles (circom 2.2.3 `--O2`): 905,931 constraints, 22 public inputs, 3,481 private inputs |
| `crates/galaxy-attestation` | Attestation | IN PROGRESS | commit b499654 (wip) |
| `crates/galaxy-xrpl` | XRPL | IN PROGRESS | commit 7abda62 (wip); codec checked against 256 real Testnet transactions |
| `crates/galaxy-prover` | MobileProver | IN PROGRESS | commit a3618d6 (wip); iOS static library cross-compiled on Linux |
| `crates/galaxy-verifier`, `galaxy-cli` | Verifier | IN PROGRESS | commit 9335e2d (wip; does not compile yet) |
| `crates/galaxy-node` | Node | IN PROGRESS | commit 9335e2d (wip) |
| `crates/galaxy-wasm`, `apps/verifier-web` | WebVerifier | IN PROGRESS | commit 9335e2d (wip) |
| `crates/galaxy-ffi` | IOS | not started | |
| `apps/capture-ios/` | IOS | IN PROGRESS | commit acf43fc (wip); POV/backend code removed; GUI-session build runner signs builds |
| `apps/prover-bench-ios/` | MobileProver | PARTIAL | commit a3618d6; 500k benchmark proved on device (see Measurements) |
| `docs/security/` | Security | not started | |

## Acceptance criteria (brief §25)

| # | Criterion | Status | Reason / evidence |
|---|---|---|---|
| 1 | Launch XRPL Galaxy on the physical iPhone 13 Pro Max | PARTIAL | the restructured capture app was built (GUI-session runner), installed and launched on the device 2026-11-20 20:43; full Galaxy flow not yet integrated |
| 2 | Capture a synchronized photograph with actual LiDAR and complementary sensor evidence | PARTIAL | two raw GXRW captures (t−/t0/t+ RGB, depth, confidence, poses, IMU) exported from the device to `fixtures/real/capture-ios/` (private, gitignored); IMU↔ARKit timestamp offset −3.1 / −5.0 ms |
| 3 | Generate the canonical image and evidence commitments | IN PROGRESS | Rust ingest/commitment code exists; not yet run on the real captures |
| 4 | Authenticate the capture using the implemented attestation mechanism | PARTIAL | App Attest works on the device without the entitlement (development environment): isSupported, generateKey, attestKey (5,830 B), generateAssertion (141/142 B); verifier-side validation in progress |
| 5 | Publish a validated CAPTURE transaction to XRPL Testnet | INCOMPLETE | XRPL submission code exists; no Galaxy CAPTURE published yet |
| 6 | Derive the sampling challenge from the prescribed future ledger | IN PROGRESS | sampling implemented + vectors; no live run yet |
| 7 | Generate a real Groth16 proof locally on the iPhone | PARTIAL | benchmark circuit (500k constraints) proved on device; Galaxy circuit not yet |
| 8 | Verify the proof locally before publication | INCOMPLETE | not started |
| 9 | Publish a validated PROOF transaction | INCOMPLETE | not started |
| 10 | Export the certified photograph with embedded XRPL references | INCOMPLETE | XMP embed code in progress |
| 11 | Open the photograph in the public verification application on another device | INCOMPLETE | web app in progress |
| 12 | Retrieve the required public cryptographic material | INCOMPLETE | |
| 13 | Independently verify the photograph and proof | INCOMPLETE | |
| 14 | Display the correct verification result | INCOMPLETE | |
| 15 | Modify the photograph and demonstrate verification failure | INCOMPLETE | |
| 16 | Remove Galaxy metadata and recover the proof through photo identifier lookup | INCOMPLETE | |
| 17 | Stop all Galaxy discovery nodes and verify through embedded XRPL references | INCOMPLETE | |
| 18 | Ordinary unregistered photograph classified as NOT_FOUND, not fake | INCOMPLETE | |
| 19 | Reference engine and circuit decline to certify representative screen-recapture cases | INCOMPLETE | synthetic scenario suite in progress; no physical screen experiments yet (need the user handling the phone) |
| 20 | Report actual transaction counts, memo sizes, latency, constraints, proving memory and fees | PARTIAL | constraints and benchmark proving measured (below); XRPL numbers pending |

The two-transaction target (CAPTURE + PROOF per photo) must be confirmed by measurement (criterion 20).

## Measurements

| Quantity | Value | Source |
|---|---|---|
| Circuit constraints (`galaxy-pc-v1m`, `main_v1m.circom`, circom 2.2.3 `--O2`) | 905,931 (22 public inputs, 3,481 private inputs) | CircuitCore2 compile 2026-11-20 ~21:05 |
| On-device proving, benchmark circuit 500k constraints (iPhone 13 Pro Max, iOS 27.2.1) | witness 0.29 s + prove 3.4 s = 3.7 s; peak phys_footprint 462–542 MB; thermal nominal; proofs verified by snarkjs on Linux | `apps/prover-bench-ios/artifacts/results/` (gitignored), 2026-11-20 20:38 |
| Desktop snarkjs `groth16 setup` feasibility at ~1M constraints (13 GB Linux box) | not feasible (`r1cs info` alone ~5 GB RSS; setup estimated ~10 GB) → Rust low-memory development setup | MobileProver 2026-11-20 20:44 |
| App Attest artefact sizes (development environment) | attestation object 5,830 B; assertions 141 / 142 B | IOS2 device run 2026-11-20 20:44 |
| Transactions per certified photo | — | pending |
| CAPTURE / PROOF memo size (serialized blob) | — | pending |
| Fees per photo (drops) | — | pending |
| Latencies (submit → validated, L0 → Lsel, Lsel → PROOF) | — | pending |
| Browser verification time | — | pending |

## Known limitations

- **Measured weakness of v1m (synthetic, [docs/experiments/synthetic-v1m.md](experiments/synthetic-v1m.md)):** depth taken from another scene with the same RGB passes v1m in 16/20 seeds, as often as genuine room scenes. The RGB–LiDAR edge check is satisfied by textures with edges everywhere, and the temporal photometric check (any pixel of the opened 4×4 block within tolerance) cannot discriminate depth when parallax is sub-pixel at the 256×192 analysis resolution (simulated hand baselines 2–15 mm). arch.docx §07 expects this case to fail; v1m does not reliably achieve it. Candidate v2 changes (new policy/circuit ids, not implemented): larger minimum baseline, exact predicted-pixel photometric checks on more candidates, full-resolution RGB patch commitments, edge-coincidence statistics against a control offset.
- Other synthetic results: flat wall, monitors (frontal/tilted/curved), print, pure rotation, static camera, low texture, reflective surfaces, dynamic scenes and foreign IMU are rejected in 20/20 seeds; a centre monitor surrounded by objects passes 2/20, swapped t−/t+ 1/20, RGB from another viewpoint 4–5/20; genuine synthetic rooms pass 16/20 (the 4 failures have baselines < 3 mm).
- **Real captures ([docs/experiments/real-captures-v1m.md](experiments/real-captures-v1m.md)):** the device→camera gyro axis matrix `m_cd` was calibrated from both captures (RMS 33 mrad vs 157 mrad for the initial value; gravity cosines 0.991/0.993) → policy id `ef744a7f…afac`. Capture #2 does not pass (0/17 challenges): it moved 26° and 15 cm in 267 ms at ~0.3 m, outside the motion envelope; the temporal reprojection chain is not yet validated on real data. Small-motion captures of 3D scenes at 0.5–3 m are needed to validate conventions and calibrate gyro_tol / rot_max / baseline.
- **Policy amendment 2026-11-20 23:00 (spec §8.5, deviation 7), implementation pending:** camera translation no longer gates capture or PASS. Done: the capture app no longer rejects low-motion captures (build installed 22:57). Pending (owners overnight): remove `baseline_min_um` from `policies/galaxy-pc-v1m.json` and `galaxy-protocol::Policy`, drop the baseline conjunct from `galaxy-geometry` `motion_ok` and from `circuits/components/geometry/sensors.circom`, regenerate policy fixtures and generated circuit constants, recompile main_v1m, redo the development setup, update verifier reports ("no parallax evidence claimed").
- App Attest runs in the development environment (free/personal team, development-signed build); production attestation needs a paid team and an App Store/TestFlight-distributed build.
- Trusted setup is development single-party only (not production-trusted); a multi-party phase-2 ceremony is required before any production use.
- Physical consistency is a property of committed sensor data under a public policy, not proof that a scene is real; a fully synthetic but internally consistent sensor dataset can satisfy it if the acquisition pipeline is compromised.

## Deviations

Normative in [spec §18](protocol/galaxy-v1-spec.md#18-deviations-from-archdocx-each-has-an-adr); each has an ADR in [`docs/adr/`](adr/README.md).

## Blockers and operational issues

- Subagent model limits: GPT 6.2 Sol usage limit reached 2026-11-20 ~20:40 (reset ≈ 5 h later); Anthropic account rate limit hit at ~20:55 with 13 concurrent agents → work continues in waves of ≤ 5 agents.
- Mac link: SSH over Wi-Fi drops intermittently (ping RTT 60–1650 ms); `scripts/lib-mac.sh` retries with backoff and multiplexing.
- Code signing over SSH fails (`errSecInternalComponent`, keychain locked in SSH sessions; no Xcode account configured) → builds are signed by a GUI-session LaunchAgent runner (`apps/capture-ios/tools/`). No new bundle ids can be provisioned (no Xcode account), so the prover benchmark temporarily reuses `utcq.XRPL-Galaxy`.
- Xcode 26.6 / iOS SDK 26.5: scheme/destination builds fail ("iOS 26.5 is not installed"); `-target` builds work. iPhone runs iOS 27.2.1.
- 7-day provisioning profiles expire 2026-11-27.
- Device data incident 2026-11-20 20:40 (capture app Documents deleted by a benchmark cleanup): [report](incidents/2026-11-20-device-container-deletion.md); mandatory rules in spec §19.
