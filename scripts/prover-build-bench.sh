#!/usr/bin/env bash
# Builds a prover benchmark circuit (apps/prover-bench-ios/circuit/prover_bench.circom)
# with STEPS chain steps and a DEVELOPMENT single-party Groth16 setup.
#
#   scripts/prover-build-bench.sh <name> <steps> [ptau]
#
# Outputs in apps/prover-bench-ios/artifacts/<name>/ (gitignored, large):
#   main.circom, <name>.r1cs (from build-circuit, same compilation as the graph),
#   <name>.graph.bin (circom-witnesscalc graph), <name>.zkey, vk.json, input.json,
#   setup.txt (labelled development ceremony, sha256 of every artifact).
#
# Requirements: .tools/bin/circom (scaffold), .tools/bin/build-circuit
# (scripts/prover-install-build-circuit.sh), circuits/node_modules (snarkjs, circomlib).
# Memory: snarkjs setup for 2^20 needs several GiB; the script refuses to start
# with less than 6 GiB available.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="${1:?name}"
STEPS="${2:?steps}"
PTAU="${3:-$ROOT/.tools/ptau/powersOfTau28_hez_final_20.ptau}"
OUT="${OUT_DIR:-$ROOT/apps/prover-bench-ios/artifacts/$NAME}"
SNARKJS="$ROOT/circuits/node_modules/.bin/snarkjs"
BUILD_CIRCUIT="${BUILD_CIRCUIT:-$ROOT/.tools/bin/build-circuit}"

for t in "$SNARKJS" "$BUILD_CIRCUIT" "$PTAU"; do
    [ -e "$t" ] || { echo "missing $t" >&2; exit 1; }
done

avail_gib=$(awk '/MemAvailable/ {printf "%d", $2/1048576}' /proc/meminfo)
if [ "$avail_gib" -lt 6 ]; then
    echo "only ${avail_gib} GiB available; need >= 6 GiB for the setup" >&2
    exit 1
fi

mkdir -p "$OUT"
cd "$OUT"
cat > main.circom <<EOF
pragma circom 2.2.0;
include "prover_bench.circom";
component main {public [pub_in]} = ProverBench($STEPS);
EOF

echo "== build-circuit (graph + r1cs, circom O2)"
"$ROOT/scripts/with-slot.sh" --heavy "$ROOT/scripts/prover-time.py" "$BUILD_CIRCUIT" main.circom "$NAME.graph.bin" \
    -l "$ROOT/apps/prover-bench-ios/circuit" -l "$ROOT/circuits/node_modules" \
    --r1cs "$NAME.r1cs" 2> build-circuit.time.txt >/dev/null
grep -E "Elapsed|Maximum resident" build-circuit.time.txt

export NODE_OPTIONS="--max-old-space-size=${NODE_HEAP_MB:-8192}"
"$SNARKJS" r1cs info "$NAME.r1cs" | tee r1cs-info.txt

echo "== groth16 setup (DEVELOPMENT, single party)"
"$ROOT/scripts/with-slot.sh" --heavy "$ROOT/scripts/prover-time.py" "$SNARKJS" groth16 setup "$NAME.r1cs" "$PTAU" "${NAME}_0000.zkey" \
    2> setup.time.txt >/dev/null
grep -E "Elapsed|Maximum resident" setup.time.txt
entropy="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
"$ROOT/scripts/with-slot.sh" --heavy "$SNARKJS" zkey contribute "${NAME}_0000.zkey" "$NAME.zkey" \
    --name="galaxy prover bench development contribution" -e="$entropy" >/dev/null
unset entropy
rm -f "${NAME}_0000.zkey"
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
    echo "ceremony: development-single-party (NOT for production)"
    echo "circuit: prover_bench.circom ProverBench($STEPS)"
    echo "circom: $("$ROOT/.tools/bin/circom" --version) (build-circuit embeds circom $(cat "$ROOT/.tools/build-circuit.version" 2>/dev/null || echo unknown))"
    echo "ptau: $(basename "$PTAU")"
    sha256sum "$PTAU" | awk '{print "ptau_sha256: " $1}'
    for f in main.circom "$NAME.r1cs" "$NAME.graph.bin" "$NAME.zkey" vk.json input.json; do
        printf '%s %s bytes sha256 %s\n' "$f" "$(stat -c %s "$f")" "$(sha256sum "$f" | cut -d' ' -f1)"
    done
} > setup.txt
rm -f log_input_signals.txt log_input_signals_new.txt
cat setup.txt
