# ADR 0080 — Mobile proving backend

Status: accepted (2026-11-20). Owner: MobileProver.

## Context
Spec §9 requires a Groth16 BN254 proof of a Circom 2 circuit (circomlib
Poseidon, 22 public inputs, target ~600k–1M constraints) generated **on the
iPhone 13 Pro Max** (A15: 2 performance + 4 efficiency cores, 6 GB RAM,
iOS 27.2.1). Cloud proving is forbidden; desktop proving is a labelled
development path only. Builds must be reproducible and preferably done on
Linux (spec §19, ADR 0008: the Mac has 8 GB RAM, ~8.6 GB free disk, a
congested Wi-Fi link and no Rust iOS targets).

Investigated options (evidence gathered 2026-11-20 from crates.io metadata,
the published crate sources, GitHub release listings and local builds):

| option | facts | verdict |
|---|---|---|
| **Mopro** (`zkmopro/mopro`, `mopro-ffi` 0.3.7, 2026-07-14; `circom-prover` 0.1.4) | Generic mobile toolkit: UniFFI bindings + pluggable witness (`rust-witness`, `witnesscalc-adapter`, `circom-witnesscalc`) and prover (`arkworks`, `rust-rapidsnark`) adapters. `circom-prover` 0.1.4 pins arkworks `=0.5.0`, `circom-witnesscalc ^0.2.1`, re-reads and fully deserializes the zkey (all points + matrices) per proof, no phase progress or cancellation. | Not used as a dependency (arkworks 0.5 vs our 0.6 line, no progress/cancel, whole-zkey deserialization). Its architecture (graph witness + arkworks + CircomReduction) is what we adopt. |
| `rust-witness` 0.1.6 | Transpiles circom wasm → C with w2c2, compiles with `cc` at build time. | Needs an Apple C toolchain/SDK for `aarch64-apple-ios` and recompiling the app for every circuit change. Rejected. |
| `witnesscalc-adapter` 0.1.7 | Clones iden3 C++ witnesscalc, builds with cmake/make (`package_ios_arm64`), links per-circuit C++. | Needs Apple SDK + cmake on the build host; circuit compiled into the binary. Rejected. |
| **iden3 `circom-witnesscalc` 0.3.0** (2026-02-19, rev d48eb7c) | Pure Rust graph evaluator; `build-circuit` (embeds circom ad44e915 = v2.2.3, same as `.tools/bin/circom`) turns a circom program into a data file (graph) and can emit the R1CS from the same compilation (`--r1cs`). README documents `aarch64-apple-ios` builds. Build deps: `prost-build` (protoc) and `bindgen` over a 20-line header. | **Chosen witness generator.** Cross-compiles on Linux (see below); the circuit is data, so a new circuit/policy needs no app rebuild. |
| **rapidsnark** (iden3 v0.0.8, 2026-01-07; `rust-rapidsnark` 0.1.4) | Prebuilt `rapidsnark-iOS-v0.0.8.zip` (librapidsnark/libfr/libfq/libgmp static libs, C++ + GMP/LGPL, assembly field arithmetic). `rust-rapidsnark`'s build script downloads the zip at build time. | Fastest known mobile prover [INFERENCE from upstream benchmarks, not measured here], but binary blobs (not reproducible from source on Linux), LGPL GMP, C++ link on Xcode. Kept as the fallback if arkworks misses the latency budget. |
| **arkworks** `ark-groth16`/`ark-ec`/`ark-poly` 0.6.0 | Pure Rust, parallel MSM (rayon; signed-bucket Pippenger with small-scalar grouping) and radix-2 FFT. Workspace already pins the 0.6 line (light-poseidon 0.5). | **Chosen prover arithmetic.** |
| `ark-circom` 0.6.0 (zkey reader, `CircomReduction`) | Depends unconditionally on `wasmer` 6 (JIT; JIT is not permitted for iOS apps) and `color-eyre`. | Not used as a dependency; we re-implement the zkey reader (zero-copy, mmap) and use the same reduction (odd-coset H) on arkworks primitives. |

Linux → iOS cross-compilation (option e), verified: `rustup target add
aarch64-apple-ios`; `cargo rustc --crate-type staticlib --target
aarch64-apple-ios` needs no linker for the static library. Two obstacles and
their fixes (scripts/prover-ios-lib.sh):
1. circom-witnesscalc's `build.rs` runs bindgen on a header that includes
   `<stdlib.h>`; without an Apple SDK clang cannot find it → a two-line stub
   `stdlib.h` via `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_ios` (the header only
   declares an `int` enum and a `char*` struct; layout identical on all LP64
   aarch64 targets).
2. circom-witnesscalc declares `crate-type = ["lib","staticlib","cdylib"]`;
   Cargo links the dependency's cdylib, which fails without an SDK
   (`cc: unrecognized option '-target'`) → `scripts/prover-ios-nolink.sh` is
   the target linker; it writes a placeholder only for
   `libcircom_witnesscalc*.dylib` (never used) and refuses any other link.
Result: `libgalaxy_prover.a` (Mach-O arm64 objects, `LC_BUILD_VERSION
platform ios minos 17.0`, 31 exported `uniffi_galaxy_prover_*` symbols),
linked by Xcode on the Mac.

## Decision
- Backend v1: **circom-witnesscalc graph witness + arkworks 0.6 Groth16**, in
  `crates/galaxy-prover`, pure Rust, built on Linux.
- Proving key format: the **snarkjs `.zkey` itself, memory-mapped**. No
  converted artifact: the bytes the phone reads are exactly the bytes hashed
  into the circuit descriptor's `zkey_sha256`. Header, VK (on-curve +
  subgroup checks) and coefficient indices are validated at open; query points
  are decoded from Montgomery form without copies of the whole section
  (`ProverConfig::msm_chunk`), so the proving key lives in clean file-backed
  pages that do not count towards iOS `phys_footprint`.
- R1CS→QAP: circom/snarkjs reduction (A·w, B·w from zkey section 4 including
  the public-input rows, C = A∘B, odd-coset evaluation of H).
- Every proof is self-verified with the zkey VK before release; errors are
  typed (`ProverError`), cancellation is checked between phases and MSM chunks,
  progress is reported per phase.
- The R1CS used for the Groth16 setup must come from the same `build-circuit
  --r1cs` run as the graph (identical wire order); `build-circuit` is
  installed at a pinned revision by `scripts/prover-install-build-circuit.sh`.
- UniFFI surface behind feature `uniffi` (re-exported by galaxy-ffi).

## Measurements
(Filled from real runs; see below. Desktop: Linux x86_64, 8 CPUs, shared with
other agents — timings are upper bounds.)

## Consequences
- No C/C++ in the proving stack; reproducible Linux builds; no JIT.
- The zkey is shipped with the app or pushed to its container (size budget in
  Measurements). O2 R1CS rows of Poseidon-heavy circuits are dense; zkey size
  is dominated by the coefficient section.
- rapidsnark remains the documented fallback for latency.
