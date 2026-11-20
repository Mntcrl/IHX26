# galaxy-prover — on-device Groth16 proving (owner: MobileProver)

Decision record: [ADR 0080](../adr/0080-mobile-prover-backend.md). Spec: §9
(circuit interface, proof and VK encodings), §19 (Mac/device workflow).

## Backend

| stage | implementation |
|---|---|
| witness | iden3 `circom-witnesscalc` 0.3.0 graph evaluator (pure Rust). The circuit is data (`<circuit>.graph.bin`), not code: a new circuit needs no app rebuild. |
| proving key | the snarkjs Groth16 `.zkey` itself, memory-mapped (no conversion step, `zkey_sha256` of the circuit descriptor covers exactly the file the phone reads). |
| prover | arkworks 0.6 (`ark-bn254`, `ark-ec` MSM, `ark-poly` FFT) with the circom R1CS→QAP reduction (odd-coset `H`, as snarkjs / ark-circom `CircomReduction`). Points are decoded from the mapping chunk by chunk (`ProverConfig::msm_chunk`). |
| self-check | every proof is verified against the zkey's verifying key (arkworks `Groth16::verify_proof`) before it is returned; failure is `ProverError::SelfCheck`. |

Production proofs are generated on the iPhone. The Linux path (same crate,
`galaxy-prover` CLI) is a labelled development path for tests and benchmarks.

## Rust API (stable for other crates)

```rust
use galaxy_prover::{Prover, ProverConfig, ProveControl, Progress, Phase, ProverError, verify};

let prover = Prover::open(graph_path, zkey_path)?;              // mmap + header/VK validation
let prover = Prover::open_with_config(graph, zkey, ProverConfig { msm_chunk: 1 << 18 })?;
prover.n_public();                 // 22 for galaxy-pc-v1m
prover.vk_bytes()?;                // spec §9 VK bytes; compare SHA256("GALAXY/VK/v1\0" ‖ vk) with vk_digest
let out = prover.prove(inputs_json, &ProveControl { progress: Some(&sink), cancel: Some(&flag) })?;
out.proof;                         // [u8; 256], spec §9 proof wire
out.public_inputs;                 // Vec<[u8; 32]> big-endian, circuit order
out.timings.witness(); out.timings.prove(); out.timings.get(Phase::MsmB2);
verify(&vk_bytes, &out.public_inputs, &out.proof)?;   // Ok(true/false); malformed encodings are Err
```

- `inputs_json`: circom input JSON (for the real circuit: produced by
  `galaxy-witness`, schema `circuits/INPUTS.md`).
- Phases (in order, each reported via `Progress::phase_started/finished`):
  `witness`, `witness_map`, `msm_h`, `msm_l`, `msm_a`, `msm_b1`, `msm_b2`,
  `finalize`, `self_verify`.
- Cancellation: an `AtomicBool` checked before every phase and between MSM
  chunks → `ProverError::Cancelled`. Witness generation itself is not
  interruptible (it is a few percent of the total).
- Errors (`ProverError`): `Io`, `InvalidArtifact` (zkey magic/version/curve,
  section sizes, VK points off-curve or outside the subgroup, coefficient
  indices out of range, trailing bytes), `Witness` (bad inputs, wire count
  mismatch, malformed wtns), `InvalidEncoding`, `Rng`, `SelfCheck`, `Cancelled`.
- Encodings (`galaxy_prover::codec`): `encode_proof`/`decode_proof` (strict:
  canonical < q, on curve, G2 subgroup, not infinity, exact length),
  `encode_vk`/`decode_vk`, `encode_scalar`/`decode_scalar` (< r),
  snarkjs `proof.json`/`public.json` interop for development cross-checks.
  These match spec §9 and are tested against snarkjs; when galaxy-crypto's
  Groth16 codec lands, the two must agree byte for byte (same vectors).
- Randomness: r, s from `getrandom` (64 bytes reduced mod r); a zero draw is an error.
- Artifacts must not be modified while a `Prover` holds them (they are mmapped).

### UniFFI surface (feature `uniffi`, namespace `galaxy_prover`)

