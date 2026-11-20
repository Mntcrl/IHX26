# XRPL Galaxy — Threat and trust model (v1)

Scope: protocol `galaxy/v1`, policy `galaxy-pc-v1m`, XRPL Testnet. Normative details: [spec](../protocol/galaxy-v1-spec.md). Decisions: [ADR 0006](../adr/0006-trust-model.md). Code-level review findings are recorded separately in `docs/security/review-*.md`.

What a positive result means: the photograph's canonical raster is bound to a CAPTURE published by a registered (App Attest) app instance, and a Groth16 proof shows that the committed sensor evidence, opened at positions chosen by a ledger after the commitment, satisfies the public physical-consistency policy. It does **not** prove that the scene is real, that the sensors were not fed synthetic data, or that the photo depicts what a viewer believes it depicts.

## 1. Trust sources and what each one gives

| Source | Gives | Does not give | How it is used |
|---|---|---|---|
| XRPL consensus | Total order, inclusion and timestamps (ledger close time) of validated transactions; immutability of published memos | Confidentiality; interpretation of Galaxy payloads; unbiased randomness | CAPTURE/PROOF ordering, deadlines, challenge ledger hash, append-only registry |
| Ledger data availability | Access to historical transactions | Guarantee that any given server keeps full history | Verification needs the ledgers L0…Lproof and the registration history; missing history → UNAVAILABLE, never VERIFIED |
| RPC endpoint | Convenient access to the ledger | Independent assurance: a remote server answering `validated: true` is trusted, not verified | Reports list endpoints and `rpc_trust: remote | local`; transaction content is integrity-checked by recomputing the tx hash from the blob; a locally operated rippled removes the dependency on a remote provider (it still trusts the validator list it is configured with) |
| Galaxy discovery nodes | Hints: photo_id → candidate captures, cached reports | Any influence on validity | Every hint is re-verified from XRPL; non-binding hints are discarded; nodes can only hide data (availability), not create validity |
| Groth16 (BN254) + setup | Succinct proof that a witness satisfies the circuit | Soundness if the setup's toxic waste is known | The verifying key is trusted only from explicit accepted-circuit configuration (circuit_id, vk_digest, policy_id, registration tx); the current key is `development-single-party` and therefore **not production-trusted** |
| Apple App Attest | Key bound to a genuine Apple device and to the app identity (team + bundle id), assertion counter | Integrity of the sensor data path at capture time; that the app was not instrumented at run time beyond Apple's checks; per-sample sensor authenticity | Registration (attestation) and per-capture assertions verified locally against the pinned Apple root; development and production environments are distinct classes; dev-unattested credentials never yield VERIFIED |
| Physical sensors (ARKit depth, poses, IMU) | Measurements as produced by Apple frameworks (sceneDepth is a fused estimate, not raw LiDAR) | Proof of physical provenance | The circuit checks consistency of committed data; Bluetooth, noise and meta aggregates are committed only |

## 2. Assets
- Integrity of the image ↔ evidence ↔ proof binding (photo_id, rgb_root, evidence_root, capture_id, challenge, proof).
- The photographer's XRPL wallet seed and App Attest key (device Keychain / Secure Enclave).
- Private evidence (frames, depth, poses, IMU, aggregates), salts and witnesses (device only).
- Verifier users' photos (never leave the browser) and their lookup privacy.
- Accepted-circuit configuration (the root of VK trust).

## 3. Threats, controls and residual risk

