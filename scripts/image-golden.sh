#!/usr/bin/env bash
# Cross-target determinism check for galaxy-image / galaxy-evidence golden vectors (spec §4).
#
#   scripts/image-golden.sh [linux] [wasm] [mac]     (default: all three)
#
# linux: x86_64-unknown-linux-gnu, native test harness.
# wasm:  wasm32-unknown-unknown under node via .tools/bin/wasm-bindgen-test-runner.
# mac:   aarch64-apple-darwin on the Mac build host (scripts/lib-mac.sh). Only the Rust
#        workspace sources and test-vectors/ are mirrored into $MAC_MIRROR/image-golden
#        (never fixtures/), built with the Mac's installed stable toolchain, and the target
#        directory is deleted afterwards (spec §19 disk budget).
# Every target runs the same tests against the same committed JSON vectors
# (test-vectors/image/jpeg-golden.json, test-vectors/evidence/synthetic-v1.json); a pass on a
# target means every decoded-raster hash, encoded-file hash, photo_id and root matched.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-target/image-golden}"
TESTS=(-p galaxy-image --test golden -p galaxy-evidence --test evidence)

# zune-jpeg selects AVX2/NEON paths from its `x86`/`neon` Cargo features; DecoderOptions has no
# runtime switch for them. Feature unification from any workspace crate would silently change
# the reference decoder, so the scalar-only feature set is checked before every run.
check_decoder_features() {
    local features
    features="$(cargo tree --workspace --target all -e features -i zune-jpeg --prefix none \
        | grep -E '^zune-jpeg feature' || true)"
    if [ -n "$features" ]; then
        echo "zune-jpeg features enabled in the workspace (scalar-only required):" >&2
        echo "$features" >&2
        exit 1
    fi
    echo "zune-jpeg: no optional features enabled (scalar IDCT, upsampling and colour conversion)"
}

run_linux() {
    echo "== x86_64-unknown-linux-gnu"
    CARGO_TARGET_DIR="$TARGET_DIR" scripts/with-slot.sh cargo test --release "${TESTS[@]}"
}

run_wasm() {
    echo "== wasm32-unknown-unknown (node)"
    CARGO_TARGET_DIR="$TARGET_DIR" \
        CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="$ROOT/.tools/bin/wasm-bindgen-test-runner" \
        scripts/with-slot.sh cargo test --release --target wasm32-unknown-unknown "${TESTS[@]}"
}

run_mac() {
    echo "== aarch64-apple-darwin (Mac build host)"
    # shellcheck source=lib-mac.sh
    . "$ROOT/scripts/lib-mac.sh"
    local base="$MAC_MIRROR"
    MAC_MIRROR="$base/image-golden"
    mac_ssh 'df -h ~ | tail -1'
    mac_rsync . \
        --include='/Cargo.toml' --include='/Cargo.lock' \
        --include='/crates/***' --include='/test-vectors/***' --exclude='*'
    local dir
    dir="$(printf '%q' "$MAC_MIRROR")"
    local status=0
    mac_ssh "cd $dir && rustc --version && cargo +stable test --locked --release --target aarch64-apple-darwin ${TESTS[*]}" || status=$?
    mac_ssh "rm -rf $dir/target"
    MAC_MIRROR="$base"
    return "$status"
}

targets=("$@")
[ "${#targets[@]}" -eq 0 ] && targets=(linux wasm mac)
check_decoder_features
for t in "${targets[@]}"; do
    case "$t" in
        linux) run_linux ;;
        wasm) run_wasm ;;
        mac) run_mac ;;
        *) echo "unknown target: $t (linux | wasm | mac)" >&2; exit 2 ;;
    esac
done
