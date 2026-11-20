# ADR 0090 — Verifier states, development credentials and PROOF selection

Status: accepted (2026-11-20). Normative text: spec §10, §11, §13. Details: `docs/protocol/verification.md`.

## Context
- Spec §13 defines seven states and the lookup aggregation but leaves open how a development-unattested credential combines with proof outcomes, and how PROOF hints from XMP, callers or nodes relate to the PROOFs actually published by the capture account.
- An untrusted party (image editor, node, caller) must never be able to make a photo look INVALID or VERIFIED by choosing which PROOF transaction to point at.

## Decision
- **PROOF candidates come from XRPL only.** Candidates are the PROOF transactions found in the registration account's `account_tx` history (scanned once from the DEVICE manifest ledger to the latest validated ledger) that name the CAPTURE, in (ledger, TransactionIndex) order. A hinted PROOF that is not in that history is evaluated and listed, but cannot select itself.
- **Rejected vs invalid.** A candidate that is not a publication for this capture's window (unvalidated, failed, other account, other CAPTURE, outside `(Lsel, Lsel + w_proof]`) is *rejected*: listed, no effect on the state. A validated in-window PROOF from the capture account for this CAPTURE whose binding (network, Lsel, Lsel hash, circuit) or Groth16 check fails is *invalid*.
- **Outcome.** The first valid candidate wins and evaluation stops (later candidates are not evaluated). No valid candidate: an invalid one with the window closed → INVALID; window open → PENDING; nothing usable and window closed → INCONCLUSIVE. The report shows the checks of the first invalid (or, failing that, first rejected) candidate.
- **Development credentials.** A dev-unattested registration is fully verified (key id, Ed25519 assertion, uniqueness, revocation, windows, proof) but `registration.attestation` fails with the label "development unattested credential", and a would-be VERIFIED, PENDING or INCONCLUSIVE becomes UNSUPPORTED with "Proof status X" kept in the summary. INVALID stays INVALID. A dev credential has no counter: `capture.counter` is `not_applicable` and replay is bounded by capture_id uniqueness.
- **Missing data is never absence.** NOT_FOUND only when the RPC claims complete history from the inception ledger to the tip; otherwise UNAVAILABLE. A transaction whose blob does not hash to the requested id is an unusable RPC answer (UNAVAILABLE), never evidence about the photo.
- **Registration ledger.** The DEVICE manifest's `reg_ledger_hash` must equal the validated ledger header's hash at `reg_ledger_index` (`registration.resolve`).

## Consequences
- A node or edited XMP can at most hide a capture (UNAVAILABLE / NOT_FOUND in the consulted ranges); it cannot downgrade or upgrade a result.
- Every verification of a capture costs one bounded account history scan (≤ 64 × 200 transactions); accounts with more history report UNAVAILABLE rather than a partial verdict.
- Offline tests cannot reach VERIFIED: that needs a real App Attest credential and a real assertion over a published CAPTURE core (Testnet capture from the device).
