# ADR 0009 — Network policy

Status: accepted (2026-11-20). Normative text: spec §3, §12.

## Context
- XRPL Testnet (`https://s.altnet.rippletest.net:51234`, network_id 1, rippled 3.4.1) is reachable from Linux and the Mac (recon §10).
- Signing keys are self-custodied by each device or developer; the legacy backend's server-held seed is gone.

## Decision
- Default network: `xrpl-testnet` (network_id 1), configured in `deployments/networks/xrpl-testnet.toml`. Mainnet (network_id 0) is refused unless the config sets `allow_mainnet = true`.
- Transactions: `AccountSet` with only common fields and exactly one Memo; ed25519 wallets; Fee autofilled and capped by `max_fee_drops` (default 1000); `LastLedgerSequence` = latest validated + 20; no `NetworkID` field for ids ≤ 1024.
- Reliable submission: tx hash computed and persisted with the blob before submit; recovery queries by hash and resubmits the same blob; success only when validated with `tesSUCCESS`.
- Dev wallet seeds live outside the repo (`~/.config/galaxy/` on Linux, Keychain on iOS).

## Consequences
- Fees per photo are bounded by two capped transactions; actual fees and memo sizes are measured (PROGRESS criterion 20).
- Mainnet use needs an explicit opt-in and is out of scope for v1 demonstrations.
