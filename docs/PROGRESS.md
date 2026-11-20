# XRPL Galaxy — Progress

Status values: DONE (demonstrated, evidence linked), PARTIAL (some evidence, gaps listed), INCOMPLETE (not demonstrated; reason given). Measurements below come only from real runs; anything else is labelled [INFERENCE].

Normative spec: [`docs/protocol/galaxy-v1-spec.md`](protocol/galaxy-v1-spec.md). Plan: [`docs/PLAN.md`](PLAN.md). Decisions: [`docs/adr/`](adr/README.md).

## Modules

Ownership per spec §1 and §20.

| Module | Owner | Status | Evidence |
|---|---|---|---|
| Root workspace, Makefile, toolchain, docs skeleton | Scaffold / orchestrator | not-started | |
| `crates/galaxy-protocol` | ProtocolCrypto | not-started | |
| `crates/galaxy-crypto` | ProtocolCrypto | not-started | |
| `crates/galaxy-image` | ImageEvidence | not-started | |
| `crates/galaxy-evidence` | ImageEvidence | not-started | |
| `crates/galaxy-geometry` | Geometry | not-started | |
| `crates/galaxy-witness` | CircuitCore | not-started | |
| `crates/galaxy-attestation` | Attestation | not-started | |
| `crates/galaxy-xrpl` | XRPL | not-started | |
| `crates/galaxy-prover` | MobileProver | not-started | |
| `crates/galaxy-verifier` | Verifier | not-started | |
| `crates/galaxy-cli` (`galaxy`) | Verifier | not-started | |
| `crates/galaxy-node` (`galaxy-node`) | Node | not-started | |
| `crates/galaxy-wasm` | WebVerifier | not-started | |
| `crates/galaxy-ffi` | IOS (phase 2) | not-started | |
| `circuits/` core (public inputs, sampling, openings, commitments, PASS, dev setup, artifacts) | CircuitCore | not-started | |
| `circuits/` geometry (§8.2–§8.5 policy math, generated policy constants) | CircuitGeo | not-started | |
| `apps/capture-ios/` | IOS | not-started | |
| `apps/prover-bench-ios/` | MobileProver | not-started | |
| `apps/verifier-web/` | WebVerifier | not-started | |
| `bindings/swift/` | IOS + MobileProver | not-started | |
| `bindings/wasm/` | WebVerifier | not-started | |
| `policies/` | ProtocolCrypto (calibration via orchestrator) | not-started | |
| `test-vectors/` | producing agent per area | not-started | |
| `fixtures/synthetic/` | Geometry | not-started | |
| `deployments/` | ProtocolCrypto (network config), Node (docker compose) | not-started | |
| `docs/security/` | Security | not-started | |

## Acceptance criteria (brief §25)

| # | Criterion | Status | Reason / evidence |
|---|---|---|---|
| 1 | Launch XRPL Galaxy on the physical iPhone 13 Pro Max | INCOMPLETE | not started |
| 2 | Capture a synchronized photograph with actual LiDAR and complementary sensor evidence | INCOMPLETE | not started |
| 3 | Generate the canonical image and evidence commitments | INCOMPLETE | not started |
| 4 | Authenticate the capture using the implemented attestation mechanism | INCOMPLETE | not started |
| 5 | Publish a validated CAPTURE transaction to XRPL Testnet | INCOMPLETE | not started |
| 6 | Derive the sampling challenge from the prescribed future ledger | INCOMPLETE | not started |
| 7 | Generate a real Groth16 proof locally on the iPhone | INCOMPLETE | not started |
| 8 | Verify the proof locally before publication | INCOMPLETE | not started |
| 9 | Publish a validated PROOF transaction | INCOMPLETE | not started |
| 10 | Export the certified photograph with embedded XRPL references | INCOMPLETE | not started |
| 11 | Open the photograph in the public verification application running on another device | INCOMPLETE | not started |
| 12 | Retrieve the required public cryptographic material | INCOMPLETE | not started |
| 13 | Independently verify the photograph and proof | INCOMPLETE | not started |
| 14 | Display the correct verification result | INCOMPLETE | not started |
| 15 | Modify the photograph and demonstrate verification failure | INCOMPLETE | not started |
| 16 | Remove Galaxy metadata without changing the canonical raster and recover the proof through photo identifier lookup | INCOMPLETE | not started |
| 17 | Stop all Galaxy discovery nodes and demonstrate verification through embedded XRPL references | INCOMPLETE | not started |
| 18 | Demonstrate that an ordinary unregistered photograph is classified as NOT_FOUND, not fake | INCOMPLETE | not started |
| 19 | Reference engine and circuit reject or decline to certify representative screen-recapture cases according to the calibrated policy | INCOMPLETE | not started |
| 20 | Report actual transaction counts, memo sizes, latency, constraints, proving memory and fees | INCOMPLETE | not started |

The two-transaction target (CAPTURE + PROOF per photo) must be confirmed by measurement (criterion 20).

## Measurements

None yet. Each entry must name the command, device/host, date and raw output location.

| Quantity | Value | Source |
|---|---|---|
| Transactions per certified photo | — | |
| CAPTURE / PROOF memo size (serialized blob) | — | |
| Fees per photo (drops) | — | |
| Capture → CAPTURE validated latency | — | |
| L0 + 5 → PROOF validated latency | — | |
| Circuit constraints (`galaxy-pc-v1m`) | — | |
| On-device proving time / peak memory (iPhone 13 Pro Max) | — | |
| Browser verification time | — | |

## Known limitations

- None recorded yet beyond the blockers below.

## Deviations

Deviations from `arch.docx` are normative in [spec §18](protocol/galaxy-v1-spec.md#18-deviations-from-archdocx-each-has-an-adr); each has an ADR in [`docs/adr/`](adr/README.md).

## Blockers

Copied from the 2026-11-20 reconnaissance.

- Xcode reports "iOS 26.5 is not installed" platform component → scheme/destination builds fail; only `-target` builds resolve. iPhone runs iOS 27.2.1 while SDK/Xcode is 26.5/26.6.
- Latest Makefile-layout product `build/Debug-iphoneos/XRPL_Galaxy.app` (17:02) is unsigned; last signed product is from 14:11.
- Provisioning profiles expire 2026-11-27 (7-day team profiles) and lack the App Attest entitlement.
- Mac: 8 GB RAM, 8.8 GiB free disk (96 % used); Rust has no iOS targets; circom/snarkjs absent.
- Linux: no `wasm32-unknown-unknown`, wasm-pack/bindgen/opt, circom, snarkjs. (Phase 0 installs the Rust targets, circom, snarkjs and wasm-bindgen-cli repo-locally; see `make doctor`.)
- Hermez ptau URL (`storage.googleapis.com/zkevm/ptau/…_20.ptau`) returns 403; S3 hermez mirror also 403. (Phase 0 resolution recorded in `circuits/ptau.lock.json`.)
- Mac working tree has 17 uncommitted deletions (backend/docs/tests); Mac repo has no remotes and `receive.denyCurrentBranch` unset (pushes to checked-out `main` would be refused).
- External POV repo (`pov/src/planarity.rs`, `pov/setup`, `pov/testdata`) not found under Mac `~/Workspace/IXH26` nor Linux `/home/ivan`.
