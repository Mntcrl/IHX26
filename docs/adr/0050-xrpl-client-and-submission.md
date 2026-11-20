# ADR 0050 — XRPL codec, ledger access and reliable submission

Status: accepted (2026-11-20). Normative text: spec §10, §12, §13, §14, §22. Details and measurements: `docs/protocol/xrpl.md`.

## Context
- Every Galaxy surface (CLI, verifier node, browser WASM, iOS through UniFFI) must decode XRPL transactions and the iOS app must sign on device, so the codec and signer cannot depend on Node.js, a server or a native rippled library.
- Public rippled endpoints are untrusted peers for transaction content, but a light client cannot check consensus (validation signatures, ledger state proofs) by itself.
- CAPTURE transactions have a ledger deadline; a blind re-sign after a crash or timeout can produce two different Galaxy records for one purpose.

## Decision
- **Codec.** Pure Rust binary codec in `galaxy-xrpl::codec`, driven by the unmodified `definitions.json` of `ripple-binary-codec` 2.11.0 (vendored in `crates/galaxy-xrpl/vendor/`, SHA-256 `dbd5c60a3b0fffa6c88a71414c5a0e8225192c90161886fe552d70f17e8cf2ff`). Canonical field order on encode, strict decode (no trailing bytes, bounded lengths and nesting). Differential tests against xrpl.js 5.3.0 / ripple-binary-codec 2.11.0 (exact pins in `scripts/xrpl-vectors/`).
- **Keys.** Ed25519 family seeds only (`sEd…`); private key = SHA-512Half(16-byte entropy); seeds zeroized and redacted from `Debug`; signing with `STX\0` prefix; tx hash = SHA-512Half(`TXN\0` ‖ blob).
- **Trust split.** For every transaction returned by an RPC the hash is recomputed from `tx_blob` and must equal the requested or listed hash; the decoded content is therefore bound to the hash. `validated`, `TransactionResult`, `TransactionIndex`, ledger index, ledger hash and close time are RPC-trusted (`proven_by: ledger-rpc`); reports state whether the endpoint is remote or an operator-trusted local rippled. `server_info.network_id` must equal the configured network before any read or submission (fail closed); network 0 is refused unless `allow_mainnet`.
- **Submission.** Sans-IO `Submitter` over an `RpcTransport` trait and a `TxStore` trait (`FileTxStore`: atomic write + fsync of file and directory, compare-and-swap per purpose). The signed blob and hash are persisted before the first submit. Recovery always re-reads the journal and resubmits the identical blob. A new signature for the same purpose is created only by `replace_expired`, and only after a validated ledger beyond `LastLedgerSequence` and a `tx` lookup with `min_ledger..max_ledger` that reports `searched_all: true`. CAPTURE callers decide whether to replace and pass their deadline. Success = validated ∧ `tesSUCCESS`; `tec*` is a validated failure that consumed the fee.
- **Registry.** Bundles are split into ≤ 800-byte chunk memos published strictly serially, then the manifest; retrieval reads `account_tx` over exactly `first_chunk_ledger..last_chunk_ledger` and rejects every condition listed in spec §10. Missing history (`complete_ledgers` does not cover the range, manifest not found) is reported as `HistoryUnavailable`, distinct from invalid registry data.

## Consequences
- The codec follows the pinned definitions; a rippled amendment adding fields requires updating the vendored file (and its recorded SHA-256) and re-running the cross-check.
- A compromised RPC can lie about validation or results, but cannot change the decoded transaction content; operators who need more run a local rippled (`trusted_local_rippled`).
- A crashed CAPTURE can always be recovered without a second, different signed record; if the deadline passes, the capture is abandoned explicitly by the caller.
