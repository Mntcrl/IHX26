# ADR 0103 — Memo admission and ordering

**Status:** Accepted

## Context

Notifications, duplicate scans, and malformed memos can otherwise produce conflicting or unverified references.

## Decision

Use binary transactions and decoded metadata from `galaxy-xrpl`, accepting only validated `tesSUCCESS`. Strictly decode the Galaxy envelope and kind-specific payload, retaining malformed memos only as rejected records. Derive index keys, deduplicate by transaction hash, and reject conflicting ledger/transaction positions. Store assertion counters as hints, not as substitutes for credential checks.

## Consequences

A ledger replay does not add rows or change order. An indexed reference is a lookup hint, not a verified claim about the memo's context.
