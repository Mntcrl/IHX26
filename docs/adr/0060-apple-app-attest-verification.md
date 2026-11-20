# ADR 0060 — Verify the Apple App Attest wire profile

## Context

Galaxy publishes its own ledger-bound App Attest challenges and needs identical local verification on native, iOS and WASM targets. Apple's certificates and authenticators use a narrower profile than general PKIX and WebAuthn. In particular, the App Attest CA signs P-256 credentials using P-384 with SHA-256; the Apple assertion API signs the computed nonce as a message, not as an ECDSA digest. Actual Apple vectors exhibit legacy flag conventions and optionally append a signed extensions map.

## Decision

Use RustCrypto, `x509-cert`, `der` and `ciborium` to check the two-certificate App Attest chain against Apple's fingerprint-pinned root at the validated DEVICE manifest ledger close time. Require strict DER and structured Apple's authenticator profile, including the documented extension schema when present. Verify assertions as ES256 over `nonce`, with no alternate digest path. Strip the Apple fraud receipt before publication; use the verified XRPL history, not a project server, for replay checks. Keep Apple development, Apple production and dev-unattested results distinct.

## Consequences

No Apple or project verification endpoint is needed; Apple and eligible provisioning remain prerequisites at enrollment. A changed Apple root, certificate profile, or authenticator schema will fail closed and require an explicit update. The receipt is not disclosed, but the public key ID and wallet correlate captures. A dev-unattested signature cannot upgrade a verdict to `VERIFIED`, and App Attest never authenticates sensor data or proves scene truth. See [the protocol and interoperability evidence](../protocol/app-attest.md).

A real development attestation and two assertions from our test device confirm the profile and the `SHA256(nonce)` digest. Development credential certificates are valid for only three days, so the DEVICE bundle and manifest must be published right after `attestKey`; captures remain verifiable afterwards because they use the registered key.
