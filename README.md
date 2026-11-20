# XRPL Galaxy

XRPL Galaxy certifies the physical consistency of iPhone photographs without a project server. The capture app records the photo together with ARKit LiDAR depth, camera poses, IMU and ambient aggregates, commits to them with Poseidon hashes, authenticates the capture with Apple App Attest and publishes a CAPTURE transaction on the XRP Ledger Testnet. A ledger validated after the capture selects which parts of the evidence must be opened; the phone then produces a Groth16 proof that the opened RGB, depth and motion data satisfy the Galaxy physical-consistency policy and publishes it in a PROOF transaction. Anyone can verify a photo from the image file and public XRPL data with the shared Rust verifier (CLI, node daemon or browser WebAssembly).

What a successful verification means, and what it does not, is defined in the specification: it shows that committed sensor data is physically consistent under a named policy, not that the scene is authentic.

- Specification and engineering contract: [`docs/protocol/galaxy-v1-spec.md`](docs/protocol/galaxy-v1-spec.md)
- Architecture reference (Italian): [`docs/reference/XRPL_Galaxy_Architettura_v1.docx`](docs/reference/XRPL_Galaxy_Architettura_v1.docx)
- Plan: [`docs/PLAN.md`](docs/PLAN.md) · Decisions: [`docs/adr/`](docs/adr/README.md)
- **Status:** [`docs/PROGRESS.md`](docs/PROGRESS.md) is the only source of truth for what works; most components are not implemented yet.

## Layout

```
apps/capture-ios/      Xcode project of the iOS capture app (imported history from the original XRPL_Galaxy repo)
apps/prover-bench-ios/ on-device proving benchmark app
apps/verifier-web/     browser verifier (Vite + TypeScript + WASM)
crates/                Rust workspace: galaxy-protocol, -crypto, -image, -evidence, -geometry, -witness,
                       -attestation, -xrpl, -prover, -verifier, -cli (`galaxy`), -node (`galaxy-node`),
                       -wasm (wasm-bindgen), -ffi (UniFFI)
circuits/              circom sources, tests, setup scripts, artifact manifest, ptau.lock.json
bindings/              generated Swift package and wasm package
policies/              policy JSON (galaxy-pc-v1m)
test-vectors/          cross-language golden vectors
fixtures/              synthetic scenes (fixtures/real/ is gitignored)
deployments/           network configs, docker compose
docs/                  plan, progress, spec, ADRs, security
scripts/               doctor, Mac helpers and per-area build scripts
```

## Requirements

- Linux or macOS with rustup (the toolchain is pinned by `rust-toolchain.toml`: 1.91.1 with `wasm32-unknown-unknown` and `aarch64-apple-ios`), Node.js and npm, Docker for the node deployment.
- Repo-local tools live in `.tools/` (gitignored): circom 2.2.3 built from source, `wasm-bindgen-cli` matching the workspace `wasm-bindgen`, binaryen `wasm-opt`, Powers of Tau files pinned in `circuits/ptau.lock.json`.
- iOS builds run on a Mac with Xcode, in a build mirror synced over ssh (`scripts/lib-mac.sh`); see [ADR 0008](docs/adr/0008-build-and-device-workflow.md).

Install the repo-local tools:

```
rustup target add wasm32-unknown-unknown aarch64-apple-ios
cargo install --locked --git https://github.com/iden3/circom --tag v2.2.3 --root .tools circom
cargo install --locked --root .tools wasm-bindgen-cli --version =0.2.130
(cd circuits && npm ci)
```

## Quick start

```
make doctor        # report toolchain, ptau files, Mac/device reachability and Testnet status
make core-build    # build the Rust workspace (and check galaxy-wasm for wasm32)
make core-test     # run the native Rust tests
make help          # list all targets (ios-*, circuit-*, node-*, verifier-*, test, integration-test)
```

Device and host settings are Make variables: `DEVICE`, `MAC_HOST`, `MAC_KEY`, `MAC_MIRROR`, `NETWORK` (default `xrpl-testnet`). Targets whose owner script does not exist yet exit with status 2 and say so.

## License

Apache-2.0 OR MIT.
