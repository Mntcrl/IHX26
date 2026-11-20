# Galaxy v1 encodings — implementation reference

Normative source: `docs/protocol/galaxy-v1-spec.md` (§2, §3, §6, §7, §9, §10, §12). This page maps
each encoding to the Rust item that implements it, the strictness rules the decoders enforce,
and the test vectors that pin it. Implementations: `crates/galaxy-crypto` (field, Poseidon,
trees, commitments, sampling, Groth16) and `crates/galaxy-protocol` (ids, canonical JSON,
policy, wire, network config, XMP). Both build for `wasm32-unknown-unknown` and
`aarch64-apple-ios` without OS-specific dependencies.

## Field elements (§2) — `galaxy_crypto::field`
| item | rule |
|---|---|
| `fr_from_be_canonical(&[u8])` | exactly 32 bytes, big-endian, value < r; ≥ r is an error (never reduced) |
| `fr_to_be` | 32-byte big-endian canonical form |
| `hash_limbs(&[u8; 32])` | `(hi, lo)` = BE integers of bytes `[0..16]`, `[16..32]` |
| `fr_from_i128` | signed integers only: x mod r |
| `fr_from_dec` / `fr_to_dec` | decimal strings without sign, leading zeros or whitespace; value < r |

Wire scalars in CAPTURE (`rgb_root`, `evidence_root`) are checked against r by the wire decoder
itself. Vectors: `test-vectors/protocol/field.json`.

## Poseidon and tags (§6) — `galaxy_crypto::{poseidon, tags}`
`poseidon::hash(&[Fr])` = circomlib `Poseidon(n)` for n = 1..=5 (light-poseidon `new_circom`,
pinned, ADR 0010). `tags::tag(code, aux)` = `0x47414C58·2^96 + 1·2^80 + code·2^64 + aux`.
Vectors: `poseidon.json` (circomlibjs `buildPoseidon`), `field.json` (tags).

## Trees and commitments (§6) — `galaxy_crypto::{layout, tree, commit}`
Morton layout (`layout::morton`, `leaf_slot`), arity-4 depth-6 frame trees (`FrameTree::build`,
`opening`, `root_from_leaf`), leaf packing (`pack_rgb`, `pack_depth`), padding tiles,
components (`tree_component`, `scalar_component`, `pad_component`) and `evidence_root`.
Openings are strictly ordered: sibling order is the child index `bit_x + 2·bit_y` at each level.
Vectors: `trees.json`, `commitments.json`.

## Sampling (§7) — `galaxy_crypto::sampling`
`seed`, `tile_stream_block`, `select_tiles` (stratified partial Fisher–Yates with rejection
`x < ⌊2^32/n⌋·n`), `ctx`, `slot_hash`, `canonical_le_bits` (254 bits), `slot_picks`,
`hypothesis_indices`, `Challenge::derive`. Details and rationale: ADR 0012. Vectors:
`sampling.json` (independent Node.js implementation).

## Public statement (§9) — `galaxy_crypto::public::public_inputs_v1`
The single source of the 22 public inputs: `rgb_root, evidence_root, capture_hi, capture_lo,
seed_hi, seed_lo, tiles[0..16]`. Vector: `public.json`.

## Groth16 (§9) — `galaxy_crypto::groth16`
- Proof: 256 bytes `A.x, A.y, B.x.c0, B.x.c1, B.y.c0, B.y.c1, C.x, C.y`, each 32-byte BE < q.
- VK: `u8 1 ‖ u8 1 (bn254) ‖ u32be n_public ‖ alpha_g1 ‖ beta_g2 ‖ gamma_g2 ‖ delta_g2 ‖ IC[n_public+1]`,
  `n_public ≤ 1024`.
- Every decoder (bytes and snarkjs JSON) rejects coordinates ≥ q, off-curve points, points
  outside the prime-order subgroup (G1 and G2), the point at infinity, wrong lengths and
  trailing bytes. `verify` rejects a public-input count different from the VK's `n_public`.
- snarkjs `[c0, c1]` order is pinned by a real snarkjs proof (`test-vectors/protocol/groth16/`,
  regenerated with `scripts/protocol-groth16-vectors.sh`).

## Identifiers (§3, §11) — `galaxy_protocol::ids`
`photo_id`, `capture_id`, `assertion_client_data(_hash)`, `registration_client_data_hash`,
`dev_assertion_message`, `dev_key_id`, `bundle_id`, `policy_id`, `circuit_id`, `vk_digest`,
`node_descriptor_signature_message`. Every domain tag is ASCII followed by `0x00`. `policy_id`,
`circuit_id` and the node message validate canonical JSON first and fail on any other byte
form. Vectors: `ids.json` (Node.js), `policy.json`.

