# Protocol test vectors (spec §16)

JSON; bytes as lowercase hex, field elements as decimal strings. Every file has `description`,
`section` and `cases`. Vectors are produced by independent Node.js code (circomlibjs, node:crypto,
snarkjs) and consumed by the Rust tests named below. Regenerate with the listed script; the
generators are deterministic, so a diff means a behaviour change.

| file | spec | content | generator | consumed by |
|---|---|---|---|---|
| `field.json` | §2, §6 | canonical scalar boundaries (0 … 2^256−1, r−1/r), hash limbs, domain tags | `scripts/protocol-gen-vectors.mjs` | `crates/galaxy-crypto/tests/encoding.rs` |
| `poseidon.json` | §6 | circomlib Poseidon, arities 1–5 | same | `galaxy-crypto/tests/encoding.rs` |
| `trees.json` | §6 | leaf packing, RGB/DEPTH/FRAME roots of a synthetic 256×192 frame, openings incl. corner tiles 0, 7, 40, 47 and padding neighbours | same | `galaxy-crypto/tests/commitments.rs` |
| `commitments.json` | §6 | tree/scalar/padding components, evidence root | same | `galaxy-crypto/tests/commitments.rs` |
| `sampling.json` | §3, §7 | 24 seeded cases: seed, tile stream, stratified picks, ctx, slot bits/picks, hypothesis indices | same | `galaxy-crypto/tests/sampling.rs` |
| `public.json` | §9, §22 | order of the 22 public inputs (`public_inputs_v1`) | same | `galaxy-crypto/tests/sampling.rs` |
| `policy.json` | §2, §3, §8.7 | canonical bytes and policy_id of `policies/galaxy-pc-v1m.json` | same | `crates/galaxy-protocol/tests/encoding.rs` |
| `ids.json` | §3, §11 | photo_id, capture_id, assertion/registration clientData, dev key/message, bundle/policy/circuit ids, vk_digest, node message | `scripts/protocol-gen-wire-vectors.mjs` | `galaxy-protocol/tests/wire.rs` |
| `wire.json` | §2, §10 | 13 valid memo/bundle encodings (byte-exact round trip) and 57 reject cases | same | `galaxy-protocol/tests/wire.rs` |
| `groth16/` | §9 | real snarkjs proof of `tiny.circom` (development single-party setup): `proof.json`, `public.json`, `verification_key.json`, and `vectors.json` with the §9 proof/VK bytes plus negative cases (flipped c0/c1, off-curve, wrong G2 subgroup, infinity, coordinate ≥ q, wrong public input, input ≥ r, wrong n_public, truncated/extended, VK header errors) | `scripts/protocol-groth16-vectors.sh` (+ `.mjs`) | `galaxy-crypto/tests/groth16.rs` |

The Groth16 setup in `groth16/` is a throwaway development key for a 3-public-input test circuit;
it is never accepted by any network configuration.
