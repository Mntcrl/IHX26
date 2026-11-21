# ADR 0101 — Ledger checkpoints and reconnects

**Status:** Accepted

## Context

A subscription can skip closes on startup or reconnect. A source may retain only part of the history, and a live checkpoint may be recorded before historical gaps are filled.

## Decision

Use validated expanded ledger reads from configured endpoints. Treat WebSocket closes as prompts for binary reads, verify the close hash and transaction count when supplied, and wake gap backfill on reconnect or a discontinuity. Atomically store records, coverage, and a checkpoint; check the hash link to both adjacent checkpoints when present.

## Consequences

No stream notice itself proves coverage. Gaps remain visible, inconsistent adjacency cannot advance coverage, and restarts resume from committed checkpoints.
