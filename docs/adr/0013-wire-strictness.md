# ADR 0013 — Wire strictness rules

Status: accepted (2026-11-20). Normative text: spec §2, §10, §17. Owner: ProtocolCrypto.

## Context
- Galaxy memos and bundles are read from a public ledger: any account can write any bytes. Lenient decoders create malleability (two encodings of one statement), parser differentials between verifiers (Rust CLI/node/WASM/iOS vs. JS tooling) and resource-exhaustion paths.

## Decision
- One decode function per format (`galaxy_protocol::wire`) and one canonical encoding: every decoder checks version = 1, magic, every length field, enum values and invariants, and rejects trailing bytes; a successful decode re-encodes to the identical bytes (tested with random mutations).
- Bounds are checked before allocation: assertion ≤ 600 B, chunk payload 1..=800 B, bundles ≤ 1 MiB, manifest spans consistent (`chunk_total ≤ bundle_len ≤ chunk_total·800`, first ≤ last ledger), `index < total`.
- Reserved CAPTURE flag bits 4–15 must be zero; image profile ∈ {png-v1, jpeg-v1}; wire scalars < r; device kind/environment pairs are restricted (env n/a iff dev-unattested); dev-unattested assertions are exactly `0xF0 ‖ sig[64]` and dev keys exactly 32 bytes.
- Bundle JSON documents must already be canonical objects; ids are computed only over validated canonical bytes.
- MemoType matching is exact (`MemoKind::from_memo_type`), no case folding or trimming.
- Decoders check only what the bytes alone determine; ledger-context rules (sender account, REVOKE ranges, chunk reassembly, App Attest verification) stay with their consumers and are never assumed by the codec.

## Consequences
- 13 valid vectors round-trip byte-exactly; 57 reject vectors (`test-vectors/protocol/wire.json`) and truncation/extension/mutation tests pin the rules for all language surfaces.
- Any future relaxation requires a new version byte.
