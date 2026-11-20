# ADR 0004 — Commitment trees

Status: accepted (2026-11-20). Normative text: spec §6, §18.3.

## Context
- The legacy app commits evidence with a SHA-256 Merkle tree over files (`EvidenceCommitmentBuilder.swift`; recon §6). SHA-256 openings are expensive inside a Groth16 circuit.
- The circuit must open selected 4×4 blocks of the analysis raster and depth map and bind them to public roots; the verifier must recompute the RGB root from the photo alone.

## Decision
- Poseidon over BN254 (circomlib parameters; Rust `light-poseidon` `new_circom`), arity-4 trees of depth 6 (4,096 leaf slots), leaves = 4×4 blocks, positions in Morton order over an 8×8 tile slot grid (rows 6–7 are padding); tile and block labels stay row-major.
- Every hash carries a domain tag `TAG(code, aux)` encoding kind, frame, level and position, so a node cannot be reinterpreted as another node type or position.
- Frame t0 has separate RGB (public, unsalted, `rgb_root`) and depth trees; frames t− and t+ commit RGB + depth in one leaf.
- Components (16 slots: trees, CAMERA, IMU, BLUETOOTH, NOISE, META, padding) are salted with 31 random bytes each and folded into `evidence_root` by a fixed two-level arity-4 tree.

## Consequences
- `rgb_root` is recomputable from any lossless copy of the canonical raster; private evidence is hiding thanks to the salts.
- Rust (`galaxy-crypto`) and circom must agree bit for bit; golden vectors in `test-vectors/protocol` enforce this.
- The legacy SHA-256 file Merkle tree is superseded.
