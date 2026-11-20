#!/usr/bin/env bash
# Generates test-vectors/protocol/groth16/ (spec §9, §16): a real snarkjs Groth16 proof for a
# tiny circom circuit (single-party development setup in a temporary directory, local
# power-6 ptau), the independently encoded §9 proof/VK bytes, and negative cases.
# Consumed by crates/galaxy-crypto/tests/groth16.rs. Not used for any production key.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SNARKJS="$ROOT/circuits/node_modules/.bin/snarkjs"
CIRCOM="$ROOT/.tools/bin/circom"
OUT="$ROOT/test-vectors/protocol/groth16"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

cat > "$WORK/tiny.circom" <<'EOF'
pragma circom 2.1.0;

// x * y + z == out, with three public inputs (y, z, out) and private x.
template Tiny() {
    signal input x;
    signal input y;
    signal input z;
    signal input out;
    signal xy;
    xy <== x * y;
    out === xy + z;
}

component main {public [y, z, out]} = Tiny();
EOF
echo '{"x": "7", "y": "11", "z": "5", "out": "82"}' > "$WORK/input.json"

cd "$WORK"
"$CIRCOM" tiny.circom --r1cs --wasm -o . > /dev/null
"$SNARKJS" powersoftau new bn128 6 pot_0.ptau > /dev/null
"$SNARKJS" powersoftau contribute pot_0.ptau pot_1.ptau --name=dev -e="$(head -c 32 /dev/urandom | base64)" > /dev/null
"$SNARKJS" powersoftau prepare phase2 pot_1.ptau pot.ptau > /dev/null
"$SNARKJS" groth16 setup tiny.r1cs pot.ptau tiny_0.zkey > /dev/null
"$SNARKJS" zkey contribute tiny_0.zkey tiny.zkey --name=dev -e="$(head -c 32 /dev/urandom | base64)" > /dev/null
"$SNARKJS" zkey export verificationkey tiny.zkey verification_key.json > /dev/null
"$SNARKJS" groth16 fullprove input.json tiny_js/tiny.wasm tiny.zkey proof.json public.json > /dev/null
"$SNARKJS" groth16 verify verification_key.json public.json proof.json

cp tiny.circom verification_key.json proof.json public.json "$OUT/"
node "$ROOT/scripts/protocol-groth16-vectors.mjs" "$OUT"
echo "wrote $OUT"
