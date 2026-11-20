# Verification (galaxy-verifier)

Normative sources: spec §7, §9–§13, §17. This document describes what the shared
Rust verifier (`crates/galaxy-verifier`) does, the stable check identifiers it
reports, how states are decided and what each external party is trusted for. The
same library runs in the `galaxy` CLI, the verifier node (proof-only mode), the
browser (WASM) and iOS.

## Entry points

| function | mode | input |
|---|---|---|
| `verify_photo(image, None, ctx)` | `direct` | references from the image's Galaxy XMP (untrusted hints) |
| `verify_photo(image, Some(refs), ctx)` / `verify_manifest(Some(image), tx, ctx)` | `manual` | CAPTURE hash (+ optional PROOF hint) supplied by the caller |
| `verify_by_photo_id(image, nodes, ctx)` | `lookup` | CAPTURE hints from one or more untrusted lookup nodes |
| `verify_proof_only(tx, ctx)` / `verify_manifest(None, tx, ctx)` | `proof-only` | CAPTURE hash, no image (nodes, `galaxy inspect`) |
| `verify_proof_transaction(proof_tx, ctx)` | `proof-only` | one exact PROOF transaction (`galaxy proof verify`) |
| `inspect_circuit(circuit_id, ctx)` | `circuit-inspect` | accepted circuit id |
| `photo_identity(image)` | — | local photo_id / rgb_root, no network |

`VerifyContext` carries the `LedgerSource` (galaxy-xrpl), the `NetworkConfig`
(trust anchors), the RPC endpoint labels and trust (`remote` | `local`) and an
optional local circuit bundle (GXCB bytes) used instead of the on-ledger copy when
its digests match the config.

## Algorithm (one capture)

1. **Image** — strict container decode (PNG / baseline JPEG profile, §4) to the
   canonical 1024×768 RGB8 raster; `photo_id = SHA256(...)` (§3) and the Poseidon
   `rgb_root` are recomputed from the presented pixels. Galaxy XMP is parsed but
   only used as a hint.
2. **Ledger** — `server_info`: the endpoint must serve the configured NetworkID
   and report a validated ledger (this is the evaluation tip, reported as
   `trust.latest_validated`).
3. **CAPTURE** — fetched by hash; the returned blob must hash to the requested
   id. Strict `galaxy/v1/capture` decoding. With an image, `photo_id` and
   `rgb_root` must equal the presented raster's (else INVALID in direct/manual
   mode). Validated + `tesSUCCESS` required (not validated → PENDING, failed →
   INVALID). Network (memo field, `NetworkID` presence rule §12, inception ledger)
   and account (AccountSet sender = core account) are bound.
4. **Circuit** — `circuit_id` must be in `NetworkConfig.accepted_circuits` with the
   CAPTURE's `policy_id`, else UNSUPPORTED. The GXCB bundle is reassembled from the
   configured CIRCUIT registration (or taken from a local copy); descriptor, policy
   and VK digests must equal the config entry; the descriptor must bind the
   v1 public layout (22 inputs), policy id, VK digest and name a ceremony; the
   policy must validate. A VK published on XRPL is never trusted by itself.
5. **Registration** — DEVICE manifest + chunks (§10 retrieval rules), manifest
   earlier than the CAPTURE and from the same account, registration ledger hash
   equal to the validated ledger's hash, then `verify_registration` (App Attest
   chain to the pinned Apple root at the manifest ledger close time, app ID and
   environment accepted by the config). A dev-unattested credential is accepted
   for key possession only and labelled "development unattested credential".
6. **Assertion** — `verify_capture_assertion` over the exact 299-byte core
   (App Attest ES256, or the dev Ed25519 signature).
7. **Account history** — one bounded `account_tx` scan of the registration
   account from the DEVICE manifest ledger to the evaluation tip (pages of 200,
   at most 64 pages; the RPC must claim complete history for the range):
   earlier CAPTUREs of the same registration whose assertions verify define the
   counter history; duplicates of the capture_id; every REVOKE; PROOF candidates
   that name this CAPTURE.
8. **Uniqueness, counter, revocation** — an earlier CAPTURE with the same
   capture_id → INVALID. App Attest counter must exceed every earlier
   authenticated counter (dev credentials have none: not applicable). REVOKE
   rules §10: effective ledger 0 = revoke ledger; backdating allowed;
   future-dated or out-of-range revokes are ignored and reported; L0 ≥ effective
   → INVALID. The report states "credential state as of ledger N".
