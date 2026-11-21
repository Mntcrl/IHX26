# ADR 0104 — Exact proof-only reports

**Status:** Accepted

## Context

A CAPTURE can have several PROOF transactions; a successful sibling must not cause an invalid proof to be reported as successful. Revocations can arrive after verification or before historical CAPTUREs are backfilled.

## Decision

Queue reports by exact PROOF hash and evaluate it with `galaxy-verifier`, without image input. Cache the versioned report. A revocation from the relevant account requeues affected cached proofs, including when the link is only learned by later CAPTURE backfill. A generation check prevents an evaluation started before invalidation from overwriting the replacement job.

Use persisted per-proof exponential backoff on retryable results or deadline expiry (60 seconds initially, capped at six hours). Invalidation for a revocation preserves an existing backoff. Keep the worker busy when only one group is due, but after four consecutive jobs from that group, prefer another due group. Group proofs by sender and device registration from the matching same-account CAPTURE; use the sender alone as a conservative fallback until that CAPTURE is indexed. Migrate existing cache rows in place.

## Consequences

A cached report neither binds pixels nor overrides independent verification. Reports may be pending while re-evaluation or XRPL history retrieval is in progress. Proof verification does not spend backfill or live sync's RPC budget (ADR 0108).
