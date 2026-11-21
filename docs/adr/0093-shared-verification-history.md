# ADR 0093 — Reuse history and network checks within an attempt

## Context

[ADR 0092](0092-own-transaction-history.md) bounds third-party history padding by falling back to an account’s own transaction chain. Independent scans for PROOF and revocation could still walk that chain twice and repeat older work from the CAPTURE history. Per-request NetworkID preflights also added a round trip to each walk step.

## Decision

Keep the three logical windows with two physical scans: A scans DEVICE through CAPTURE, and B/C share a scan from the capture ledger through the evaluation tip for PROOF and revocation. Reuse A’s revocation records. When A uses a backward walk, B/C also check same-ledger successors of the CAPTURE before rejoining A at the known CAPTURE hash. If A paged all of L0, the shared scan only needs transactions in later ledgers. Each scan has its own page and walk budgets and fails closed on an incomplete chain. Native verification and the browser check NetworkID once per pinned attempt; registry reads reuse the attempt’s validated tip and history coverage. Standalone native commands retain their per-request network checks.

In A, any earlier CAPTURE from the same account with a matching photo ID, RGB root or evidence root makes a later CAPTURE invalid, even under another registration. Capture ID uniqueness and assertion counters remain per registration. Photo lookup evaluates only the earliest XRPL-authenticated binding hint in ledger order, across registrations and accounts, and lists later binding CAPTUREs without evaluating their proofs.

## Consequences

Walk cost after the capture no longer grows with history before L0. High own-account activity can still exhaust the bounded walk or request deadline. CLI callers can raise the per-walk budget from the default 4,096 to at most 65,536 steps. The network check applies to one endpoint URL, not a quorum or an assurance that an HTTP load balancer uses one server. Lookup order is only as complete as the consulted nodes’ hints. Direct and manual verification do not consider a CAPTURE earlier than the referenced DEVICE manifest, even on the same account.
