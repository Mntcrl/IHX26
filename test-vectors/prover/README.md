# galaxy-prover test fixture

DEVELOPMENT material only: a tiny circuit with a single-party Groth16 setup.
It is never an accepted circuit and proves nothing about photos. Used by
`cargo test -p galaxy-prover` (crates/galaxy-prover/tests/roundtrip.rs).

| file | content |
|---|---|
| `fixture.circom` | 22 public inputs (`pub_in`, like galaxy-pc-v1m), Poseidon(2), Num2Bits(64), 44 products |
| `fixture.graph.bin` | circom-witnesscalc graph (`build-circuit`, circom 2.2.3 ad44e915, O2) |
| `fixture.zkey` | snarkjs 0.7.6 Groth16 zkey, setup on the R1CS emitted by the same `build-circuit` run, ptau `powersOfTau28_hez_final_20.ptau`, one random development contribution |
| `vk.json` | `snarkjs zkey export verificationkey` |
| `vk.bin` | spec §9 VK bytes (`galaxy-prover vk`) |
| `input.json` | inputs (deterministic SHA-256 derived values) |
| `snarkjs_proof.json`, `snarkjs_public.json` | proof made by `snarkjs groth16 fullprove` with the circom wasm witness (independent of galaxy-prover) |
| `setup.txt` | provenance and sha256 of every file (manifest) |

Regenerate with `scripts/prover-build-fixture.sh` (new random contribution ⇒
new zkey, vk and proofs; commit all files together).

Pinned sha256 of the committed fixture (see `setup.txt`):

```
fixture.zkey      213957 bytes fdd79342027ddb5c16edda2113c434e2bcd638e25ecd61b84b64fd33d87a1625
fixture.graph.bin  26690 bytes 19fe900dd05bc36e630e6c2e43e1423710ef981427ff7afaa2c3b5a39886043a
vk.bin              1926 bytes a2ae94f12554a1170b1333d8c078d2862c6506e4c5a3ee6c54237743a92c509c
```