| Threat | Controls | Residual risk |
|---|---|---|
| Modified photograph (edit, crop, resize, recompression) | photo_id over the exact canonical raster; rgb_root recomputed from pixels; strict PNG/JPEG profiles | Lossless re-encodings that keep the raster verify by design |
| Metadata substitution (references to another capture) | XMP is untrusted; CAPTURE must bind photo_id and rgb_root of the presented image | None for validity; wrong references only cause INVALID/NOT_FOUND |
| Reused or substituted LiDAR/RGB (commitments to unrelated data) | All evidence committed before the challenge; Merkle openings in-circuit; RGB–depth edge check; temporal reprojection; camera/IMU in evidence_root | **Measured weakness:** on synthetic scenes, depth from another scene with the same RGB passes v1m in 16/20 seeds ([report](../experiments/synthetic-v1m.md)); v1m does not reliably detect this substitution |
| Screen / print / projection recapture | Global and central planarity limits, distributed non-planar evidence, RGB–depth correspondence, parallax | Synthetic: flat/tilted/curved screens and prints rejected 20/20; a centre screen surrounded by real objects passed 2/20; physical-device experiments still pending |
| Synthetic but internally coherent sensor data (compromised acquisition or device) | App Attest limits the code identity; capture flow commits before the challenge | **Not prevented:** a fully synthetic, coherent dataset fed into the pipeline can satisfy the proof |
| Unconstrained or under-constrained circuit | Constraint-level negative tests; range checks on every private input; code review | Residual until an independent audit; any soundness bug can make invalid evidence provable |
| Malicious verification key / permissive circuit published on XRPL | VK accepted only from explicit configuration with vk_digest; circuit registrations are not trusted by themselves | A user who adds an untrusted circuit to their config accepts its policy |
| Groth16 setup compromise | Production requires a multi-party phase-2 ceremony with verifiable contributions | Current development key: whoever ran the setup could forge proofs |
| Proof replay / swap | Public inputs reconstructed from CAPTURE + selection ledger; PROOF bound to manifest tx, selection ledger and circuit; same account; window Lsel < Lproof ≤ Lsel + w_proof; first valid proof wins | None known |
| Assertion replay / credential misuse | Assertion over core299 (incl. nonce, network, account, Lpre); counter strictly increasing per registration in ledger order; capture_id uniqueness; revocation | Historical checks need the full account history of the registration |
| Network confusion (Testnet vs Mainnet) | network_id inside CAPTURE/PROOF and checked against server_info; mainnet refused unless allowed | None known |
| Ledger sampling manipulation / grinding | Challenge from a single prescribed ledger (L0 + 5), no retries | Ledger hashes are not an unbiased beacon (validator influence); a prover can abandon unfavourable captures (selective abort) and try again with a new capture |
| Malicious RPC endpoint | Tx hash recomputed from the returned blob (content integrity); network_id checked; multiple endpoints configurable; local rippled supported | A remote endpoint can lie about `validated`, metadata (TransactionResult, TransactionIndex), ledger hashes and close times, or hide history; v1 does not verify validator signatures over ledger headers, so this is mitigated only by using one's own rippled or several independent endpoints |
| Malicious / Sybil discovery nodes | No quorum; every hint re-verified; non-binding hints discarded; SSRF guard, timeouts, size and rate limits | Nodes can hide captures (availability, censorship) and observe queried photo_ids |
| Oversized / malformed payloads and images | Strict bounded decoders, dimension checks before allocation, bundle and response caps, fuzz-style tests | Residual until fuzzing coverage is broader |
| Unavailable historical data | UNAVAILABLE state with consulted ranges | Long-term verification depends on independent XRPL archives |

## 4. Privacy
- Never published: images, frames, depth, poses, IMU samples, Bluetooth observations, salts, witnesses.
- Public and correlatable by anyone: photo_id (anyone holding the photo can test membership), capture time (ledger), wallet address, App Attest key id and registration, device model class if included in public material, circuit/policy ids.
- Bluetooth: foreground scan only, session-random HMAC dedup, identifiers discarded after aggregation; the aggregate is a commitment, not a proof of nearby devices.
- Browser verification: decoding and verification run locally; XRPL endpoints learn which transaction hashes are queried; nodes learn photo_ids only after explicit consent.

## 5. Operational security notes
- Seeds: iOS Keychain (this-device-only) / `~/.config/galaxy/` for development wallets; never in git, logs or memos.
- Test device data: see the [2026-11-20 incident](../incidents/2026-11-20-device-container-deletion.md) and spec §19 device data safety rules.