## Canonical JSON (§2) — `galaxy_protocol::canonical_json`
`to_vec` produces, `validate` / `validate_object` check exact bytes: ASCII only, keys sorted
bytewise (so `"n_public"` precedes `"name"`), no whitespace, integers with |n| < 2^53 only,
minimal escapes (`\"`, `\\`, `\u00xx` for control characters), no duplicate keys (a duplicate
cannot survive the parse/re-encode comparison).

## Wire formats (§10) — `galaxy_protocol::wire`
One codec per format: `CaptureCore` (299 B), `CaptureMemo`, `ProofMemo` (361 B), `ChunkMemo`,
`DeviceManifest` (141 B), `DeviceBundle` (`GXDB`), `CircuitManifest` (147 B), `CircuitBundle`
(`GXCB`), `RevokeMemo` (42 B), `NodeAnnouncement` (+ `signing_message`). `MemoKind` maps the
MemoType strings (`as_str`, `FromStr`, `from_memo_type(&str) -> Option<MemoKind>`); matching is
exact (case-sensitive, no whitespace, version `v1` only). MemoFormat is
`application/octet-stream`.

Strict decoding rules (ADR 0013), all fail closed:
- version byte = 1; bundle magic exact; no trailing bytes; every length field exact.
- `network` ∈ {0, 1, 2}.
- CAPTURE: `flags` bits 4–15 must be 0 and the image profile (bit 0..3 value) ∈ {0 png-v1,
  1 jpeg-v1}; `rgb_root`, `evidence_root` < r; `1 ≤ assertion_len ≤ 600`; an assertion starting
  with `0xF0` (dev-unattested marker; never a CBOR map head) must be exactly 65 bytes.
- CHUNK: `index < total`; `1 ≤ payload_len ≤ 800`.
- DEVICE / CIRCUIT manifests: `1 ≤ bundle_len ≤ 1 MiB`, `1 ≤ chunk_total ≤ bundle_len`,
  `bundle_len ≤ chunk_total·800`, `first_chunk_ledger ≤ last_chunk_ledger`.
- device kind ∈ {1 apple-app-attest, 0xF0 dev-unattested}; env ∈ {0, 1, 2}; env = 0 iff kind = 0xF0.
- GXDB: whole bundle ≤ 1 MiB; app id = dot-separated non-empty `[A-Za-z0-9-]` labels (empty
  only for dev-unattested); attestation 1..1 MiB, exactly 32 bytes (ed25519 key) for 0xF0.
- GXCB: whole bundle ≤ 1 MiB; each section 1..1 MiB; descriptor and policy JSON are canonical
  objects. VK bytes are opaque here (validated by `groth16::VerifyingKey::from_bytes`).
- NODE: URL 1..65535 bytes of printable ASCII (no spaces).
- PROOF: the 256 proof bytes are opaque here; `groth16::Proof::from_bytes` validates them.
Ledger-context rules (account checks, REVOKE ledger ranges, chunk reassembly) are enforced by
the consumers (galaxy-xrpl, galaxy-verifier), not by the byte codecs.

Vectors: `wire.json` (valid cases round-trip byte-exactly; each reject case fails), built field
by field in Node.js by `scripts/protocol-gen-wire-vectors.mjs`.

## Network config (§12) — `galaxy_protocol::network::NetworkConfig`
TOML, unknown keys rejected, ≤ 64 KiB. `network_id` must match `name`; mainnet refused unless
`allow_mainnet = true`; `max_fee_drops ≤ 1_000_000`; app ids validated like GXDB app ids.
Shipped files: `deployments/networks/xrpl-testnet.toml`, `xrpl-devnet.example.toml`.

## XMP (§4) — `galaxy_protocol::xmp::GalaxyXmp`
Namespace `https://xrpl-galaxy.org/ns/galaxy/1.0/`; properties `protocol`, `network`,
`imageProfile`, `photoId`, `captureId`, `manifestTx`, `proofTx`. `to_packet` writes Galaxy ids
lowercase and XRPL tx hashes uppercase; `parse_packet` is namespace-aware, size-capped (64 KiB),
rejects DOCTYPE, duplicates, missing or malformed values, and ignores unknown properties. XMP is
a lookup hint only: verifiers re-check every binding on the ledger.

## Robustness
Every decoder has truncation, extension and random-mutation tests that must never panic and
must never accept a non-canonical form (`crates/galaxy-protocol/tests/{wire,xmp,network,encoding}.rs`,
`crates/galaxy-crypto/tests/{encoding,groth16}.rs`).
