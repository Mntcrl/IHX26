# ADR 0104 — Exact proof-only reports

**Status:** Accepted

## Context

A CAPTURE can have several PROOF transactions; a successful sibling must not cause an invalid proof to be reported as successful. Revocations can arrive after verification or before historical CAPTUREs are backfilled.

## Decision

Queue reports by exact PROOF hash and evaluate it with `galaxy-verifier`, without image input. Cache the versioned report. A revocation from the relevant account requeues affected cached proofs, including when the link is only learned by later CAPTURE backfill. A generation check prevents an evaluation started before invalidation from overwriting the replacement job.

## Consequences

A cached report neither binds pixels nor overrides independent verification. Reports may be pending while re-evaluation or XRPL history retrieval is in progress.
