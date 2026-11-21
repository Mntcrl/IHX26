# GalaxyCore: iOS capture integration

`galaxy-ffi` is the capture app's only Rust static library. It wraps the shared evidence, protocol, geometry, witness, prover, image, App Attest codecs and XRPL mobile APIs. Networking stays in Swift (`URLSession` to the configured XRPL Testnet endpoint); App Attest uses `DCAppAttestService`. No camera data or witness is sent to either service.

## Build and trust pins

```sh
scripts/ios-rust-lib.sh
# Inputs may be selected explicitly (paths relative to the repository root or absolute):
# GALAXY_IOS_POLICY=policies/galaxy-pc-v1m.json
# GALAXY_IOS_NETWORK=deployments/networks/xrpl-testnet.toml
# GALAXY_IOS_DESCRIPTOR=circuits/artifacts/v1m/descriptor.json
```

The script validates the canonical descriptor against the accepted CIRCUIT entries in the compiled-in network TOML and the canonical hash of the compiled-in policy JSON. It fails for a missing or unmatched entry, mismatched policy or VK digest, unsupported layout or disallowed ceremony. Rebuild after any policy/config update; do not change numeric thresholds in Swift or use a descriptor found in an untrusted response. `development-single-party` is accepted on development networks only when explicitly pinned. On Testnet it can produce `CERTIFIED`; display **Development setup (Testnet)**, not a claim of production security. See [ADR 0043](../adr/0043-groth16-setup-ceremony.md).

Outputs in `bindings/swift/GalaxyCore/`: `ios-arm64/libgalaxy_ffi.a` (ignored), `swift/galaxy_ffi.swift`, `include/galaxy_ffiFFI.h`, `include/module.modulemap`, and `BUILD-INFO.txt` (toolchain, git revision, SHA-256 hashes of selected inputs and outputs). The descriptor is a separate app resource: use exactly the checked `GALAXY_IOS_DESCRIPTOR`. The graph and zkey can be app resources or immutable files in Application Support; they must be the pair for that descriptor. `openProver` hashes both mapped artifacts with `Prover::open_pinned`, checks the VK digest and actual public-input count, and refuses mismatches. Never mutate, replace in place or delete a mapped file while a prover is alive.

Use a new versioned directory for artifact updates. Finish or explicitly end in-flight captures before replacing the app with one that embeds a different policy: keeping old graph and zkey files does not authorize the old descriptor under the new policy. Never reinterpret an old core under new pins.

## Xcode cutover when the Mac is available