9. **Windows** — `Lpre < L0 ≤ Lpre + w_cap`, Lpre hash equals the validated
   ledger's hash; `Lsel = L0 + sel_offset` must be validated (else PENDING).
10. **Challenge** — seed, 16 sampled tiles and the 22 public inputs are recomputed
    (`galaxy_crypto::sampling`, `public_inputs_v1`) from the CAPTURE, its
    capture_id and the Lsel hash.
11. **PROOF selection** — candidates are the PROOFs found in the account history
    (ledger order). A PROOF hint from XMP, the caller or a node is evaluated and
    listed but can never select itself. Each candidate: validated + `tesSUCCESS`,
    sent by the CAPTURE account, references this CAPTURE, ledger in
    `(Lsel, Lsel + w_proof]`, memo network / Lsel / Lsel hash / circuit equal to
    the CAPTURE challenge, Groth16 proof valid for the recomputed public inputs
    under the pinned VK. The first valid one in ledger order wins; evaluation
    stops there. Candidates outside the window or from other accounts are
    *rejected* (listed, no effect); an in-window PROOF from the capture account
    that fails binding or Groth16 is *invalid*.
12. **Outcome** — a valid PROOF → VERIFIED (all five `policy.*` checks pass,
    `proven_by: zk-circuit`). No valid PROOF: an invalid one with the window
    closed → INVALID; window still open → PENDING; nothing usable and window
    closed → INCONCLUSIVE.
13. **Credential class** — a dev-unattested credential turns VERIFIED, PENDING
    and INCONCLUSIVE into UNSUPPORTED (summary keeps "Proof status X"); INVALID
    stays INVALID.

Proof-only modes skip image steps (checks `not_applicable`) and add the
limitation "Image binding not checked".

## Check identifiers

Report order; `proven_by` is fixed per check.

| id | proven_by | meaning |
|---|---|---|
| image.decode | cryptographic | container decoded to the canonical raster |
| image.profile | cryptographic | supported container / pixel profile |
| image.photo_id | cryptographic | photo_id recomputed from presented pixels |
| image.rgb_root | cryptographic | Poseidon rgb_root recomputed |
| refs.metadata | cryptographic | Galaxy XMP found (hints only) |
| ledger.network | ledger-rpc | RPC serves the configured network; tip known |
| capture.found | ledger-rpc | CAPTURE retrieved, blob hash = id |
| capture.validated_success | ledger-rpc | validated, tesSUCCESS |
| capture.decode | cryptographic | strict v1 CAPTURE decode |
| capture.network | cryptographic | network fields and inception |
| capture.photo_binding | cryptographic | CAPTURE photo_id = image photo_id |
| capture.rgb_binding | cryptographic | CAPTURE rgb_root = image rgb_root |
| capture.account_binding | cryptographic | sender = core account = registration account |
| circuit.accepted | cryptographic | circuit + policy pinned in config |
| circuit.registration | cryptographic | bundle digests match the config |
| registration.resolve | ledger-rpc | DEVICE manifest found, earlier, same account, reg ledger hash |
| registration.bundle | cryptographic | bundle reassembled and matches manifest |
| registration.attestation | attestation | App Attest registration (dev credential: fail, labelled) |
| capture.assertion | attestation | assertion over the CAPTURE core |
| capture.counter | attestation | counter strictly increasing |
| capture.unique | ledger-rpc | capture_id not published earlier |
| registration.revocation | ledger-rpc | no applicable REVOKE up to the tip |
| ledger.preledger_window | ledger-rpc | Lpre hash and capture window |
| ledger.selection | ledger-rpc | Lsel validated |
| ledger.challenge | cryptographic | seed, tiles, public inputs recomputed |
| proof.found | ledger-rpc | a PROOF exists for the capture |
| proof.validated_success | ledger-rpc | PROOF validated, tesSUCCESS |
| proof.account | cryptographic | PROOF sent by the CAPTURE account |
| proof.binding | cryptographic | PROOF binds CAPTURE, Lsel, Lsel hash, circuit |
| proof.window | ledger-rpc | PROOF ledger within the window |
| proof.groth16 | cryptographic | Groth16 pairing check |
| policy.sensor_quality, policy.lidar_geometry, policy.rgb_lidar, policy.temporal, policy.motion_imu | zk-circuit | pass exactly when proof.groth16 passes for an accepted circuit |
| evidence.bluetooth, evidence.noise | committed-only | always `not_applicable`: committed, never verified |

