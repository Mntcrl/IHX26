# ADR 0070 — App Attest in the development environment without the entitlement

Status: accepted (2026-11-20). Evidence: [device experiment](../experiments/device-app-attest.md).

## Context
- Spec §11 needs `DCAppAttestService` key generation, attestation and assertions on the iPhone; the project only has a personal (free) team with 7-day profiles and no Xcode account on the Mac build host.
- Adding `com.apple.developer.devicecheck.appattest-environment` fails provisioning: the team profile lacks the App Attest capability and the entitlement, and "No Accounts" prevents regenerating it (exit 65).

## Decision
- The capture app is built without any App Attest entitlement. Measured on the device (iOS 27.2.1): `isSupported` = true, `generateKey`, `attestKey` and two `generateAssertion` calls succeed; the attestation's aaguid is `appattestdevelop`, counters 0 → 1 → 2, and the assertion signatures verify as ES256 over `nonce`.
- Galaxy registrations from this build are labelled environment "development"; verifiers accept them per configuration (spec §11). Production App Attest is out of reach for this team and is not claimed.
- `AppAttest.entitlements` and the `capture-development|capture-production` runner modes stay available for a paid team; they are not used by default.
- The Swift side computes only the spec §3 preimages and calls Apple's API (`GalaxyAppAttest.swift`); parsing and verification belong to galaxy-attestation (ADR 0060).

## Consequences
- Real Apple attestations are available for registration, so criterion 4 does not depend on the dev-unattested credential; results are reported as development-environment App Attest.
- The credential certificate is valid for about three days; the app must publish the DEVICE manifest soon after `attestKey`.
- Re-signing with a different team or bundle id changes `app_id` (rpIdHash) and requires a new registration.
