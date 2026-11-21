#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
root=$PWD
app=$root/apps/verifier-web
pkg=$root/bindings/wasm/pkg
wasm_target=wasm32-unknown-unknown
bindgen_version=0.2.130
export PATH="$root/.tools/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/VerifierWeb}"
[[ $CARGO_TARGET_DIR = /* ]] || export CARGO_TARGET_DIR="$root/$CARGO_TARGET_DIR"
slot=$root/scripts/with-slot.sh

if [[ -n ${GALAXY_WEB_NETWORK_CONFIG:-} && $GALAXY_WEB_NETWORK_CONFIG != /* ]]; then
    export GALAXY_WEB_NETWORK_CONFIG="$root/$GALAXY_WEB_NETWORK_CONFIG"
fi

build_wasm() {
    if [[ $(wasm-bindgen --version 2>/dev/null || :) != "wasm-bindgen $bindgen_version" ]]; then
        echo "Install wasm-bindgen-cli $bindgen_version (see docs/verifier-web.md)." >&2
        exit 2
    fi
    "$slot" cargo build --locked -p galaxy-wasm --lib --target "$wasm_target" --release
    wasm-bindgen --target web --out-dir "$pkg" --out-name galaxy_wasm \
        "$CARGO_TARGET_DIR/$wasm_target/release/galaxy_wasm.wasm"
    # Explicitly mark the same browser package as ESM for the Node parity test.
    printf '{"type":"module","private":true}\n' > "$pkg/package.json"
}

install_js() {
    "$slot" npm --prefix "$app" ci --no-audit --no-fund
}

build() {
    build_wasm
    install_js
    "$slot" npm --prefix "$app" run build
    local wasm
    for wasm in "$app"/dist/assets/*.wasm; do
        printf 'WASM gzip: %s bytes\n' "$(gzip -c < "$wasm" | wc -c)"
    done
}

case ${1:-} in
    build) build ;;
    run)
        [[ -s $app/dist/index.html ]] || build
        port=${WEB_PORT:-4173}
        printf 'Galaxy verifier: http://127.0.0.1:%s/\n' "$port"
        export PYTHONUNBUFFERED=1
        exec python3 -m http.server --bind 127.0.0.1 --directory "$app/dist" "$port"
        ;;
    test)
        build_wasm
        install_js
        "$slot" npm --prefix "$app" test
        "$slot" cargo test --locked -p galaxy-wasm
        export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
        "$slot" cargo test --locked -p galaxy-wasm --target "$wasm_target" --release --test inspect --test proof
        tmp=$(mktemp -d)
        trap 'rm -rf -- "$tmp"' EXIT
        "$slot" cargo run --locked -p galaxy-wasm --example verifier_vector_image -- "$tmp/photo.png"
        "$slot" cargo build --locked -p galaxy-cli --bin galaxy
        "$slot" node "$app/tests/consistency.mjs" "$CARGO_TARGET_DIR/debug/galaxy" "$tmp/photo.png"
        ;;
    *) echo "usage: $0 {build|run|test}" >&2; exit 2 ;;
esac