Statuses: `pass`, `fail`, `unavailable`, `not_applicable`, `skipped` (not
evaluated because an earlier step decided the state). Per-candidate checks are
listed under `candidates[]`.

## States

| state | exit | meaning |
|---|---|---|
| VERIFIED | 0 | valid PROOF from an accepted circuit, App Attest credential |
| PENDING | 10 | CAPTURE or PROOF not validated yet, or Lsel/PROOF window still open |
| INVALID | 20 | a hash-bound record contradicts the claim (binding, signature, window, revocation, replay, Groth16) |
| INCONCLUSIVE | 30 | valid capture, window closed, no accepted proof |
| NOT_FOUND | 40 | no capture bound to this image in the consulted, complete ranges / nodes — never "fake" |
| UNAVAILABLE | 50 | RPC/node/history missing or rejected; nothing established |
| UNSUPPORTED | 60 | unknown circuit, non-accepted app ID/environment, unsupported image, dev-unattested credential |

Lookup aggregation (§13): hints whose CAPTURE does not bind to the image are
listed as rejected hints (NOT_FOUND), never INVALID. Among binding candidates:
VERIFIED > PENDING > UNSUPPORTED > INCONCLUSIVE > INVALID; INVALID only if every
binding candidate is invalid. An incomplete lookup (node error, gaps, caps,
unavailable candidate) yields UNAVAILABLE unless a state above INVALID was
established. A complete lookup with no binding candidate yields NOT_FOUND with
the consulted ranges and nodes.

## Report (`galaxy-report/v1`)

`version, state, summary, network, mode, image{format, profile, photo_id,
rgb_root, xmp}, capture{manifest_tx, L0, Lpre, capture_id, account,
registration_tx, claimed_time_ms, ledger_close_time, attestation, app_id,
counter}, proof{proof_tx, Lsel, Lproof, deadline, selection_hash, circuit_id,
policy_id, policy_name, ceremony, seed, sampled_tiles}, checks[], candidates[],
limitations[], trust{rpc_endpoints, rpc_trust, lookup_endpoints,
ledger_ranges[{purpose, min, max}], latest_validated}`. Success summary:
"Physical consistency verified under Galaxy policy <name> (<policy_id>)."
Limitations are always included.

## Trust boundaries

- **XRPL RPC** — trusted for validation status, transaction results, ledger
  order, ledger hashes/close times and history completeness (`complete_ledgers`).
  Not trusted for content: every blob is hashed and decoded locally; a blob that
  does not hash to the requested id makes the answer unusable (UNAVAILABLE).
  Use a local synchronized rippled (`rpc_trust: local`) to remove this trust.
- **Lookup nodes** — trusted for nothing. They can omit records (absence is only
  reported for the ranges they claim) and observe photo-id queries. `galaxy verify
  --no-discovery` never contacts a node.
- **App Attest** — establishes that the credential key lives in a genuine Apple
  app instance of an accepted app ID; it does not attest sensor data.
  Development-environment credentials are reported and accepted only if the config
  lists `development`. Dev-unattested credentials prove key possession only.
- **Groth16 setup** — soundness depends on the ceremony named in the circuit
  descriptor; `development-single-party` is not a production setup.
- **Network config** — the verifier's own trust anchors (accepted circuits, app
  IDs, environments); never taken from the ledger or peers.

## Resource limits

Image ≤ 16 MiB; registry bundles ≤ 1 MiB; account history ≤ 64 pages × 200
transactions; ≤ 32 PROOF candidates per capture; lookup: ≤ 16 nodes, ≤ 32
candidates, node response ≤ 1 MiB; recordings ≤ 64 MiB. Network transports
bound each request (galaxy-xrpl: timeouts and response caps); the CLI adds an
overall `--timeout`. Hostile inputs produce INVALID/UNSUPPORTED/UNAVAILABLE
reports, never panics.

## Offline recordings

`recorded::Recording` replays JSON-RPC answers (`[{method, params, result}]`,
`api_version` ignored when matching); an unmatched request is a transport error
(UNAVAILABLE, never absence). `recorded::Recorder` wraps a live transport and
captures a run (`galaxy --record FILE`). The test suite
(`crates/galaxy-verifier/tests/`) builds histories from real signed AccountSet
transactions (throwaway wallets); `tests/main_v1m.rs` runs the real main_v1m
circuit (development setup) with a proof made for its offline challenge
(`test-vectors/verifier/main-v1m/`), the other cases use a small development
Groth16 circuit with the v1 public input count. See ADR 0090 and 0091.
