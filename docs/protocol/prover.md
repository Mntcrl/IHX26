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
use galaxy_prover::{ArtifactDigests, Prover, ProverConfig, ProveControl, Progress, Phase, ProverError, verify};

let prover = Prover::open(graph_path, zkey_path)?;              // mmap + header/VK validation
let prover = Prover::open_with_config(graph, zkey, ProverConfig { msm_chunk: 1 << 18 })?;
// Production path: SHA-256 of the mapped graph/zkey must equal the circuit
// descriptor's witness_graph_sha256 / zkey_sha256, else InvalidArtifact.
let prover = Prover::open_pinned(graph, zkey, &ArtifactDigests { witness_graph_sha256, zkey_sha256 }, ProverConfig::default())?;
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
let prover = try GalaxyProver.open(graphPath: g, zkeyPath: z, msmChunk: 0)   // 0 = default (benchmarks)
// Capture app: pin the artifacts to the accepted circuit descriptor (32-byte digests).
let prover = try GalaxyProver.openPinned(graphPath: g, zkeyPath: z,
    witnessGraphSha256: descriptorGraphSha, zkeySha256: descriptorZkeySha, msmChunk: 0)
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
with `features = ["uniffi"]`. Capture-app sequence: `openPinned` → compare
`SHA256("GALAXY/VK/v1\0" ‖ vkBytes())` with the descriptor's `vk_digest`
(galaxy-protocol ids) → `prove(inputsJson:)` with the `galaxy-witness` input →
PROOF memo carries `r.proof`. `prove` runs on the caller's thread (call it off
the main thread); `CancelToken.cancel()` aborts between phases/MSM chunks.

## Building

### Circuit artifacts
- `scripts/prover-install-build-circuit.sh` → `.tools/bin/build-circuit`
  (circom-witnesscalc v0.3.0, embedded circom ad44e915 = v2.2.3, same commit
  as `.tools/bin/circom`).
- `build-circuit main.circom out.graph.bin -l <libs> --r1cs out.r1cs` emits
  the witness graph **and** the R1CS from one compilation; the Groth16 setup
  must use that R1CS (wire order of graph and zkey must match). The circuit
  descriptor's `witness_graph_sha256` is the sha256 of `out.graph.bin`.
- Benchmarks: `scripts/prover-build-bench.sh <name> <steps> [ptau]` (setup via
  `galaxy-prover dev-setup`, see below); test fixture: `scripts/prover-build-fixture.sh`.

### Development setup (`galaxy-prover dev-setup`)
```
galaxy-prover dev-setup --r1cs C.r1cs --ptau P.ptau --ptau-sha256 HEX --out C.zkey
```
DEVELOPMENT ONLY. Phase 1 = the prepared powers of tau of `P.ptau` (hashed first;
a digest other than `HEX` fails closed; pass the `sha256` of `circuits/ptau.lock.json`);
phase 2 = one contribution with δ drawn from the OS RNG inside the process and
dropped on return. Whoever runs it could forge proofs, so the key must never be
accepted outside development networks ([ADR 0043](../adr/0043-groth16-setup-ceremony.md)).
Output: snarkjs zkey sections 1–9 (no section 10 transcript);
the δ-independent sections equal `snarkjs zkey new` byte for byte (test
`tests/dev_setup.rs`), and snarkjs proves/verifies with it. It exists because
`snarkjs groth16 setup` needs ~10 GB at ~1M constraints. Memory: the r1cs and ptau
are mmapped; the per-wire transposition is the main heap cost.
Threads: it uses the current rayon pool only (`RAYON_NUM_THREADS`; set to 4 by
`scripts/with-slot.sh`). ark-ec 0.6's parallel `msm_bigint` creates a new rayon
pool on every call; the setup combines each wire's terms serially inside its
parallel map and calls the arkworks MSM only from its own thread for wires with
more than 4096 terms (an earlier version called it per wire inside the map and
reached 16,458 threads on the 500k bench). `tests/dev_setup_threads.rs` bounds the
live thread count during a run.

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
`scripts/prover-bench-ios.sh {build|install|list|push <names>|run <names> [--repeat N] [--msm-chunk N]|fetch|cleanup}`:
`build` syncs `apps/prover-bench-ios` + `bindings/swift/galaxy-prover` to the Mac
mirror and signs through IOS's Aqua-session runner
(`apps/capture-ios/tools/mac-gui-build.sh proverbench|proverbench-fallback`;
SSH sessions cannot use the signing keychain); `list` prints the container's
`Documents/` (spec §19: before and after every window); `push` copies
`<name>.graph.bin`, `<name>.zkey`, `input.json` into
`Documents/galaxy-proverbench/<name>/`; `run` launches with `--autorun <names> --exit`
under `devicectl --console` (lines prefixed `GXBENCH`, one `RESULTJSON` per proof);
`fetch` copies `Documents/galaxy-proverbench/results` back and verifies every phone
proof with snarkjs on Linux; `cleanup` fetches, then the app logs every file under
`Documents/galaxy-proverbench/` and deletes exactly that directory (spec §19 device
data safety). Every device step holds `/tmp/galaxy-device.lock` on the Mac.

