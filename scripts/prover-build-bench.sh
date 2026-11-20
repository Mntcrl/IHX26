#!/usr/bin/env bash
# Builds a prover benchmark circuit (apps/prover-bench-ios/circuit/prover_bench.circom)
# with STEPS chain steps and a DEVELOPMENT single-party Groth16 setup
# (`galaxy-prover dev-setup`: verified phase-1 ptau + one phase-2 contribution).
#
#   scripts/prover-build-bench.sh <name> <steps> [ptau]
#
# Outputs in apps/prover-bench-ios/artifacts/<name>/ (gitignored, large):
#   main.circom, <name>.r1cs (from build-circuit, same compilation as the graph),
#   <name>.graph.bin (circom-witnesscalc graph), <name>.zkey, vk.json, input.json,
#   setup.txt (labelled development ceremony, sha256 of every artifact).
# An existing r1cs + graph compiled from an identical main.circom is reused.
#
# Requirements: .tools/bin/circom (scaffold), .tools/bin/build-circuit
# (scripts/prover-install-build-circuit.sh), circuits/node_modules (snarkjs, circomlib),
# a ptau listed in circuits/ptau.lock.json (its pinned SHA-256 is checked by dev-setup;
# the domain 2^(floor(log2(constraints + 22)) + 1) must not exceed its power).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="${1:?name}"
STEPS="${2:?steps}"
PTAU="${3:-$ROOT/.tools/ptau/powersOfTau28_hez_final_20.ptau}"
OUT="${OUT_DIR:-$ROOT/apps/prover-bench-ios/artifacts/$NAME}"
SNARKJS="$ROOT/circuits/node_modules/.bin/snarkjs"
BUILD_CIRCUIT="${BUILD_CIRCUIT:-$ROOT/.tools/bin/build-circuit}"
PROVER="${PROVER:-$ROOT/target/MobileProver/release/galaxy-prover}"

for t in "$SNARKJS" "$BUILD_CIRCUIT" "$PTAU"; do
    [ -e "$t" ] || { echo "missing $t" >&2; exit 1; }
done
PTAU="$(realpath "$PTAU")"
PTAU_SHA256="$(python3 -c 'import json,sys; print(next(e["sha256"] for e in json.load(open(sys.argv[1])) if e["name"] == sys.argv[2] and e.get("verified")))' \
    "$ROOT/circuits/ptau.lock.json" "$(basename "$PTAU")")"
[ -x "$PROVER" ] || CARGO_TARGET_DIR="$ROOT/target/MobileProver" "$ROOT/scripts/with-slot.sh" \
    cargo build --manifest-path "$ROOT/Cargo.toml" -p galaxy-prover --release --bin galaxy-prover

mkdir -p "$OUT"
cd "$OUT"
main="pragma circom 2.2.0;
include \"prover_bench.circom\";
component main {public [pub_in]} = ProverBench($STEPS);"
if [ -s "$NAME.r1cs" ] && [ -s "$NAME.graph.bin" ] && [ "$(cat main.circom 2>/dev/null)" = "$main" ]; then
    echo "== build-circuit: reusing $NAME.r1cs and $NAME.graph.bin"
else
    rm -f "$NAME.r1cs" "$NAME.graph.bin"
    printf '%s\n' "$main" > main.circom

    echo "== build-circuit (graph + r1cs, circom O2)"
    "$ROOT/scripts/with-slot.sh" --heavy "$ROOT/scripts/prover-time.py" "$BUILD_CIRCUIT" main.circom "$NAME.graph.bin" \
        -l "$ROOT/apps/prover-bench-ios/circuit" -l "$ROOT/circuits/node_modules" \
        --r1cs "$NAME.r1cs" 2> build-circuit.time.txt >/dev/null
    grep -E "Elapsed|Maximum resident" build-circuit.time.txt
fi

export NODE_OPTIONS="--max-old-space-size=${NODE_HEAP_MB:-8192}"
# snarkjs loads the whole R1CS (several GiB at ~1M constraints).
"$ROOT/scripts/with-slot.sh" --heavy "$SNARKJS" r1cs info "$NAME.r1cs" | tee r1cs-info.txt

echo "== groth16 setup (DEVELOPMENT, galaxy-prover dev-setup)"
"$ROOT/scripts/with-slot.sh" --heavy "$ROOT/scripts/prover-time.py" "$PROVER" dev-setup --r1cs "$NAME.r1cs" \
    --ptau "$PTAU" --ptau-sha256 "$PTAU_SHA256" --out "$NAME.zkey" 2> setup.time.txt
grep -E "Elapsed|Maximum resident" setup.time.txt
"$SNARKJS" zkey export verificationkey "$NAME.zkey" vk.json >/dev/null

echo "== input.json (deterministic, seed = name)"
python3 - "$STEPS" "$NAME" > input.json <<'PY'
import hashlib, json, sys
steps, name = int(sys.argv[1]), sys.argv[2]
R = 21888242871839275222246405745257275088548364400416034343698204186575808495617
def h(*parts):
    return int.from_bytes(hashlib.sha256("/".join(map(str, parts)).encode()).digest(), "big")
pub = [str(h(name, "pub", i) % R) for i in range(22)]
priv = [[str(h(name, "priv", s, k) % (1 << 64)) for k in range(4)] for s in range(steps)]
json.dump({"pub_in": pub, "priv_in": priv}, sys.stdout)
PY

{
    echo "ceremony: development-single-party, galaxy-prover dev-setup (NOT for production)"
    echo "circuit: prover_bench.circom ProverBench($STEPS)"
    echo "circom: $("$ROOT/.tools/bin/circom" --version) (build-circuit embeds circom $(cat "$ROOT/.tools/build-circuit.version" 2>/dev/null || echo unknown))"
    echo "ptau: $(basename "$PTAU")"
    echo "ptau_sha256: $PTAU_SHA256 (checked by dev-setup)"
    for f in main.circom "$NAME.r1cs" "$NAME.graph.bin" "$NAME.zkey" vk.json input.json; do
        printf '%s %s bytes sha256 %s\n' "$f" "$(stat -c %s "$f")" "$(sha256sum "$f" | cut -d' ' -f1)"
    done
} > setup.txt
rm -f log_input_signals.txt log_input_signals_new.txt
cat setup.txt
