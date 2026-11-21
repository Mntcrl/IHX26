# ADR 0100 — Disposable node cache

**Status:** Accepted

## Context

Nodes offer reference lookup and proof diagnostics but must not become authoritative registries. An operator must be able to reconstruct the cache without changing the node's discovery identity.

## Decision

Use versioned SQLite in WAL mode for the index, checkpoints, coverage, reports, and peer cache. Bind the database to the configured network, inception ledger, and scan mode. `rebuild` clears cached rows and reruns indexing, preserving the independent Ed25519 key file.

## Consequences

No information solely held by the node is required to verify a photograph. Rebuilding can leave results incomplete until the operator's XRPL sources serve the required history.
