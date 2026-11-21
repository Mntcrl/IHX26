# ADR 0102 — Account-scoped coverage

**Status:** Accepted

## Context

Operators with limited XRPL history or bandwidth may want to index known publishers without scanning every ledger transaction.

## Decision

Scan explicitly selected or observed accounts using forward binary `account_tx` over available ranges. A range is covered only after its terminal page; a marker belongs to the endpoint issuing it. Keep this coverage separate from global coverage, and index only records published by the scoped account.

## Consequences

The account mode can find references efficiently but cannot establish global absence. It always advertises incomplete global coverage, even when all configured accounts have caught up.
