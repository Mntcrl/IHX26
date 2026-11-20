# ADR 0002 — Shared Rust core and pinned dependency set

Status: accepted (2026-11-20).

## Context
- The brief (§4) requires one implementation of protocol serialization, cryptography, image canonicalization, verification and XRPL evidence handling, exposed to Swift, the browser, a CLI and a node daemon. Circom is the only second implementation (spec §0).
- The legacy app has no Rust, no XRPL signing and no ZK code (recon §6).
- Poseidon must match circomlib exactly; `light-poseidon` provides `Poseidon::<Fr>::new_circom(n)` and fixes the arkworks generation the whole workspace must use, because field types (`ark_bn254::Fr`) cross crate boundaries.

## Decision
### Architecture
- One Cargo workspace (`members = ["crates/*"]`, resolver 2, edition 2021, rust-version 1.91, toolchain pinned to 1.91.1).
- Sans-IO core crates (`galaxy-protocol`, `galaxy-crypto`, `galaxy-image`, `galaxy-evidence`, `galaxy-geometry`, `galaxy-witness`, `galaxy-attestation`, `galaxy-verifier`); networking lives at the edges: Swift `URLSession` on iOS (no Rust networking on iOS), a JavaScript `LedgerSource` in the browser, `reqwest`/`tokio-tungstenite` natively (`galaxy-xrpl`, `galaxy-cli`, `galaxy-node`).
- Surfaces: `galaxy-ffi` (UniFFI, `staticlib` + `lib`), `galaxy-wasm` (wasm-bindgen, `cdylib` + `rlib`), `galaxy-cli` (bin `galaxy`), `galaxy-node` (bin `galaxy-node`).

### Arkworks generation: 0.6
- crates.io metadata (queried 2026-11-20): newest `light-poseidon` is 0.5.0 (published 2026-10-27) and depends on `ark-bn254 ^0.6.0`, `ark-ff ^0.6.0`, `num-bigint ^0.4.4`, `thiserror ^1.0`. The previous release 0.4.0 depends on arkworks 0.5.
- arkworks 0.6.0 is the newest release of every required crate (`ark-bn254`, `ark-ff`, `ark-ec`, `ark-groth16`, `ark-serialize`, `ark-std`, `ark-relations`); `ark-groth16 0.6.0` requires `ark-* ^0.6`.
- Therefore the newest mutually compatible set is light-poseidon 0.5.0 + arkworks 0.6.0. `num-bigint` is pinned to 0.4.8 (newest 0.4.x) because `ark-ff 0.6.0` and `light-poseidon 0.5.0` require `^0.4`; 0.5.1 would add a second, type-incompatible copy.
- Spec §0 mentions "arkworks 0.5 / 0.4 as pinned"; the pin here is 0.6 because that is what the newest light-poseidon requires.

### Pinned versions (`[workspace.dependencies]`, all `=x.y.z`)
| crate | version | notes |
|---|---|---|
| light-poseidon | 0.5.0 | circom-compatible Poseidon |
| ark-bn254, ark-ff, ark-ec, ark-serialize, ark-relations | 0.6.0 | |
| ark-groth16 | 0.6.0 | `default-features = false` (default `parallel` pulls rayon; native members opt in) |
| ark-std | 0.6.0 | `default-features = false` |
| num-bigint | 0.4.8 | see above |
| serde / serde_json | 1.0.229 / 1.0.154 | serde with `derive` |
| thiserror | 2.0.21 | light-poseidon still brings 1.0.69 transitively |
| hex / base64 / ciborium | 0.4.3 / 0.23.1 / 0.2.2 | base64 without `simd-unsafe` default |
| sha2 / ripemd | 0.11.1 / 0.2.0 | RustCrypto digest 0.11 generation |
| p256 / ed25519-dalek / x509-cert / der | 0.14.0 / 3.0.0 / 0.3.1 / 0.8.3 | all on digest 0.11 / signature 3 |
| rand_core / getrandom | 0.10.1 / 0.4.3 | wasm members enable `getrandom/wasm_js` |
| png | 0.18.1 | |
| jpeg-decoder | 0.3.2 | `default-features = false, features = ["platform_independent"]` (no rayon, scalar IDCT; spec §4) |
| jpeg-encoder | 0.7.1 | `default-features = false, features = ["std"]` (no `simd`; spec §4) |
| tokio | 1.54.1 | members choose features |
| reqwest | 0.13.5 | `default-features = false, features = ["rustls", "json"]` |
| tokio-tungstenite | 0.30.0 | `default-features = false, features = ["connect", "rustls-tls-webpki-roots"]` |
| axum | 0.8.9 | |
| rusqlite | 0.43.0 | `bundled` |
| clap | 4.6.9 | `derive` |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | subscriber with `env-filter` |
| uniffi | 0.32.2 | |
| wasm-bindgen | 0.2.130 | `wasm-bindgen-cli` 0.2.130 installed in `.tools/bin` (versions must match) |

### Evidence (real runs, 2026-11-20)
A throwaway workspace (`/tmp/galaxy-depcheck`, same `[workspace.dependencies]` and toolchain) with one native crate depending on every pinned crate and one wasm crate depending on the wasm-relevant subset:
```
$ cargo check -p depcheck-native
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 15s
$ cargo check -p depcheck-wasm --target wasm32-unknown-unknown
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 39.71s
$ cargo tree -p depcheck-native --depth 1 -e normal | grep -E 'ark|light|num-bigint'
├── ark-bn254 v0.6.0
├── ark-ec v0.6.0
├── ark-ff v0.6.0
├── ark-groth16 v0.6.0
├── ark-relations v0.6.0
├── ark-serialize v0.6.0
├── ark-std v0.6.0
├── light-poseidon v0.5.0
├── num-bigint v0.4.8
$ cargo tree -p depcheck-native -i ark-ff
ark-ff v0.6.0
├── ark-bn254 v0.6.0
│   ├── depcheck-native v0.0.0
│   └── light-poseidon v0.5.0
├── ark-crypto-primitives v0.6.0
│   └── ark-groth16 v0.6.0
├── ark-ec v0.6.0 …
```
`cargo tree -d` shows a single copy of every `ark-*` crate and of `num-bigint` (0.4.8). Accepted duplicates: `sha2 0.10.9` / `digest 0.10` (via `ark-crypto-primitives` / `ark-ff`, arkworks is still on digest 0.10), `thiserror 1.0.69` (via light-poseidon), `rand 0.8` / `rand_core 0.6` (via `ark-std`), `webpki-roots` 0.26 and 1.0 (tokio-tungstenite vs reqwest).

## Consequences
- Every member uses `dep.workspace = true`; version bumps happen only in the root `Cargo.toml` and are re-verified with `cargo tree`.
- Field types are shared across crates without conversion; Poseidon vectors are checked against circomlib (spec §6).
- reqwest's `rustls` feature uses `aws-lc-rs` (C build via `aws-lc-sys`): native builds and node Docker images need a C toolchain and cmake. iOS and wasm builds do not link reqwest.
- `ark-groth16` and `jpeg-decoder` defaults that add threads/SIMD are off; members that want `parallel` (desktop prover, node) enable it explicitly.