Bundle id: the free team has no Xcode account on the Mac, so
`utcq.XRPL-Galaxy.proverbench` cannot be provisioned ("No Accounts", "No
profiles for 'utcq.XRPL-Galaxy.proverbench'"; rechecked 2026-11-20 22:13). The
bench therefore runs as `BUNDLE_ID=utcq.XRPL-Galaxy` (runner mode
`proverbench-fallback`, orchestrator-approved per window), which **replaces the
capture app on the iPhone in place and shares its data container**. Procedure:
list `Documents/` and copy `Documents/appattest` + `Documents/galaxy-fixtures` to
`fixtures/real/device-backup-<timestamp>/` (gitignored) first; install in place
only (never uninstall: that deletes the container and the App Attest key); after
the window reinstall the capture app (`mac-gui-build.sh capture-development` +
`devicectl device install app`), launch it once and compare the after-listing.
On 2026-11-20 ≈20:40 CET an earlier version of `--cleanup` deleted every entry of
that container's `Documents` (163 entries, of which 3 were the bench's own; names
were not logged); cleanup is now restricted to `Documents/galaxy-proverbench/`.

## Measurements
Real runs only. Desktop: Linux x86_64, 8 CPUs and 13 GiB shared with other agents
(timings are upper bounds), `RAYON_NUM_THREADS=4` via `scripts/with-slot.sh`;
peak RSS from `scripts/prover-time.py` (getrusage of the child), live threads
sampled from `/proc/<pid>/task` every 5 ms.

`galaxy-prover dev-setup` (2026-11-20):

| circuit | constraints | domain | ptau | wall | peak RSS | live threads | zkey |
|---|---|---|---|---|---|---|---|
| fixture | 348 | 2^9 | dev_9 | 0.2 s | — | 9 (pool 8; before the fix: 621) | 214 kB |
| bench_500k | 499,869 | 2^19 | hez_final_20 | 690 s (965 s before GLV) | 746 MB | 5–9 | 520 MB |
| bench_1m | 1,046,288 | 2^20 | hez_final_21 | 1,178 s | 1.38 GB | 5 (sampled) | 1.085 GB |
| main_v1m (run by CircuitCore4) | 876,639 | 2^20 | hez_final_20 | 1,275 s | 1.0 GiB | 5 | 831 MB |

Reference: `snarkjs groth16 setup` on bench_500k took 1,029 s at 5.4 GB RSS (and
does not fit at ~1M). bench_500k cross-check: zkey sections 1–7 (header with
α/β/γ, IC, coefficients, A, B1, B2) byte-equal to the snarkjs key built from the
same r1cs and ptau; sections 8/9 differ by δ as expected; snarkjs exports the VK.

Desktop proving with dev-setup keys (galaxy-prover prove → snarkjs groth16 verify):

| circuit | witness | prove | peak RSS | snarkjs |
|---|---|---|---|---|
| bench_500k | 0.24 s | 5.19 s | 901 MB | OK (and `Invalid proof` against the snarkjs-setup VK) |
| bench_1m | 0.47 s | 11.0 s | 1.72 GB | OK |
| main_v1m (synthetic room seed 0, reference verdict PASS; pinned open) | 0.34 s | 8.62 s | 1.48 GB | OK; `galaxy-prover verify` OK; VK bytes = `verification_key.bin`; vk_digest matches `descriptor.json`; a wrong `--zkey-sha256` fails with `InvalidArtifact` |

main_v1m prove phases (s): witness_map 1.36, msm_h 2.62, msm_l 1.02, msm_a 1.07,
msm_b1 0.73, msm_b2 1.82, self_verify 0.003.

iPhone 13 Pro Max (iPhone14,3, iOS 27.2.1, 6 active CPUs; 2026-11-20 ≈20:37,
snarkjs-setup keys), bench_500k: open 0.02–0.04 s, witness 0.20–0.29 s, prove
3.40–3.49 s, peak phys_footprint 462–542 MB, thermal nominal; both proofs
verified with snarkjs on Linux. bench_1m and main_v1m on the device: not run (the
device window of 22:15 was revoked before installation; artifacts are ready:
`apps/prover-bench-ios/artifacts/bench_1m/`, `circuits/artifacts/v1m/`).
