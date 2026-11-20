#!/usr/bin/env bash
# Reproducible build of the galaxy-prover static library for iOS devices
# (aarch64-apple-ios) on Linux, plus its UniFFI Swift bindings.
#
#   scripts/prover-ios-lib.sh
#
# Outputs (bindings/swift/galaxy-prover/):
#   ios-arm64/libgalaxy_prover.a   static library (gitignored)
#   swift/galaxy_prover.swift      UniFFI Swift bindings
#   include/galaxy_proverFFI.h     C header of the FFI
#   include/module.modulemap       module `galaxy_proverFFI` (SWIFT_INCLUDE_PATHS)
#   BUILD-INFO.txt                 toolchain, git revision, sha256 of the outputs
#
# No Apple SDK is needed: the dependency graph has no C/C++ code for the target.
# The only build-time C parsing is circom-witnesscalc's bindgen run over
# include/graph_witness.h (stddef.h/stdlib.h, one enum + one struct). For the
# iOS target we point bindgen at a two-line stub stdlib.h; the generated layout
# (an int enum and a pointer) is identical for every LP64 aarch64 target.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
TARGET=aarch64-apple-ios
OUT="$ROOT/bindings/swift/galaxy-prover"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/MobileProver}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
# Size/speed for the device library (workspace has no release profile overrides).
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3

rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"

STUB="$CARGO_TARGET_DIR/ios-bindgen-stub"
mkdir -p "$STUB"
printf '#pragma once\nvoid free(void *);\n' > "$STUB/stdlib.h"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_ios="-I$STUB"

echo "== staticlib for $TARGET"
# circom-witnesscalc also declares a cdylib crate-type; that unused dylib cannot be
# linked without an Apple SDK, see scripts/prover-ios-nolink.sh.
export CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$ROOT/scripts/prover-ios-nolink.sh"
"$ROOT/scripts/with-slot.sh" cargo rustc -p galaxy-prover --lib --release --target "$TARGET" --features uniffi --crate-type staticlib

echo "== Swift bindings (host cdylib, same sources and features)"
"$ROOT/scripts/with-slot.sh" cargo rustc -p galaxy-prover --lib --release --features uniffi --crate-type cdylib
"$ROOT/scripts/with-slot.sh" cargo build -p galaxy-prover --release --features uniffi-cli --bin uniffi-bindgen
GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
"$CARGO_TARGET_DIR/release/uniffi-bindgen" generate --library "$CARGO_TARGET_DIR/release/libgalaxy_prover.so" \
    --language swift --out-dir "$GEN" --no-format

rm -rf "$OUT/swift" "$OUT/include" "$OUT/ios-arm64"
mkdir -p "$OUT/swift" "$OUT/include" "$OUT/ios-arm64"
cp "$GEN/galaxy_prover.swift" "$OUT/swift/"
cp "$GEN/galaxy_proverFFI.h" "$OUT/include/"
cp "$GEN/galaxy_proverFFI.modulemap" "$OUT/include/module.modulemap"
cp "$CARGO_TARGET_DIR/$TARGET/release/libgalaxy_prover.a" "$OUT/ios-arm64/"

{
    echo "built: $(date -u +%Y-%m-%dT%H:%M:%SZ) on $(uname -sm)"
    echo "git: $(git rev-parse HEAD 2>/dev/null || echo none)$(git diff --quiet 2>/dev/null || echo ' (dirty)')"
    echo "rustc: $(rustc --version)"
    echo "target: $TARGET IPHONEOS_DEPLOYMENT_TARGET=$IPHONEOS_DEPLOYMENT_TARGET lto=fat codegen-units=1"
    (cd "$OUT" && sha256sum ios-arm64/libgalaxy_prover.a swift/galaxy_prover.swift include/galaxy_proverFFI.h include/module.modulemap)
    stat -c '%n %s bytes' "$OUT/ios-arm64/libgalaxy_prover.a" | sed "s|$OUT/||"
} > "$OUT/BUILD-INFO.txt"
cat "$OUT/BUILD-INFO.txt"
