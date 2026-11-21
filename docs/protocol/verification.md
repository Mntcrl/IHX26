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
2. **Ledger** — One `server_info` preflight per endpoint attempt checks the
   configured NetworkID and sets the evaluation tip (`trust.latest_validated`).
   Registry reads reuse this snapshot.
3. **CAPTURE** — fetched by hash; the returned blob must hash to the requested
   id. Strict `galaxy/v1/capture` decoding. With an image, `photo_id` and
   `rgb_root` must equal the presented raster's (else INVALID in direct/manual
   mode). Validated + `tesSUCCESS` required (not validated → PENDING, failed →
   INVALID). Network (memo field, `NetworkID` presence rule §12, inception ledger)
   and account (AccountSet sender = core account) are bound.
   The master-key signature must verify and bind a nonzero `AccountTxnID`.
4. **Circuit** — `circuit_id` must be in `NetworkConfig.accepted_circuits` with the
   CAPTURE's `policy_id`, else UNSUPPORTED. The GXCB bundle is reassembled from the
   configured CIRCUIT registration (or taken from a local copy); descriptor, policy
   and VK digests must equal the config entry; the descriptor must bind the
   v1 public layout (22 inputs), policy id, VK digest and name a ceremony; the
   policy must validate. A VK published on XRPL is never trusted by itself.
5. **Registration** — DEVICE manifest + chunks (§10 retrieval rules), manifest
   earlier than the CAPTURE and from the same account, a nonzero `AccountTxnID`
   signed by that account's master key, registration ledger hash equal to
   the validated ledger's hash, then `verify_registration` (App Attest
   chain to the pinned Apple root at the manifest ledger close time, app ID and
   environment accepted by the config). A dev-unattested credential is accepted
   for key possession only and labelled "development unattested credential".
6. **Assertion** — `verify_capture_assertion` over the exact 299-byte core
   (App Attest ES256, or the dev Ed25519 signature).
7. **Account history** — three logical windows: A `[DEVICE, L0]` for earlier
   CAPTUREs and counters; B `(Lsel, min(deadline, tip)]` for PROOF; C
   `[DEVICE, tip]` for REVOKE. Reuse A for C through L0. A single remaining
   scan covers B and C after L0, including any same-ledger successors of the
   CAPTURE that a backward walk in A could not see.
   Each scan uses at most 64 `account_tx` pages of 200 rows by default. On
   exhaustion, discard partial results and follow `AccountTxnID` (default
   4,096 steps). A starts at the CAPTURE's signed predecessor and must reach
   its DEVICE manifest. The shared B/C walk starts at the validated tip's
   AccountRoot and stops at L0: at the first own transaction in L0 if A
   paged all of L0, otherwise at the CAPTURE after checking any successors
   in L0. Recompute each linked hash and require its account, order and
   successful validation. If an own transaction has no signed predecessor,
   read `PreviousFields.AccountTxnID` in RPC metadata.
   Third-party, failed, unsigned and mis-signed rows are ignored and counted;
   incomplete history or a broken/exhausted walk returns UNAVAILABLE.
8. **Uniqueness, counter, revocation** — an earlier CAPTURE in A from the same
   account that shares its photo ID, RGB root or evidence root → INVALID,
   regardless of registration. Capture ID uniqueness and strictly increasing
   App Attest counters are checked per registration (dev credentials have
   no counter). REVOKE is honoured with or without the predecessor field.
   Under §10, effective ledger 0 = revoke ledger; backdating is allowed;
   future-dated or out-of-range revokes are ignored and reported; L0 ≥ effective
   → INVALID. The report states "credential state as of ledger N".
9. **Windows** — `Lpre < L0 ≤ Lpre + w_cap`, Lpre hash equals the validated
   ledger's hash; `Lsel = L0 + sel_offset` must be validated (else PENDING).
10. **Challenge** — seed, 16 sampled tiles and the 22 public inputs are recomputed
    (`galaxy_crypto::sampling`, `public_inputs_v1`) from the CAPTURE, its
    capture_id and the Lsel hash.
