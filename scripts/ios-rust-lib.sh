#!/usr/bin/env bash
# Linux -> iOS device static library plus host-generated UniFFI bindings.
# Configure the matched policy, network allowlist and circuit descriptor before building.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
rev=$(git rev-parse HEAD)
dirty=$(git status --porcelain --untracked-files=no)
TARGET=aarch64-apple-ios
OUT="$ROOT/bindings/swift/GalaxyCore"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/GalaxyCore}"
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR="$(realpath -e "$CARGO_TARGET_DIR")"
export GALAXY_IOS_POLICY="$(realpath -e "${GALAXY_IOS_POLICY:-policies/galaxy-pc-v1m.json}")"
export GALAXY_IOS_NETWORK="$(realpath -e "${GALAXY_IOS_NETWORK:-deployments/networks/xrpl-testnet.toml}")"
DESC="$(realpath -e "${GALAXY_IOS_DESCRIPTOR:-circuits/artifacts/v1m/descriptor.json}")"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3

# Detect edits during the host/target build so their embedded pins cannot diverge.
GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
sha256sum "$GALAXY_IOS_POLICY" "$GALAXY_IOS_NETWORK" "$DESC" > "$GEN/inputs.sha256"

scripts/with-slot.sh cargo build --locked -p galaxy-ffi --release --features uniffi-cli --bin galaxy-ffi-bindgen
BINDGEN="$CARGO_TARGET_DIR/release/galaxy-ffi-bindgen"
CIRCUIT="$("$BINDGEN" check-circuit "$DESC")"
echo "== Accepted CIRCUIT registration: $CIRCUIT"

rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
STUB="$CARGO_TARGET_DIR/ios-bindgen-stub"
mkdir -p "$STUB"
printf '#pragma once\nvoid free(void *);\n' > "$STUB/stdlib.h"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_ios="-I$STUB"
# circom-witnesscalc declares an unused cdylib as well as its rlib. The shim
# rejects every other link request; Xcode performs the eventual application link.
export CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$ROOT/scripts/prover-ios-nolink.sh"

scripts/with-slot.sh cargo rustc --locked -p galaxy-ffi --lib --release --target "$TARGET" --crate-type staticlib
scripts/with-slot.sh cargo rustc --locked -p galaxy-ffi --lib --release --crate-type cdylib
"$BINDGEN" generate --library "$CARGO_TARGET_DIR/release/libgalaxy_ffi.so" --language swift --out-dir "$GEN" --no-format
sha256sum --check --status "$GEN/inputs.sha256" || { echo 'iOS build inputs changed during build' >&2; exit 1; }

mkdir -p "$OUT/swift" "$OUT/include" "$OUT/ios-arm64"
cp "$GEN/galaxy_ffi.swift" "$OUT/swift/"
cp "$GEN/galaxy_ffiFFI.h" "$OUT/include/"
cp "$GEN/galaxy_ffiFFI.modulemap" "$OUT/include/module.modulemap"
cp "$CARGO_TARGET_DIR/$TARGET/release/libgalaxy_ffi.a" "$OUT/ios-arm64/"

{
    echo "git: $rev"
    if [[ -n "$dirty" ]]; then echo 'worktree: dirty'; fi
    echo "rustc: $(rustc --version)"
    echo "cargo: $(cargo --version)"
    echo "target: $TARGET IPHONEOS_DEPLOYMENT_TARGET=$IPHONEOS_DEPLOYMENT_TARGET lto=fat codegen-units=1"
    echo "circuit registration tx and ceremony: $CIRCUIT"
    awk 'NR==1 {print $1 "  build input (policy JSON)"} NR==2 {print $1 "  build input (network TOML)"} NR==3 {print $1 "  build input (descriptor JSON)"}' "$GEN/inputs.sha256"
    (cd "$OUT" && sha256sum ios-arm64/libgalaxy_ffi.a swift/galaxy_ffi.swift include/galaxy_ffiFFI.h include/module.modulemap)
} > "$OUT/BUILD-INFO.txt"
cat "$OUT/BUILD-INFO.txt"