1. Sync `bindings/swift/GalaxyCore/` and the matched descriptor, graph and zkey into the project build mirror (spec §19, never the user's original workspace). Check disk space; avoid multiple copies of the zkey in DerivedData and old mirrors. Add `swift/galaxy_ffi.swift` to the capture target. Set `SWIFT_INCLUDE_PATHS` to `…/GalaxyCore/include`, `LIBRARY_SEARCH_PATHS` to `…/GalaxyCore/ios-arm64`, and `OTHER_LDFLAGS = $(inherited) -lgalaxy_ffi`. This library is for physical arm64 iOS; do not link `libgalaxy_prover.a` or include the bench Swift source. Import only Foundation and Apple frameworks from handwritten Swift; the generated Swift imports `galaxy_ffiFFI`.
2. At setup, call `CaptureCircuit.configure(descriptor:)` with the resource bytes, obtain the approved endpoint, network/fee settings and CIRCUIT registration tx via `circuit.network()`, and show `circuit.ceremony()` in the certification UI. Run `openProver(graphPath:zkeyPath:)` off the main actor; require it to succeed before enabling on-ledger capture. Do not treat policy limits as a policy verdict. Use those limits only for challenge-independent tracking/depth/motion checks; never perform selection-dependent screening before Lsel.
3. Keep camera/UI work on its existing actor. Run ingest/restore, proof artifact opening, geometry/witness, prove, verify and image packaging on a dedicated worker (not `MainActor`). UniFFI functions are synchronous. `CaptureProgress` callbacks arrive on the proving thread; send only phase name/time to the UI. A `ProvingCancellation` can be set from the UI; cancellation takes effect between prover phases/MSM chunks. Do not start a second prover on memory pressure or while the first is unwinding.
4. Perform the signed Xcode/device run in the existing Aqua-session workflow, install in place with the original bundle ID, and use the §19 device lock and backups. Keep bench and capture installation windows disjoint. The Linux build does not prove that the Swift target compiles, signs, or runs on the device.

Move the production capture gate in `Acquisition` off its phase-1 literal:
compare the minimum of the measured t−/t+ distances with
`Double(circuit.policyLimits().baselineMinUm) / 1_000_000` in meters.
Keep the sensor/tracking and depth-coverage checks independent of sampled tiles.
Do not use preview motion or a synthetic challenge as the policy verdict.

## One-time setup: wallet and device

- Generate **16 bytes** using `SecRandomCopyBytes` and check its return status. Pass them to `walletIdentity(entropy:)`; store the same 16 bytes once via `WalletSeedStore.store`, and display only `classicAddress`. The Rust API never returns `sEd…`. Pass entropy from the Keychain to signing only; never store it in the capture journal. Show reserves and fee caps. Funding is an explicit Testnet faucet action supplying the address only.
- Generate the App Attest key with `GalaxyAppAttest.generateKey`; persist the original key ID string and use its exact 32-byte base64 decode in the protocol. Read a validated ledger with `server_info` (which checks the network; obtains the latest validated index) then request the header by that exact index. Read a fresh 32-byte nonce. Use `deviceRegistrationClientData` and pass the **prehashed** output directly to `DCAppAttestService`; don't call the Phase 1 wrapper that hashes its input. Use the app ID and actual development/production entitlement/profile environment; don't guess or silently fallback.

```swift
let challenge = RegistrationChallenge(network: setup.xrpl.networkId,
    ledger: header, nonce: nonce32, keyId: keyId32, accountId: wallet.accountId)
let reg = try deviceRegistrationClientData(input: challenge)
let attestation = try await DCAppAttestService.shared.attestKey(keyIdentifier,
    clientDataHash: reg.clientDataHash)
let publication = try prepareDevicePublication(input: challenge, appId: appId,
    environment: .development, attestation: attestation) // choose actual environment
```

`prepareDevicePublication` strips the Apple receipt and returns the GXDB bundle, its ID, ordered chunk memos and completion deadline. Keep the registration journal through interruption: the key ID, exact ledger/nonce, bundle, per-chunk signed records and inclusion ledgers. Submit the chunks **serially**, using the same seed and validated `tesSUCCESS` for each, then pass their inclusion indices in chunk order to `buildDeviceManifest(bundle:chunkLedgers:)`. Use `publication.deadline` as the `xrplAutofill` cap for every tx of that registration. Save the DEVICE tx hash only after its validated `tesSUCCESS`; this differs from the CIRCUIT registration tx. If the window expires, end that registration attempt; never reuse its attestation under a new ledger or nonce.

## Transaction helper, on Swift's network actor

For each signed purpose (one CAPTURE and at most one PROOF per capture):

1. `xrplServerInfoRequest()` → `xrplLatestValidated(network:body:)`; `xrplLedgerRequest(index:)` → `xrplParseValidatedLedger(index:body:)` for a header when needed. Cap HTTP responses at 1 MiB and reject non-success HTTP status or wrong encoding before passing text to Rust. No logging of response bodies, tx blobs, app key identifiers or seeds. Use the same configured endpoint/chain for all calls.
2. `xrplFeeRequest()` and `xrplAccountInfoRequest(classicAddress:validatedLedger:)` at the validated index → `xrplAutofill` with the applicable deadline. Use one active serial wallet writer, including across setup/capture. `xrplSignAccountSet` returns the exact signed bytes, hash, purpose and ledger bounds; write the whole record atomically under `FileProtectionType.complete`, excluded from backup, **before** submit. Do not auto-sign a different payload if the outcome is ambiguous.
3. Build requests with `xrplSubmitRequest`; parse with `xrplParseSubmit` for provisional diagnostics. Treat neither `tesSUCCESS` nor `terQUEUED` as inclusion. Poll using `xrplTxRequest` and `xrplParseTx` with the same latest validated index used to build each request. `.validated` with `success: true` is the only success. `.validated` with `success: false` is terminal (may claim a fee); `.expired` means the full search range was covered. A history gap or RPC/network error is not expiry. After interruption, query the stored hash first. Resubmit only the exact stored blob while in its valid window. No retries with a new CAPTURE/PROOF or new challenge.

For JSON-RPC, POST the Rust request string as UTF-8 with `Content-Type: application/json` via `URLSession`; decode each body as strict UTF-8. Keep the transaction journal in the capture UUID directory, not alongside exported images. Store signed records (including blob as `Data`) in a Swift Codable journal record and lift the same values back into the generated `SignedTransaction` record when resuming.

## Capture, challenge, certify

Map these steps to the existing `CaptureRecord` state machine. Every transition should atomically record the exact inputs necessary to resume that transition; use the existing protected-file primitives inside the app's own UUID directory.

1. **Before ACQUIRING:** read the latest validated ledger and its exact header just before the shutter, store it as Lpre. Freeze the existing GXRW v1 from `Acquisition`; gate only on challenge-independent checks. `ingestGxrw(raw:profile:captureState:)` creates salts and commitments together; save `evidence.encodeBundle()` once, before requesting an assertion, and restore with `decodeEvidenceBundle(bytes:)`. Use `captureState: Data()` if journal metadata is stored separately. `captureState` is opaque immutable data in GXEV; do not rewrite the evidence bundle to update transaction state.
2. **ATTESTING:** build the statement once from the persisted evidence. Keep the exact core, capture ID and deadline in the journal. Feed `assertionClientDataHash` directly to DeviceCheck, then build the CAPTURE memo. Use `.pngV1` and flags `0` or `.jpegV1` and flags `1`; do not rotate or re-encode the committed raster. Save the assertion with the core before scheduling signing. If interrupted before signing, resume the same core; if an assertion must be fetched again, only the persisted signed CAPTURE may ever be submitted.

```swift
let input = CaptureParameters(network: setup.xrpl.networkId, registrationTx: deviceTx,
    preledger: lpre, nonce: captureNonce32, claimedTimeMs: shutterMs,
    flags: 0, accountId: wallet.accountId)
let statement = try circuit.buildCaptureStatement(evidence: evidence, input: input)
// Atomically persist statement.core, statement.captureId and statement.captureDeadline.
let assertion = try await DCAppAttestService.shared.generateAssertion(keyIdentifier,
    clientDataHash: statement.assertionClientDataHash)
let memo = try buildCaptureMemo(core: statement.core, assertion: assertion)
```

3. **SUBMITTING_MANIFEST through WAITING_SELECTION_LEDGER:** sign and journal the CAPTURE with `LastLedgerSequence ≤ statement.captureDeadline`. After validated `tesSUCCESS` record L0 and the exact CAPTURE hash. `circuit.selectionLedgerIndex(core:captureLedger:)` checks the window and returns Lsel; wait for that exact ledger to validate. Persist its hash, not the latest validated hash at the time a task resumes. Reconstruct with `deriveChallenge(core:captureTx:captureLedger:selectionLedger:)`. There is no new challenge for this CAPTURE, including after an inconclusive result or app termination.
4. **GENERATING_WITNESS onward:** if the reference verdict fails, save terminal `INCONCLUSIVE` with the failed predicate names (not raw statistics), and send no PROOF. An opaque witness object keeps the private JSON in Rust. Prove from that witness and the pinned prover. The prover compares actual public signals against the reconstructed CAPTURE/Lsel statement and verifies locally. `buildProofMemo` performs the local verification against that statement before returning the memo. Before a signed PROOF exists, an interrupted worker may recompute on the same evidence/challenge; after signing, reuse the journaled tx.

```swift
let challenge = try circuit.deriveChallenge(core: core, captureTx: captureTx,
    captureLedger: l0, selectionLedger: lselHeader)
let evaluation = try challenge.evaluate(evidence: evidence)
if evaluation.verdict().pass {
    let witness = try evaluation.buildWitness()
    let out = try prover.prove(witness: witness, progress: listener, cancellation: cancellation)
    let proofMemo = try prover.buildProofMemo(challenge: challenge, proof: out.proof)
    // Persist proof/memo before signing; deadline is challenge.info().proofDeadline.
} else {
    // Persist INCONCLUSIVE and stop; display the failed policy predicates.
}
```

5. **WAITING_PROOF_VALIDATION to CERTIFIED:** only after the *matching* signed PROOF confirms validated `tesSUCCESS`, with `Lsel < Lproof ≤ proofDeadline`, call `challenge.exportImage(evidence:proofTx:)` using its hash. Rust packages the exact profile bytes with XMP; references are not a receipt and `exportImage` does not perform XRPL lookup. Atomically record the certification and resulting file. Export the file bytes to Files or as a photo resource (not a `UIImage` re-encode); verify the exported bytes through the independent Rust verifier. A key, resource, or RPC failure cannot be labelled `CERTIFIED`. Capture consent and sensor retention follow spec §§19–21; never delete existing user/device files as part of a migration/test install.

All recoverable Rust errors use stable `GalaxyError` cases; use the case, not localized string matching. Surface no downstream errors, private evidence, raw bundle/witness bytes, seeds, salts, or raw ledger responses in logs or crash breadcrumbs. Swift owns durable storage and the ordering of externally observed events; Rust verifies format, windows and bindings from those observations, not endpoint honesty.

## Gallery previews and certified exports

- Use `try canonicalImageFromGxrw(raw: raw, profile: profile)` for a local calibration
  preview. This is the exact encoded, XMP-free canonical image in the selected
  `CaptureProfile`. Ingest still validates the GXRW structure and evidence, but it
  does not generate salts, compute commitments, apply the challenge-dependent
  policy, or require certification state. Do not label a preview certified.
- `certifiedPhotoDetails(file:)` checks that the raster has the photo ID named by Galaxy XMP and
  returns `photoId` (lowercase hex), the committed `imageProfile`, and `captureTxUrl` / `proofTxUrl`
  derived from the bundled network's HTTPS explorer template. Use these details even after raw
  evidence has been removed. The call checks local binding, not the ledger or proof.
- Call `certifiedFileWithInfo(file:info:)` on an already-certified `png-v1` or `jpeg-v1` export.
  It writes one XMP packet with the same Galaxy references, §4 `galaxyinfo` annotations, and
  `xmp:CreatorTool`. It does not re-encode PNG IDAT chunks or the JPEG scan. Include only public
  capture-journal values; do not consult existing informational XMP for truth. Omit unknown
  values instead of guessing. The verifier ignores the annotations and its report supersedes them.

`ExportInfo` fields and Swift types:

| Fields | Type and encoding |
|---|---|
| `account` | `String?`, XRPL classic address (checksum verified) |
| `registrationTx` | `String?`, DEVICE registration hash, 64 uppercase hex digits |
| `captureLedger`, `selectionLedger`, `proofLedger` | `UInt32?`, written in decimal |
| `captureTime`, `claimedTime` | `String?`, ISO-8601 UTC ending `Z`, fractional seconds allowed. `captureTime` is the L0 close time and also supplies `xmp:CreateDate` |
| `circuitId`, `policyId` | `String?`, 64 lowercase hex digits |
| `policyName`, `setupCeremony` | `String?`, public policy name and ceremony label |
| `attestationEnvironment` | `String?`, `development`, `production`, or `dev-unattested` |
| `captureTxUrl`, `proofTxUrl` | `String?`, `nil` derives the link automatically. A supplied value must exactly match the bundled HTTPS template filled with the corresponding Galaxy XMP transaction hash |
| `appVersion` | `String`, required; `xmp:CreatorTool` is `XRPL Galaxy <appVersion>` |

For a `jpeg-v1` capture, pass the enriched JPEG to `losslessPngFromCertifiedJpeg(file:)`
if the user requests PNG. It reference-decodes the original JPEG, losslessly encodes those
pixels as `png-v1`, and embeds the complete unchanged packet. The PNG keeps the JPEG capture's
`imageProfile`, `photoId`, and `rgb_root`. A generic JPEG encode of a `png-v1` capture is not a
certified export. Pass the returned bytes as a file/photo resource, not through `UIImage`
or an image editor. Invalid XMP, changed pixels, overlong metadata (64 KiB overall, subject to
JPEG APP1's smaller maximum), or mismatched explorer URLs fail rather than modifying the input.

## Linux checks

```sh
scripts/with-slot.sh cargo test -p galaxy-ffi
scripts/with-slot.sh cargo clippy -p galaxy-ffi --all-targets --features uniffi-cli -- -D warnings
scripts/with-slot.sh --heavy cargo test --release -p galaxy-ffi \
  accepted_main_v1m_proves_the_real_witness -- --ignored --nocapture
```

The ordinary suite covers synthetic captures, local App Attest/XRPL vectors,
and a private GXRW round trip (skipped if no local calibration fixture exists).
The ignored proof test needs the same accepted build-time policy/network pins
and `descriptor.json`, `main_v1m.graph.bin`, `main_v1m.zkey` in
`circuits/artifacts/v1m/` (or `GALAXY_IOS_TEST_ARTIFACTS`).

Export checks use committed PNG verifier recordings, reference JPEG vectors and an opt-in
JPEG ledger replay. With the matching private Testnet JPEG already in `fixtures/real/`, run:

```sh
GALAXY_EXPORT_REPLAY_JPEG="$PWD/fixtures/real/<private jpeg-v1 export>" \
  scripts/with-slot.sh cargo test --release -p galaxy-ffi --test exports \
  recorded_jpeg_and_lossless_png_bind_to_same_capture -- --ignored
```

The replay uses the committed `development-4ad25185-account-txn-id` ledger recording.
The device image is not part of the recording and must never be committed.

## Checkpoints on the Mac and phone

- Build with the actual accepted descriptor/policy on the mirror; test Keychain lock/unlock, file protection and backup exclusion. Confirm the binary links one Rust library, the XMP bytes survive export, and the network trace has only XRPL and the platform App Attest call.
- Test interruption before first submit, after validated CAPTURE, while waiting for Lsel, during proving, and after PROOF submission; check that the tx hash/ledger selection stays identical. Test an unaccepted descriptor, missing artifact, fee-cap failure, expired tx, and a real inconclusive capture. Keep failures distinct from verdicts.
- Perform a real handheld positive capture under the pinned policy and check both txs in an independent Testnet verifier. Record capture-to-export phase times, device peak memory, archive and app size, tx sizes/fees, and the number of registration/per-photo txs. Linux proof tests and archive generation are not device measurements.
