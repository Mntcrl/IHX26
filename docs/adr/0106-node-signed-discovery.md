# ADR 0106 — Signed, replaceable discovery

**Status:** Accepted

## Context

A single peer list can fail or mislead clients. Nodes need a stable publisher identifier, but its existence cannot grant authority over proof validation.

## Decision

Give each node its own Ed25519 key and sign the canonical descriptor with the Galaxy node domain separator. Check network, expiry, and signature. Probe origins from operator configuration, cache, peer exchange, and optionally signed XRPL announcements. Bind an announcement probe to its signed node key.

## Consequences

Nodes can discover one another without project enrollment. A signature authenticates the claims' publisher, not the claims' truth. No trusted bootstrap or verification quorum is required.
