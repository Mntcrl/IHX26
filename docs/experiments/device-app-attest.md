# Device experiment — App Attest with free-team signing

Date: 2026-11-20 20:43–20:44 CET. Device: iPhone 13 Pro Max (`iPhone14,3`), iOS 27.2.1, CoreDevice id E34AE83F-D43C-510A-94EA-8E032BCD853E. App: `apps/capture-ios`, bundle id `utcq.XRPL-Galaxy`, team PPS57BF3X3 (personal team, 7-day "iOS Team Provisioning Profile: utcq.XRPL-Galaxy"), Debug configuration, Xcode 26.6 / iOS SDK 26.5, signed in the Mac's Aqua session by the build runner (ADR 0071).

## Procedure

`AppAttestProbe.swift` (DEBUG only, launched with `--app-attest-probe --diagnostic-exit`):

1. `DCAppAttestService.shared.isSupported`
2. `generateKey()` → key id (base64 of SHA256 of the uncompressed P-256 public key)
3. `attestKey(keyID, clientDataHash: SHA256(regClientData))`, where `regClientData` is the spec §3 preimage `"GALAXY/DEVICE-REG/v1\0" ‖ u32be(1) ‖ u32be(1234) ‖ 0xA5×32 ‖ reg_nonce[32] (SecRandomCopyBytes) ‖ key_id[32] ‖ 0xB6×20` (dummy ledger and account: not usable for a real DEVICE manifest)
4. `generateAssertion(keyID, clientDataHash: SHA256("GALAXY/ASSERT/v1\0" ‖ core))` twice, with core = 299 × 0x00 and 299 × 0x01

Commands (from the repo root, device lock held per command):

```
APP_ATTEST=off scripts/ios.sh install   # builds in the Aqua session (ADR 0071)
scripts/ios.sh logs --app-attest-probe --diagnostic-exit
scripts/ios.sh pull-fixtures appattest
```

## Results

### Without the App Attest entitlement (build `APP_ATTEST=off`, no `CODE_SIGN_ENTITLEMENTS`)

Console output:

```
GXATTEST isSupported=true
GXATTEST generateKey succeeded
GXATTEST attestKey succeeded (5830 bytes)
GXATTEST generateAssertion #1 succeeded (141 bytes)
GXATTEST generateAssertion #2 succeeded (142 bytes)
GXATTEST fixtures saved (dummy ledger, account and capture; not for certification)
The app terminated with the exit code 0.
```

No DCError was raised. Offline inspection of the pulled files (Python CBOR decoder + `cryptography`):

| item | observed |
|---|---|
| attestation `fmt` | `apple-appattest`, no trailing bytes |
| `authData` | 164 bytes, rpIdHash = SHA256(`PPS57BF3X3.utcq.XRPL-Galaxy`), flags 0x40, counter 0 |
| aaguid | `appattestdevelop` (development environment) |
| credentialId | 32 bytes, equals the key id and SHA256 of the credential certificate's uncompressed public key |
| `x5c` | 2 certificates (1048 B leaf, 583 B intermediate); leaf validity 2026-11-19 19:44 → 2026-11-22 19:44 UTC (≈ 3 days) |
| nonce extension | SHA256(authData ‖ clientDataHash) is present in the leaf certificate |
| `receipt` | 3972 bytes |
| assertion 1 / 2 | CBOR `{signature, authenticatorData}`; authenticatorData 37 bytes, same rpIdHash, counters 1 and 2; DER signatures 71 / 72 bytes |
| assertion signature | ES256 verifies over the message `nonce = SHA256(authenticatorData ‖ SHA256(clientData))` (ECDSA digest SHA256(nonce)), for both |

Without the entitlement iOS uses the development App Attest environment for a development-signed app; this is sufficient for every step of spec §11 (registration and capture), reported as environment "development".

### With `com.apple.developer.devicecheck.appattest-environment = development`

The build fails during provisioning (Aqua session, `xcodebuild … -allowProvisioningUpdates CODE_SIGN_ENTITLEMENTS=AppAttest.entitlements APP_ATTEST_ENVIRONMENT=development`, exit 65):

```
error: No Accounts: Add a new account in Accounts settings.
error: Provisioning profile "iOS Team Provisioning Profile: utcq.XRPL-Galaxy" doesn't include the App Attest capability.
error: Provisioning profile "iOS Team Provisioning Profile: utcq.XRPL-Galaxy" doesn't include the com.apple.developer.devicecheck.appattest-environment entitlement.
```

The Mac has no Xcode account, so the profile cannot be regenerated with the capability. This is not needed: the entitlement-free build already produces development-environment attestations. The production environment requires a paid team and a distribution-signed build (not available).

## Files (private, gitignored)

`fixtures/real/appattest/3AA97936-94B7-4137-B902-13D3DFDF9021/` (handed to Attestation2 for its real-device vector tests):

| file | sha256 |
|---|---|
| key-id.txt | 19d933e95379a3a75f1de01489f38d43a0d9c9c6aea7bddd4d2ff0d934ecae66 |
| registration-client-data.bin | 752f0ce224077e9bda618325b2ad5f345ee29fc0a3ec8b0153793f44f5c5cf69 |
| attestation.cbor | 8be5fdacf9e20fc6132d5f6311d99eea3b44a18fab626d3edc71b3defc718f52 |
| assertion-1-client-data.bin | 4d6c7be1abb65be9297abd59a2e917e5897d33e2a6cfd5a2053a23263d49fdcd |
| assertion-1.cbor | bc19ad4bdae4aa13d5f659cc4e1d519952cdf63f4e7bc1ff26c1ccad7d281e07 |
| assertion-2-client-data.bin | 6149ac59ff77805a1e39298d4ba995cd50f5762c5c3513ca28eec2884ed3610d |
| assertion-2.cbor | b4ff1e5fa2cfbfb5cf71ef0139e7ec327bc07818ba80d21934435197df3a5d74 |

The attestation contains Apple's fraud receipt and device-specific certificates; it is kept out of git. A published bundle strips the receipt (ADR 0060).

## Notes for the protocol

- The credential certificate is valid for about three days. Verifiers evaluate validity at the DEVICE manifest ledger close time (spec §11), so the device must publish the registration promptly after `attestKey`; later captures only need assertions.
- Each probe run generates a fresh key; key persistence across app reinstall was not tested. [INFERENCE from Apple's documentation] A key must be treated as lost when the app is deleted or the device restored; the app then registers a new key with a new DEVICE manifest.