```swift
let prover = try GalaxyProver.open(graphPath: g, zkeyPath: z, msmChunk: 0)   // 0 = default
let cancel = CancelToken()
let r = try prover.prove(inputsJson: json, listener: myListener /* ProgressListener */, cancel: cancel)
r.proof              // Data (256 B)
r.publicInputs       // [Data] (32 B each)
r.phases             // [PhaseTiming(phase:seconds:)]
r.witnessSeconds; r.proveSeconds; r.snarkjsProofJson; r.snarkjsPublicJson
try prover.vkBytes(); prover.nPublic(); prover.nVars(); prover.domainSize()
try verifyProof(vkBytes: vk, publicInputs: r.publicInputs, proof: r.proof)
```
`galaxy-ffi` (IOS) re-exports this namespace; it depends on `galaxy-prover`
with `features = ["uniffi"]`.

## Building

### Circuit artifacts
- `scripts/prover-install-build-circuit.sh` → `.tools/bin/build-circuit`
  (circom-witnesscalc v0.3.0, embedded circom ad44e915 = v2.2.3, same commit
  as `.tools/bin/circom`).
- `build-circuit main.circom out.graph.bin -l <libs> --r1cs out.r1cs` emits
  the witness graph **and** the R1CS from one compilation; the Groth16 setup
  must use that R1CS (wire order of graph and zkey must match). The circuit
  descriptor's `witness_graph_sha256` is the sha256 of `out.graph.bin`.
- Benchmarks: `scripts/prover-build-bench.sh <name> <steps>`; test fixture:
  `scripts/prover-build-fixture.sh`.

### iOS static library + Swift bindings (Linux, no Apple SDK)
```
scripts/prover-ios-lib.sh
```
produces `bindings/swift/galaxy-prover/{ios-arm64/libgalaxy_prover.a,
swift/galaxy_prover.swift, include/galaxy_proverFFI.h, include/module.modulemap,
BUILD-INFO.txt}`. Details: `rustup target add aarch64-apple-ios`; `cargo rustc
--crate-type staticlib --target aarch64-apple-ios --features uniffi` with
fat LTO; bindgen in circom-witnesscalc's build script gets a stub `stdlib.h`;
circom-witnesscalc's unused cdylib is not linked (`scripts/prover-ios-nolink.sh`);
Swift bindings come from `uniffi-bindgen generate --library` on the host cdylib
built from the same sources/features.

Xcode integration: `SWIFT_INCLUDE_PATHS = …/include`, `LIBRARY_SEARCH_PATHS =
…/ios-arm64`, `OTHER_LDFLAGS = -lgalaxy_prover`, add `swift/galaxy_prover.swift`
to the target (apps/prover-bench-ios uses a synchronized group).

### Benchmark app
`scripts/prover-bench-ios.sh {build|install|push <names>|run <names> [--repeat N] [--msm-chunk N]|fetch|cleanup}`:
`build` syncs `apps/prover-bench-ios` + `bindings/swift/galaxy-prover` to the Mac
mirror and signs through IOS's Aqua-session runner
(`apps/capture-ios/tools/mac-gui-build.sh proverbench|proverbench-fallback`;
SSH sessions cannot use the signing keychain); `push` copies
`<name>.graph.bin`, `<name>.zkey`, `input.json` into the app's
`Documents/<name>/`; `run` launches with `--autorun <names> --exit` under
`devicectl --console` (lines prefixed `GXBENCH`, one `RESULTJSON` per proof);
`fetch` copies `Documents/results` back and verifies every phone proof with
snarkjs on Linux; `cleanup` removes only `Documents/results` and the pushed
circuit directories. Every device step holds `/tmp/galaxy-device.lock` on the Mac.

Bundle id: the free team has no Xcode account on the Mac, so
`utcq.XRPL-Galaxy.proverbench` cannot be provisioned ("No Accounts", "No
profiles for 'utcq.XRPL-Galaxy.proverbench'"). The bench therefore runs as
`BUNDLE_ID=utcq.XRPL-Galaxy` (runner mode `proverbench-fallback`), which
**replaces the capture app on the iPhone and shares its data container**.
After a bench window the capture app must be reinstalled (IOS) and the
cleanup must never touch entries it did not push. On 2026-11-20 ≈20:40 CET an
earlier version of `--cleanup` deleted every entry of that container's
`Documents` (163 entries, of which 3 were the bench's own; names were not
logged); it now deletes only its own entries and logs their names.

## Measurements
See ADR 0080 (§Measurements) for desktop and iPhone 13 Pro Max numbers.
