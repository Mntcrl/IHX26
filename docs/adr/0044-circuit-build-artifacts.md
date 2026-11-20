# ADR 0044 — Circuit build pipeline, artifact digests and the circuit bundle

Status: accepted (CircuitCore)

## Context
A circuit_id (§9) must bind everything that determines which statements a proof
can establish: the circuit sources and policy constants, the compiled constraint
system, the two witness generators (WASM for desktop/browser tooling, the
circom-witnesscalc graph for iOS), the proving/verifying key and how the key was
produced. The verifier only ever sees the descriptor, VK bytes and policy JSON in the
GXCB bundle registered on XRPL (§10), so the descriptor's digests must be
reproducible by anyone from the repository.

## Decision
- `scripts/circuit.sh build` compiles `main_v1m.circom` with fixed flags
  (`circom 2.2.3 --prime bn128 --O2 --inspect --r1cs --wasm --sym`) and with
  `build-circuit` (iden3 circom-witnesscalc 0.3.0, `--O2`) into the witness graph.
  The build fails unless both compilers emit byte-identical R1CS files; the graph
  compiler's R1CS is the one kept, so the proving key, the WASM witness order and the
  graph witness order agree by construction.
- `source_sha256` = SHA-256 of the `sha256sum` listing (sorted paths) of every
  `.circom` file under circuits/components/core, circuits/components/geometry,
  circuits/physical-consistency and circomlib's `circuits/`, plus
  circuits/package-lock.json, policies/galaxy-pc-v1m.json and
  scripts/circuit-gen-policy.mjs. It is computed before and after compiling; a change
  in between fails the build. `node scripts/circuit-gen-policy.mjs --check` must pass
  first (generated constants equal the policy file).
- `build.json` pins source, r1cs, wasm and graph digests; setup and export refuse to
  run on stale outputs. `export` builds the canonical descriptor (§9 field set,
  canonical JSON), VK bytes (galaxy-crypto codec; identical bytes from
  `galaxy-prover vk` are required), `circuit_id = SHA256("GALAXY/CIRCUIT/v1\0" ‖
  descriptor)`, and the GXCB bundle (descriptor ‖ VK bytes ‖ canonical policy JSON),
  checking that the bundle and VK decode round-trip.
- `scripts/circuit.sh witness` (and the first half of `e2e`) checks the build on a
  passing synthetic capture: the WASM witness (all circuit assertions) and the graph
  witness must be equal wire by wire, every R1CS constraint is evaluated in Rust
  (galaxy-witness `circuit_tool check-witness`, which holds only the R1CS file and
  the witness in memory, unlike `snarkjs wtns check`), and public wires 1..22 must
  equal `public_inputs_v1` reconstructed from the photo and the ledger statement.
- Small public artifacts (descriptor, VK JSON/bytes, bundle, circuit_id, transcripts,
  build/setup digests, `artifacts/v1m.json` manifest) are committed. R1CS, sym, WASM,
  witness graph and zkey are gitignored; anyone can rebuild them and compare digests.

## Consequences
- Any policy change (new policy_id) requires regenerating the include, recompiling,
  a new setup and a new circuit_id; the descriptor binds the policy_id.
- Byte-identical rebuilds require the pinned circom and build-circuit versions and
  the pinned `circuits/package-lock.json` (circomlib 2.0.5).
- The zkey digest is in the descriptor, so provers can check their downloaded key
  against the registered circuit before proving.
