# ADR 0006 — Trust model

Status: accepted (2026-11-20).

## Context
The system combines several trust sources with different strengths: XRPL consensus, the RPC endpoint used to read it, optional discovery nodes, the Groth16 setup, Apple App Attest, and the physical sensors. The legacy design trusted a project backend and a committee coordinator (recon §7, "Incompatible with a no-coordinator architecture").

## Decision
- XRPL Testnet consensus orders and timestamps CAPTURE/PROOF transactions and provides the unpredictable future-ledger challenge (`Lsel = L0 + 5`).
- RPC trust is explicit: reports list `rpc_endpoints` and `rpc_trust: remote|local`; a local rippled can be configured for full independence.
- Discovery nodes (`galaxy-node`) only cache and index; they never decide validity, and node or peer data never affects a verdict (spec §14, §17).
- The verifying key is trusted only from explicit accepted-circuit configuration (`circuit_id`, `vk_digest`, `policy_id`, registration tx). The development setup is labelled `development-single-party`.
- App Attest binds a capture to a genuine app instance on Apple hardware; dev-unattested credentials can never yield VERIFIED.
- No quorum, committee or project server is part of verification.
- Sensor provenance is not proven: the proof shows physical consistency of committed sensor data under the policy, not that the scene was real (Bluetooth, noise and meta are committed only).

## Consequences
- Verification works with only the image, an XRPL RPC endpoint and the accepted-circuit config; nodes are optional.
- Reports must state limitations (RPC trust, development ceremony, attestation environment) instead of confidence scores.
