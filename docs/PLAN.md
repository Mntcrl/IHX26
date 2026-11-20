# XRPL Galaxy — Implementation Plan (docs/PLAN.md)

Normative spec: `docs/protocol/galaxy-v1-spec.md` (derived from `arch.docx`, the primary reference). Progress and measurements: `docs/PROGRESS.md`. Decisions: `docs/adr/`.

## Starting point (inventory 2026-11-20)
- iOS app (history imported from the Mac remote `mac/main`, 7 commits): SwiftUI + ARKit (`sceneDepth`), rolling buffers, CoreMotion 100 Hz, CoreBluetooth HMAC-dedup scanner, SHA-256 Merkle commitment, App Attest client talking to a project backend, Secure Enclave signing, POV committee client (Shamir/HPKE shares to a coordinator at a LAN address).
- Central components incompatible with the target architecture (removed): `backend/` (challenge issuer, App Attest verifier, custodial anchoring wallet, SQLite), POV coordinator/committee client, Shamir/HPKE share code. The user had already deleted `backend/`, `docs/`, `tests/` in the Mac working tree (uncommitted); the monorepo commit removes them.
- No XRPL signing, wallet, ZK circuit, prover, verifier, node or web app existed.
- Toolchains: Linux (8 CPU, 13 GB RAM) Rust 1.91, Node 26, Docker; Mac M2 8 GB RAM, ~8.8 GB free disk, Xcode 26.6 / iOS SDK 26.5, iPhone 13 Pro Max on iOS 27.2.1; free/personal-team 7-day provisioning without the App Attest entitlement.

## Architecture in one picture
```
iPhone (capture-ios + GalaxyCore Rust via UniFFI)
  ARKit t-/t0/t+ + depth/conf + poses, CoreMotion, BT aggregate, noise
  -> canonical PNG/JPEG + analysis raster -> Poseidon commitments (rgb_root, evidence_root)
  -> App Attest assertion over core299 -> CAPTURE AccountSet memo (tx 1)
  -> wait L0+5 validated -> seed -> sampled tiles -> witness -> Groth16 prove (on device) -> self-verify
  -> PROOF AccountSet memo (tx 2) -> export PNG/JPEG + XMP refs
XRPL Testnet: registry of DEVICE / CIRCUIT bundles (one-time), CAPTURE + PROOF (per photo)
Verifier (galaxy-verifier in Rust: CLI, node, WASM in browser)
  image -> photo_id/rgb_root -> XMP refs | photo_id lookup via any node | manual tx
  -> fetch txs/ledgers from XRPL -> attestation, counters, windows, seed, public inputs -> Groth16 verify -> report
galaxy-node (optional, untrusted): XRPL backfill + subscribe -> SQLite cache -> REST + discovery
```

## Workstreams and owners (agents)
| Workstream | Owner | Main outputs |
|---|---|---|
| Scaffold, history merge, tools, docs skeleton | Scaffold | monorepo, Makefile, toolchain, ptau |
| Protocol + crypto core | ProtocolCrypto | galaxy-protocol, galaxy-crypto, protocol vectors, policy file |
| Image + evidence | ImageEvidence | galaxy-image, galaxy-evidence, image/evidence vectors |
| Reference geometry | Geometry | galaxy-geometry, synthetic scenes, calibration, policy vectors |
| ZK circuit | Circuit | circuits/, galaxy-witness, dev setup, constraint report, adversarial tests |
| XRPL | XRPL | galaxy-xrpl, testnet integration, registry publishing |
| App Attest | Attestation | galaxy-attestation, attestation vectors |
| iOS capture | IOS | apps/capture-ios, galaxy-ffi integration |
| Mobile prover | MobileProver | galaxy-prover, iOS Rust build pipeline, prover-bench-ios, measurements |
| Verifier + CLI | Verifier | galaxy-verifier, galaxy-cli |
| Node + discovery | Node | galaxy-node, OpenAPI, Docker, 3-node compose |
| Browser verifier | WebVerifier | galaxy-wasm, apps/verifier-web |
| Security review | Security | docs/security/, adversarial findings |

## Phases
- Phase 0: scaffold (history merge, layout, workspace, tools, ptau, docs).
- Phase 1 (parallel): ProtocolCrypto, ImageEvidence, Geometry, Circuit, XRPL, Attestation, IOS (Swift pipeline + App Attest feasibility), MobileProver (toolchain + on-device benchmark).
- Phase 2 (parallel): Verifier + CLI, Node, WebVerifier, galaxy-ffi + iOS integration, circuit ↔ reference differential tests, dev setup + circuit registration on Testnet.
- Phase 3: device captures + calibration, end-to-end Testnet runs, decentralization tests (3 nodes), image tamper tests, security review, measurements, docs.

## Milestones (brief §24) → phases
M1 protocol foundation (P1) · M2 XRPL publication/retrieval (P1–P2) · M3 iOS acquisition (P1) · M4 reference geometry (P1, calibration P3) · M5 circuit (P1–P2) · M6 mobile proving + App Attest (P1–P2) · M7 node/CLI (P2) · M8 web verifier (P2) · M9 full integration (P3) · M10 security review (P3).

## Acceptance (brief §25) tracking
Each criterion is tracked in `docs/PROGRESS.md` with status DONE / PARTIAL / INCOMPLETE and evidence (command output, tx hashes, measurements). Physical-scene experiments need the user to handle the phone; they are scheduled in Phase 3.

