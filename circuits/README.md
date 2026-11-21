# Circuits — `GalaxyPhysicalConsistencyProofV1` (galaxy-pc-v1m)

Circom 2.2.3 (BN254), circomlib Poseidon, Groth16. Spec: docs/protocol/galaxy-v1-spec.md
§6, §7, §9, §17, §20. Witness input schema: [INPUTS.md](INPUTS.md). Constraint
report: [CONSTRAINTS.md](CONSTRAINTS.md). Geometry predicates: [GEOMETRY.md](GEOMETRY.md).

## Layout

| path | content | owner |
|---|---|---|
| `physical-consistency/main_v1m.circom` | main template: 22 public inputs (§9), no public outputs, `PASS === 1` | CircuitCore |
| `components/core/` | sampling (ctx, slot hashes, strict bit decompositions, picks), leaf unpacking, Merkle openings (arity 4, depth 6), commitments, camera/IMU scalars | CircuitCore |
| `components/geometry/`, `physical-consistency/generated/` | `GalaxyPolicyV1m` predicates, generated policy constants | CircuitGeo |
| `tests/core/` | core component tests, including [constraint-level negative tests](../docs/adr/0042-constraint-level-negative-tests.md) | CircuitCore |
| `tests/geometry/` | geometry predicate tests | CircuitGeo |
| `artifacts/v1m/` | compiled circuit and development setup outputs | CircuitBuild |

## Commands (`scripts/circuit.sh`)

From the repository root, install the pinned circuit dependencies with
`npm ci --prefix circuits` (Node.js 22 or later). The build expects Circom 2.2.3
at `.tools/bin/circom` and the pinned witness-graph compiler installed by
`scripts/prover-install-build-circuit.sh`. Setup also requires the phase-1 file
listed in `ptau.lock.json` under `.tools/ptau/`.

Make aliases are `make circuit-build`, `make circuit-test`, and
`make circuit-setup-dev`.

| command | does |
|---|---|
| `build` | heavy slot; `circom --O2 --inspect --r1cs --wasm --sym` plus the circom-witnesscalc graph (`.tools/bin/build-circuit`); fails unless both compilers emit the identical R1CS; writes `build.json` (source tree, r1cs, wasm, graph digests) and `r1cs-info.json` |
| `test` | core tests (`node --test tests/core/core.test.mjs`) and geometry tests |
| `constraints` | heavy slot; compiles the production components in isolation with `--O2` and prints their constraint counts (input for [CONSTRAINTS.md](CONSTRAINTS.md)) |
| `witness` | synthetic passing room capture → `galaxy_witness::build_input` → WASM witness (all assertions) → circom-witnesscalc graph witness, equal wire by wire → every R1CS constraint checked in Rust → public wires equal to `public_inputs_v1` reconstructed from the photo and the ledger statement (no setup needed) |
| `setup-dev` | verifies the pinned ptau (circuits/ptau.lock.json), runs `galaxy-prover dev-setup` (development single-party phase 2, ADR 0043), exports `verification_key.json`, then `export` |
| `export` | VK bytes (galaxy-crypto codec, cross-checked against `galaxy-prover vk`), canonical descriptor, `circuit_id.hex`, GXCB `circuit_bundle.bin`, manifest `artifacts/v1m.json` |
| `e2e` | synthetic passing room capture → `galaxy_witness::build_input` → WASM witness (all assertions) → `galaxy-prover prove` (graph witness) → `snarkjs groth16 verify` and galaxy-crypto verify with public inputs reconstructed from the photo and ledger statement |
| `clean` | removes `artifacts/v1m/`, `build/`, the manifest |

Compiler flags are fixed: `--O2 --prime bn128`. Compile and setup need several GiB:
always through `scripts/with-slot.sh --heavy` (the script does this).

After changing the policy, run `node scripts/circuit-gen-policy.mjs` before
`build`. Run `constraints` against the rebuilt R1CS, then `setup-dev` and `e2e`.
Compile digests identify the reproducible circuit; fresh development setup
entropy gives each setup a new key and circuit ID.

To run just the core suite, including the invalid-witness tests against the
unmodified harness R1CS:

```sh
PATH="$PWD/.tools/bin:$PATH" scripts/with-slot.sh \
  node --test circuits/tests/core/core.test.mjs
```

## Artifacts (`artifacts/v1m/`)

Tracked public outputs include [build metadata](artifacts/v1m/build.json),
R1CS metadata, compiler logs, [setup metadata](artifacts/v1m/setup.json), the
[setup transcript](artifacts/v1m/setup-transcript.log), VK JSON and bytes, the
[descriptor](artifacts/v1m/descriptor.json), the [circuit ID](artifacts/v1m/circuit_id.hex),
and the [GXCB bundle](artifacts/v1m/circuit_bundle.bin). The
[artifact manifest](artifacts/v1m.json) indexes the files. The R1CS, symbols, WASM
calculator, witness graph, and proving key stay gitignored.

The current key comes from a **development single-party** phase 2 and is not
production-trusted. See the [ceremony ADR](../docs/adr/0043-groth16-setup-ceremony.md)
for production requirements. The descriptor is the source of truth for the current
policy ID, VK digest, and compile/setup digests; the constraint report gives the
current circuit size.
