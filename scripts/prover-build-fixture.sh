#!/usr/bin/env bash
# Regenerates the galaxy-prover test fixture in test-vectors/prover/ from
# test-vectors/prover/fixture.circom (DEVELOPMENT single-party setup):
#   fixture.graph.bin  circom-witnesscalc graph (build-circuit)
#   fixture.zkey       snarkjs Groth16 proving key (setup on the build-circuit R1CS)
#   vk.json            snarkjs verification key
#   vk.bin             spec §9 VK bytes (galaxy-prover vk)
#   input.json         circuit inputs
#   snarkjs_proof.json / snarkjs_public.json  proof made by snarkjs (wasm witness)
#   setup.txt          provenance + sha256 of every file
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/test-vectors/prover"
PTAU="${PTAU:-$ROOT/.tools/ptau/powersOfTau28_hez_final_20.ptau}"
SNARKJS="$ROOT/circuits/node_modules/.bin/snarkjs"
CIRCOM="$ROOT/.tools/bin/circom"
BUILD_CIRCUIT="${BUILD_CIRCUIT:-$ROOT/.tools/bin/build-circuit}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cd "$DIR"

"$BUILD_CIRCUIT" fixture.circom fixture.graph.bin -l "$ROOT/circuits/node_modules" \
    --r1cs "$TMP/fixture.r1cs" >/dev/null
"$SNARKJS" groth16 setup "$TMP/fixture.r1cs" "$PTAU" "$TMP/f0.zkey" >/dev/null
"$SNARKJS" zkey contribute "$TMP/f0.zkey" fixture.zkey --name="galaxy-prover fixture (development)" \
    -e="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" >/dev/null
"$SNARKJS" zkey export verificationkey fixture.zkey vk.json >/dev/null

python3 - > input.json <<'PY'
import hashlib, json
R = 21888242871839275222246405745257275088548364400416034343698204186575808495617
h = lambda s: int.from_bytes(hashlib.sha256(s.encode()).digest(), "big")
print(json.dumps({
    "pub_in": [str(h(f"fixture/pub/{i}") % R) for i in range(22)],
    "secret": [str(h("fixture/s0") % (1 << 64)), str(h("fixture/s1") % R)],
}))
PY

# Independent witness (circom wasm) + snarkjs proof for the cross-check tests.
"$CIRCOM" fixture.circom --O2 --wasm -l "$ROOT/circuits/node_modules" -o "$TMP" >/dev/null
"$SNARKJS" groth16 fullprove input.json "$TMP/fixture_js/fixture.wasm" fixture.zkey \
    snarkjs_proof.json snarkjs_public.json >/dev/null
"$SNARKJS" groth16 verify vk.json snarkjs_public.json snarkjs_proof.json

(cd "$ROOT" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/MobileProver}" \
    "$ROOT/scripts/with-slot.sh" cargo run -q --release -p galaxy-prover --bin galaxy-prover -- vk --zkey "$DIR/fixture.zkey" --out "$DIR/vk.bin")

{
    echo "ceremony: development-single-party (test fixture, NOT for production)"
    echo "circom: $("$CIRCOM" --version); build-circuit: $(cat "$ROOT/.tools/build-circuit.version" 2>/dev/null || echo unknown)"
    echo "ptau: $(basename "$PTAU") sha256 $(sha256sum "$PTAU" | cut -d' ' -f1)"
    echo "r1cs: from build-circuit --r1cs, sha256 $(sha256sum "$TMP/fixture.r1cs" | cut -d' ' -f1)"
    for f in fixture.circom fixture.graph.bin fixture.zkey vk.json vk.bin input.json snarkjs_proof.json snarkjs_public.json; do
        printf '%s %s bytes sha256 %s\n' "$f" "$(stat -c %s "$f")" "$(sha256sum "$f" | cut -d' ' -f1)"
    done
} > setup.txt
rm -f log_input_signals.txt log_input_signals_new.txt
cat setup.txt
