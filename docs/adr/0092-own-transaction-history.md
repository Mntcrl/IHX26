# ADR 0092 — Walk device-account history through AccountTxnID

## Context

A device account's `account_tx` includes third-party transactions. Bounded pagination alone can make unrelated traffic exhaust a verifier's budget. Testnet showed that even with DepositAuth, a third-party TrustSet or EscrowCreate can advance the account's `PreviousTxnID`. See [the measured transactions](../protocol/xrpl.md#testnet-account-history-tests-2026-11-21).

## Decision

Enable `AccountTxnID` with a memo-free, journaled AccountSet before publishing device chunks. The first device chunk may sign the zero sentinel; subsequent records sign the tracked hash of the last successful transaction from that account. Failed own transactions and third-party transactions do not advance it. CIRCUIT records do not need this field. Only the registration account's Ed25519 master-key-signed Galaxy records contribute to verification.

Scan independent CAPTURE, PROOF, and revocation windows. When pagination exhausts its budget, discard the partial scan and walk `AccountTxnID` backward. For own successful transactions that omitted the signed field, use the AccountRoot's previous value from transaction metadata. CAPTURE/revocation walks must reach the referenced DEVICE publication. Unavailable history, an incomplete chain, and exhausted work budgets fail closed.

A verification uses one endpoint and restarts from the beginning on failover. Reports disclose the final endpoint, and offline recordings contain that attempt's answers only. CLI node lookup requires explicit opt-in.

## Consequences

Third-party ledger activity can force a fallback scan, but cannot make the own-transaction chain longer. The verifier still trusts the endpoint for validation, history completeness, ordering and AccountRoot/metadata links; endpoint rotation is not a quorum. DEVICE and CAPTURE records produced without `AccountTxnID` before this change require a new DEVICE registration and CAPTURE. A valid historical REVOKE without the field remains effective.