11. **PROOF selection** — candidates are the PROOFs found in the account history
    (ledger order). A PROOF hint from XMP or the caller is evaluated and
    listed but can never select itself. Each candidate: validated + `tesSUCCESS`,
    signed with the CAPTURE account's master key, with a nonzero `AccountTxnID`,
    references this CAPTURE, ledger in `(Lsel, Lsel + w_proof]`, memo network /
    Lsel / Lsel hash / circuit equal to
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
| capture.txn_chain | cryptographic | CAPTURE signs a nonzero AccountTxnID |
| capture.photo_binding | cryptographic | CAPTURE photo_id = image photo_id |
| capture.rgb_binding | cryptographic | CAPTURE rgb_root = image rgb_root |
| capture.account_binding | cryptographic | sender = core account = registration account |
| circuit.accepted | cryptographic | circuit + policy pinned in config |
| circuit.registration | cryptographic | bundle digests match the config |
| registration.resolve | ledger-rpc | DEVICE manifest found, earlier, same account, reg ledger hash |
| registration.txn_chain | cryptographic | DEVICE signs a nonzero AccountTxnID |
| registration.bundle | cryptographic | bundle reassembled and matches manifest |
| registration.attestation | attestation | App Attest registration (dev credential: fail, labelled) |
| capture.assertion | attestation | assertion over the CAPTURE core |
| capture.counter | attestation | counter strictly increasing |
| history.complete | ledger-rpc | history windows complete, with page, ignored-row and walk counts |
| capture.unique | ledger-rpc | earlier CAPTUREs in A: unique capture ID per registration, unique photo ID, RGB root and evidence root per account |
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

Lookup (§13): use the earliest validated, master-key-signed CAPTURE that
binds to this photo among the consulted hints, in XRPL transaction order
across all registrations and accounts. Later binding CAPTUREs are counted
and listed as not evaluated; their proofs cannot change the result.
Non-binding hints are rejected, not INVALID. An incomplete lookup (node
error, gaps, caps, unavailable ledger data) yields UNAVAILABLE unless the
earliest binding hint establishes VERIFIED, PENDING, UNSUPPORTED or
INCONCLUSIVE. A complete lookup with no binding candidate yields NOT_FOUND
with the consulted ranges and nodes. Even a complete lookup cannot exclude
an earlier CAPTURE omitted by those nodes.

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

- **XRPL RPC** — trusted for validation status, results, ledger order, hashes,
  close times, history coverage and the AccountRoot and metadata anchors.
  Every blob fetched by hash must match it. CAPTUREs, PROOFs, registrations,
  history rows and chunks must verify under the Galaxy account's Ed25519
  master key before they can contribute evidence. One endpoint supplies all
  answers for an attempt; a failure restarts the entire attempt on the next
  endpoint. Answers are never combined or cross-checked. Use your own
  synchronized rippled to avoid trusting a remote endpoint.
- **Lookup nodes** — trusted for nothing. They can omit records (absence is only
  reported for the ranges they claim) and observe explicit photo-ID queries.
  `galaxy verify` sends no photo ID unless `--lookup` is present.
- **App Attest** — establishes that the credential key lives in a genuine Apple
  app instance of an accepted app ID; it does not attest sensor data.
  Development-environment credentials are reported and accepted only if the config
  lists `development`. Dev-unattested credentials prove key possession only.
- **Groth16 setup** — soundness depends on the ceremony named in the circuit
  descriptor; `development-single-party` is not a production setup.
- **Network config** — the verifier's own trust anchors (accepted circuits, app
  IDs, environments); never taken from the ledger or peers.

## Resource limits

Image ≤ 16 MiB; registry bundles ≤ 1 MiB; each of the two history scans defaults
to 64 pages × 200 `account_tx` rows, then ≤ 4,096 AccountTxnID steps (CLI
configurable to 65,536); ≤ 32 PROOF candidates per capture; lookup: ≤ 16 nodes,
≤ 32 candidates, node response
≤ 256 KiB; recordings ≤ 64 MiB. Network transports
bound each request (galaxy-xrpl: timeouts and response caps); the CLI adds an
overall `--timeout`. Hostile inputs produce INVALID/UNSUPPORTED/UNAVAILABLE
reports, never panics.

## Offline recordings

`recorded::Recording` replays JSON-RPC answers (`[{method, params, result}]`,
`api_version` ignored when matching); an unmatched request is a transport error
(UNAVAILABLE, never absence). `recorded::Recorder` wraps a live transport and
captures a run (`galaxy --record FILE`). An AccountTxnID-walk recording includes
the `account_info` anchor and linked `tx` results. Regenerate older recordings
if they lack responses for the shared B/C scan’s new request range. The test suite
(`crates/galaxy-verifier/tests/`) builds histories from real signed AccountSet
transactions (throwaway wallets); `tests/main_v1m.rs` runs the real main_v1m
circuit (development setup) with a proof made for its offline challenge
(`test-vectors/verifier/main-v1m/`), the other cases use a small development
Groth16 circuit with the v1 public input count. See ADR 0090 and 0091.
