#!/usr/bin/env bash
# Installs iden3 circom-witnesscalc `build-circuit` (circom -> witness graph) into
# .tools/bin at a pinned commit. Its Cargo.lock pins the embedded circom compiler
# to iden3/circom ad44e915 (= tag v2.2.3, the same commit as .tools/bin/circom),
# so graphs and R1CS files come from the same compiler.
# The repository is cloned without submodules (its test_deps submodules are
# large and unused); `cargo install --git` would fetch them.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REV=d48eb7c97857d46b8a75c94ab96f769207263245 # tag v0.3.0
SRC="$(mktemp -d)"
trap 'rm -rf "$SRC"' EXIT
git -c advice.detachedHead=false clone -q https://github.com/iden3/circom-witnesscalc "$SRC"
git -C "$SRC" checkout -q "$REV"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" CARGO_TARGET_DIR="$SRC/target" "$ROOT/scripts/with-slot.sh" cargo install --locked \
    --path "$SRC/extensions/build-circuit" --root "$ROOT/.tools"
echo "circom-witnesscalc $REV (v0.3.0; embedded circom ad44e915 = v2.2.3)" > "$ROOT/.tools/build-circuit.version"
"$ROOT/.tools/bin/build-circuit" --help | head -2
