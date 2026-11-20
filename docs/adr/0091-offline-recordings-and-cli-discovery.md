# ADR 0091 — Offline XRPL recordings, verifier test vectors and CLI discovery

Status: accepted (2026-11-20). Normative text: spec §13, §16, §19. Details: `docs/protocol/verification.md`, `docs/cli.md`.

## Context
- Verifier behaviour must be tested deterministically without network access, but with real XRPL transaction bytes, before real Testnet captures exist.
- The App Attest material available offline (real device attestation, assertions over placeholder cores) cannot be bound to a test CAPTURE, and the main_v1m circuit artifacts (≈ 906k constraints) cannot be set up in a unit test.
- The CLI must work against a directly configured rippled without any project node, and the photo ID is a correlatable identifier.

## Decision
- **Recordings.** `galaxy_verifier::recorded::Recording` replays bare XRPL `result` objects keyed by method and exact params (`api_version` ignored) through galaxy-xrpl's `RpcLedger`; unanswered requests are transport errors (UNAVAILABLE). `Recorder` wraps a live transport to capture a run (`galaxy --record`), so real Testnet sessions become offline fixtures with the same format.
- **Synthetic histories from real transactions.** Tests sign real AccountSet transactions with galaxy-xrpl (throwaway Ed25519 wallets), encode real metadata blobs with the binary codec and synthesize ledger headers. A small development Groth16 circuit with the v1 public input count (22 public inputs, `x·x = s` per input, arkworks setup with a fixed seed, test-only) is published as a real CIRCUIT bundle so the negative cases (invalid proof, ordering, windows) are cheap to generate.
- **Real main_v1m proof.** `test-vectors/verifier/main-v1m/` holds CircuitCore's exported GXCB bundle of main_v1m (development single-party setup), a synthetic committed scene (`photo.png`, `commitment.json`; salts stay in CircuitCore's gitignored build directory) and a proof (`proof.bin`, `public.json`, `statement.json`) generated for the challenge of the offline history built by `tests/main_v1m.rs` (capture_id, Lsel = 1017, Lsel hash). The test publishes the bundle and the proof and runs the full pipeline; a changed proof byte is INVALID. Any change of the test history's CAPTURE core or ledger hashes requires a new proof from the circuit prover (the test prints the statement).
- **Committed vectors.** `test-vectors/verifier/dev-unattested/` (recording, network config, refs) are regenerated and compared by `cargo test -p galaxy-verifier --test vectors` (`GALAXY_WRITE_VECTORS=1` rewrites); the 2.4 MB image is rebuilt from its documented raster formula instead of being committed.
- **CLI discovery.** There is no built-in node list. Lookup nodes come only from `--node` / `GALAXY_NODES`; an image without references falls back to lookup when nodes are configured; `--no-discovery` guarantees that no node is contacted. Lookup uses `GET {node}/v1/photos/{photo_id}?network_id=N` with a 1 MiB cap, no redirects and a 15 s timeout.
- **Node commands** (`galaxy node …`) are behind the `node` cargo feature and delegate to the galaxy-node library entry points.

## Consequences
- The success path (VERIFIED with an Apple credential) and authenticated counter replay are covered by galaxy-attestation tests and by recordings of real Testnet captures, not by synthetic histories.
- Recordings trust the recorded RPC answers exactly as a live run trusts its endpoint; they are reproducible evidence of what an endpoint said, not proofs of ledger state.
