# ADR 0043 — Groth16 setup: development key now, multi-party ceremony for production

Status: accepted (CircuitCore)

## Context
Groth16 needs a circuit-specific structured reference string. Whoever knows its
trapdoor (tau, alpha, beta, gamma, delta) can produce proofs that verify for false
statements, so the soundness of every accepted proof depends on that trapdoor being
destroyed. See `circuits/CONSTRAINTS.md` for the current circuit size and domain.
`snarkjs groth16 setup` loads the whole R1CS as JS objects and needs about 10 GB for
this circuit; the shared 13 GB build machine cannot run it next to other jobs.

## Decision
1. **Development key (`ceremony: "development-single-party"`)**. `scripts/circuit.sh
   setup-dev` verifies the pinned Hermez phase-1 file `powersOfTau28_hez_final_20.ptau`
   (size, SHA-256, published BLAKE2b from circuits/ptau.lock.json) and runs
   `galaxy-prover dev-setup --ptau … --ptau-sha256 …` (crates/galaxy-prover/src/dev_setup.rs,
   MobileProver). Like `snarkjs zkey new` it consumes the phase-1 Lagrange powers
   (so tau stays the multi-party Hermez tau); phase 2 is a single party: delta is
   sampled from the OS RNG inside the process and dropped, gamma is the generator as
   in snarkjs. The descriptor records the real `ptau_name`/`ptau_sha256`.
   `setup-transcript.log` records the tool, the SHA-256 of its sources
   (`crates/galaxy-prover/src/**`, its Cargo.toml, Cargo.lock) and binary, the ptau,
   r1cs and zkey digests and the exported `verification_key.json` digest; its SHA-256
   is the descriptor's `setup_transcript_sha256`.
2. The VK is exported twice independently — snarkjs `zkey export verificationkey`
   → galaxy-crypto `VerifyingKey::from_snarkjs_json` → VK bytes, and
   `galaxy-prover vk` reading the zkey directly — and `export` fails unless the
   bytes are identical. `scripts/circuit.sh e2e` proves a synthetic passing capture
   and verifies with snarkjs and galaxy-crypto.
3. **Production** requires a multi-party phase-2 ceremony on a machine with
   ≥ 16 GB RAM, starting from the pinned, checksum-verified Hermez phase-1 file
   `powersOfTau28_hez_final_20.ptau` (`circuits/ptau.lock.json`; powers must cover
   the full R1CS count plus 22 public inputs and one constant wire):
   - coordinator: `snarkjs groth16 setup main_v1m.r1cs powersOfTau28_hez_final_20.ptau c0000.zkey`
     (r1cs digest must equal the descriptor's `r1cs_sha256`);
   - each of N ≥ 3 independent participants, in sequence:
     `snarkjs zkey contribute c<i>.zkey c<i+1>.zkey --name=<participant>` with
     private entropy, publishing the contribution hash;
   - final public randomness: `snarkjs zkey beacon c<N>.zkey main_v1m.zkey <hash of a
     future XRPL ledger> 10`;
   - anyone verifies: `snarkjs zkey verify main_v1m.r1cs powersOfTau28_hez_final_20.ptau
     main_v1m.zkey` (checks the full contribution chain against the ptau) and compares
     the printed contribution hashes with the published ones;
   - descriptor: `ceremony: "groth16-phase2-multi-party"`, real `ptau_name`/`ptau_sha256`,
     `setup_transcript_sha256` = SHA-256 of the `zkey verify` output. The new
     circuit_id is registered on XRPL as a new CIRCUIT bundle; the development
     circuit_id is never configured as accepted outside development.

## Consequences
- Proofs under the development key are sound only if the single process that
  sampled delta did not leak it (knowing delta suffices to produce proofs of false
  statements even with an honest phase 1); verifiers surface the `ceremony` value
  (galaxy-verifier report) and the key must not be an accepted production circuit.
- The development zkey carries no phase-2 contribution record. `snarkjs zkey
  verify` is not a ceremony attestation for this key. The development checks
  compare VK exports and verify a real proof under both verifiers; they do not
  establish production trust.
- A production ceremony changes zkey, VK, vk_digest and circuit_id but not the
  r1cs, wasm, witness graph or policy.
