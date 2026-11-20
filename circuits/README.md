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
| `tests/core/` | core component tests incl. constraint-level negative tests (ADR 0042) | CircuitCore |
| `tests/geometry/` | geometry predicate tests | CircuitGeo |
| `artifacts/v1m/` | build/setup outputs (small public files committed, large ones gitignored) | CircuitCore |

## Commands (`scripts/circuit.sh`, also `make circuit-*`)

| command | does |
|---|---|
| `build` | heavy slot; `circom --O2 --inspect --r1cs --wasm --sym` plus the circom-witnesscalc graph (`.tools/bin/build-circuit`); fails unless both compilers emit the identical R1CS; writes `build.json` (source tree, r1cs, wasm, graph digests) and `r1cs-info.json` |
| `test` | core tests (`node --test tests/core/core.test.mjs`) and geometry tests |
| `constraints` | heavy slot; compiles every production template alone with `--O2` and prints its constraint count (input for [CONSTRAINTS.md](CONSTRAINTS.md)) |
| `witness` | synthetic passing room capture → `galaxy_witness::build_input` → WASM witness (all assertions) → circom-witnesscalc graph witness, equal wire by wire → every R1CS constraint checked in Rust → public wires equal to `public_inputs_v1` reconstructed from the photo and the ledger statement (no setup needed) |
| `setup-dev` | verifies the pinned ptau (circuits/ptau.lock.json), runs `galaxy-prover dev-setup` (development single-party phase 2, ADR 0043), exports `verification_key.json`, then `export` |
| `export` | VK bytes (galaxy-crypto codec, cross-checked against `galaxy-prover vk`), canonical descriptor, `circuit_id.hex`, GXCB `circuit_bundle.bin`, manifest `artifacts/v1m.json` |
| `e2e` | synthetic passing room capture → `galaxy_witness::build_input` → WASM witness (all assertions) → `galaxy-prover prove` (graph witness) → `snarkjs groth16 verify` and galaxy-crypto verify with public inputs reconstructed from the photo and ledger statement |
| `clean` | removes `artifacts/v1m/`, `build/`, the manifest |

Compiler flags are fixed: `--O2 --prime bn128`. Compile and setup need several GiB:
always through `scripts/with-slot.sh --heavy` (the script does this).

## Artifacts (`artifacts/v1m/`)

Committed: `build.json`, `r1cs-info.json`, `setup.json`, `setup-transcript.log`,
`inspect.log`, `verification_key.json`, `verification_key.bin`, `descriptor.json`,
`circuit_id.hex`, `circuit_bundle.bin`, and `artifacts/v1m.json`.
Gitignored (digests pinned in `descriptor.json`): `main_v1m.r1cs`, `main_v1m.sym`,
`main_v1m_js/`, `main_v1m.graph.bin`, `main_v1m.zkey`.

The current key comes from a **development single-party** phase 2 and is not
production-trusted; the production ceremony procedure is in
docs/adr/0043-groth16-setup-ceremony.md.

Current development key (policy_id `ef744a7f…afac`, 876,639 constraints):
circuit_id `bfdc24d683e594640a3059ac83f4717fdedcfaef7f061a9093b783aada3b08ea`,
vk_digest `3150a6a6540b384b141bfd11738d954e22b8e82b8cb145de1abfee19b4e81b74`,
zkey sha256 `5cd0ec2cc484e612ad1a93b7584ca70d8e7e30de15eb3286eced9f86404d8cdd`
(831,198,820 B), GXCB bundle 4,284 B. `galaxy-prover dev-setup` took 1,275 s on
the shared build machine (RAYON_NUM_THREADS=4, peak RSS ≈ 1 GiB); a desktop proof
takes ≈ 11–14 s (witness 0.4–0.5 s).
