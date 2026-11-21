# Third-party notices

Third-party components remain under their own licenses.

| Component | License or notice | Location |
| --- | --- | --- |
| circomlib 2.0.5, Copyright 2018 0KIMS association | GPL-3.0 (upstream designation) | npm dependency of [`circuits/`](circuits/); see [`circuits/LICENSE`](circuits/LICENSE). |
| ScrollCraft engine, Copyright (c) 2026 Nate Herk | MIT | [`apps/verifier-web/src/scrollcraft/`](apps/verifier-web/src/scrollcraft/); [license](apps/verifier-web/public/scrollcraft-LICENSE.txt). |
| ripple-binary-codec definitions, Copyright (c) 2015 Ripple Labs Inc. | ISC | [`crates/galaxy-xrpl/vendor/definitions.json`](crates/galaxy-xrpl/vendor/definitions.json); [license](crates/galaxy-xrpl/vendor/ripple-binary-codec.LICENSE). |
| takimoto3 App Attest fixture | MIT | [`test-vectors/attestation/public-ios-14.4-pair.json`](test-vectors/attestation/public-ios-14.4-pair.json); [license](test-vectors/attestation/LICENSE-takimoto3). |
| veehaitch App Attest fixture (original source of the same iOS 14.4 sample) | Apache-2.0 | [`test-vectors/attestation/public-ios-14.4-pair.json`](test-vectors/attestation/public-ios-14.4-pair.json); [license](test-vectors/attestation/LICENSE-veehaitch). |
| uebelack App Attest fixture | MIT | [`test-vectors/attestation/public-production.json`](test-vectors/attestation/public-production.json); [license](test-vectors/attestation/LICENSE-uebelack). |
| Apple App Attest Root CA | Public certificate, pinned as a trust anchor | [`crates/galaxy-attestation/src/Apple_App_Attestation_Root_CA.pem`](crates/galaxy-attestation/src/Apple_App_Attestation_Root_CA.pem). |
| `uniffi`, `uniffi_core`, `uniffi_pipeline` 0.32.2 | MPL-2.0 | Linked unmodified into the iOS app's GalaxyCore library ([`galaxy-ffi`](crates/galaxy-ffi/)). Source: [mozilla/uniffi-rs, tag `v0.32.2`](https://github.com/mozilla/uniffi-rs/tree/v0.32.2) and crates.io ([`uniffi`](https://crates.io/crates/uniffi/0.32.2), [`uniffi_core`](https://crates.io/crates/uniffi_core/0.32.2), [`uniffi_pipeline`](https://crates.io/crates/uniffi_pipeline/0.32.2)). |

Rust and npm dependencies are used under their own licenses. See [`Cargo.lock`](Cargo.lock) and the npm lockfiles for the [circuits](circuits/package-lock.json), [web verifier](apps/verifier-web/package-lock.json), and [XRPL vector scripts](scripts/xrpl-vectors/package-lock.json).

## Circuits and compiled circuit artifacts

The following are licensed under [GPL-3.0-only](circuits/LICENSE) because they incorporate circomlib. This includes ignored build outputs and copies outside the original locations, including copies installed on devices.

- [`circuits/`](circuits/) and its compiled outputs, including R1CS files, witness WASM and graphs, zkeys, verification keys, descriptors, and circuit bundles.
- [`test-vectors/prover/fixture.circom`](test-vectors/prover/fixture.circom) and its compiled outputs, including `test-vectors/prover/fixture.graph.bin`, `test-vectors/prover/fixture.r1cs`, `test-vectors/prover/fixture.zkey`, `test-vectors/prover/fixture_zkey_new.zkey`, `test-vectors/prover/vk.bin`, and `test-vectors/prover/vk.json`.
- [`test-vectors/verifier/main-v1m/circuit_bundle.bin`](test-vectors/verifier/main-v1m/circuit_bundle.bin).
- [`apps/prover-bench-ios/circuit/prover_bench.circom`](apps/prover-bench-ios/circuit/prover_bench.circom), its generated `main.circom`, and its compiled outputs under `apps/prover-bench-ios/artifacts/`, including the R1CS, witness graph, zkey, and verification key.

The capture app loads the separately installed GPL-3.0-only `main_v1m.graph.bin` and `main_v1m.zkey`. It runs the witness graph and uses the zkey for on-device proving. The `galaxy-ffi` tests embed `fixture.graph.bin`, `fixture.zkey`, and `vk.bin` from `test-vectors/prover/`.
