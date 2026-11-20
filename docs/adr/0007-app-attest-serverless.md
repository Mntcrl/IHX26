# ADR 0007 — Serverless App Attest

Status: accepted (2026-11-20). Normative text: spec §11, §18.5.

## Context
- The legacy app obtained App Attest challenges from its own backend (`CaptureAttestationService.swift`, `POST v1/challenges`) and the backend verified attestations and assertions (recon §6–§7). The target architecture has no project server.
- Current provisioning profiles are 7-day personal-team profiles without the App Attest entitlement (recon §4).

## Decision
- Registration challenge: the app reads a validated ledger (index, hash) and a random 32-byte nonce; `clientDataHash` binds network, ledger, nonce, key id and account (spec §3). The ledger must fall within a bounded window (≤ 100 ledgers) of the DEVICE manifest.
- The attestation object (receipt stripped) is published on XRPL as a chunked device bundle plus a DEVICE manifest from the same wallet; anyone can verify it against the pinned Apple App Attestation Root CA, with certificate validity evaluated at the ledger close time.
- Capture assertions sign `GALAXY/ASSERT/v1\0 ‖ core299`; counters must strictly increase in ledger order.
- Development vs production environment is reported and accepted per verifier config. A labelled dev-unattested Ed25519 credential (kind 0xF0) exists for testing and never yields VERIFIED.

## Consequences
- No server is needed to issue challenges or verify attestations.
- If the App Attest entitlement cannot be provisioned for the personal team, end-to-end runs can only use the dev-unattested credential, and criterion 4 stays INCOMPLETE with that reason.
