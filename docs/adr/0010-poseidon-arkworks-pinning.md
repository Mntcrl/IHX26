# ADR 0010 — Poseidon and arkworks pinning

Status: accepted (2026-11-20). Normative text: spec §0, §6, §9. Owner: ProtocolCrypto.

## Context
- Every commitment (§6) and the 22 public inputs (§9) must be bit-identical between the Rust core (CLI, node, browser WASM, iOS), the Circom circuit (circomlib `Poseidon(n)`) and snarkjs/circomlibjs tooling. A change in round constants, MDS matrix, field representation or point serialization in any dependency silently breaks every certificate.
- ADR 0002 fixes the shared dependency set; this ADR records the cryptographic constraints on it.

## Decision
- Poseidon: `light-poseidon =0.5.0`, constructor `Poseidon::<Fr>::new_circom(n)` for n = 1..=5 only (`galaxy_crypto::poseidon::hash`); any other arity is a programming error. Golden vectors for every arity come from circomlibjs `buildPoseidon` (`test-vectors/protocol/poseidon.json`, `scripts/protocol-gen-vectors.mjs`).
- Curve and proof system: arkworks 0.6.0 (`ark-bn254`, `ark-ff`, `ark-ec`, `ark-groth16` with default features off), exact `=` pins in the workspace table. galaxy-crypto does not use arkworks serialization: the §9 byte layouts are encoded coordinate by coordinate in `galaxy_crypto::groth16`, so an arkworks format change cannot alter wire bytes.
- The Fq2 `[c0, c1]` convention and the full verifier path are pinned against a real snarkjs proof (`test-vectors/protocol/groth16/`, `scripts/protocol-groth16-vectors.sh`).
- Upgrading any of these crates is acceptable only if every existing vector passes unchanged; a vector change is a protocol change (new policy/circuit ids).

## Consequences
- `cargo test -p galaxy-crypto` detects any drift against circomlib/snarkjs before it reaches a device.
- The crates compile unchanged for wasm32-unknown-unknown and aarch64-apple-ios (no OS-specific dependency in galaxy-crypto/galaxy-protocol).
