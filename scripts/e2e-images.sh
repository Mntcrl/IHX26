#!/usr/bin/env bash
# Test real private development exports against the recorded Testnet RPC session.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077
if (( $# < 1 || $# > 2 )); then
    echo 'usage: scripts/e2e-images.sh PRIVATE_SESSION_DIR [REAL_LOOKUP_NODE_URL]' >&2
    exit 2
fi
run=$(realpath -- "$1")
[[ $run == "$(realpath fixtures/real)"/* ]] || { echo 'development: private session must be inside fixtures/real' >&2; exit 2; }
node=${2-}
galaxy=${GALAXY_CLI:-${CARGO_TARGET_DIR:-target/Integration}/release/galaxy}
network="$run/network.toml"
png_attempt="$run/$(cat "$run/png-v1.attempt")"
jpg_attempt="$run/$(cat "$run/jpeg-v1.attempt")"
variants="$run/image-cases"
mkdir -p "$variants"
python3 tests/integration/prepare.py images "$png_attempt/development.png" "$jpg_attempt/development.jpg" \
    "$jpg_attempt/development-jpeg-raster.png" "$variants"

pass='(.summary | contains("development unattested credential")) and
    (([.checks[] | select(.status == "pass") | .id]) as $passed |
        ["capture.photo_binding", "capture.rgb_binding", "registration.txn_chain",
         "capture.txn_chain", "history.complete", "proof.groth16"] - $passed == []) and
    (all(.checks[]; if .id == "registration.attestation" then .status == "fail"
       elif .id == "evidence.bluetooth" or .id == "evidence.noise" then .status == "not_applicable"
       elif .id == "capture.counter" or .id == "refs.metadata" then (.status == "pass" or .status == "not_applicable")
       else .status == "pass" end))'
binding_error='any(.checks[]; .id == "capture.photo_binding" and .status == "fail")'
profile_error='any(.checks[]; .id == "image.profile" and .status == "fail")'
verify() {
    local name=$1 recording=$2 file=$3 expected=$4 exit_code=$5 checks=$6
    shift 6
    local status=0
    local -a ledger=()
    if [[ -n $recording ]]; then ledger=(--recording "$run/$recording.recording.json"); fi
    "$galaxy" --network-config "$network" "${ledger[@]}" --json verify "$file" "$@" \
        > "$variants/$name.json" || status=$?
    if [[ $status != "$exit_code" ]] || ! jq -e --arg state "$expected" ".state == \$state and ($checks)" "$variants/$name.json" >/dev/null; then
        printf 'development: %s expected %s (exit %s), observed exit %s\n' "$name" "$expected" "$exit_code" "$status" >&2
        jq -c '{state,summary,checks:[.checks[] | select(.status != "pass") | {id,status,detail}]}' "$variants/$name.json" >&2
        exit 1
    fi
    printf 'development: %s -> %s (exit %s)\n' "$name" "$expected" "$status"
}

if [[ -z $node ]]; then
    verify original-png png-v1 "$png_attempt/development.png" UNSUPPORTED 60 "$pass" --no-discovery
    verify original-jpeg jpeg-v1 "$jpg_attempt/development.jpg" UNSUPPORTED 60 "$pass" --no-discovery
    verify lossless-jpeg-raster-png jpeg-v1 "$jpg_attempt/development-jpeg-raster.png" UNSUPPORTED 60 "$pass" --no-discovery
    verify metadata-substituted jpeg-v1 "$variants/metadata-substituted.png" INVALID 20 "$binding_error" --no-discovery
    verify one-pixel png-v1 "$variants/one-pixel.png" INVALID 20 "$binding_error" --no-discovery
    verify crop png-v1 "$variants/crop.png" UNSUPPORTED 60 "$profile_error" --no-discovery
    verify resize png-v1 "$variants/resize.png" UNSUPPORTED 60 "$profile_error" --no-discovery
    verify recompressed jpeg-v1 "$variants/recompressed.jpg" INVALID 20 "$binding_error" --no-discovery
    verify metadata-removed-manual png-v1 "$variants/metadata-removed.png" UNSUPPORTED 60 \
        ".mode == \"manual\" and ($pass)" --no-discovery \
        --manifest-tx "$(jq -er .capture.tx "$png_attempt/run.json")" \
        --proof-tx "$(jq -er .proof.tx "$png_attempt/run.json")"
    verify jpeg-raster-metadata-removed-manual jpeg-v1 "$variants/metadata-removed-jpeg-raster.png" UNSUPPORTED 60 \
        ".mode == \"manual\" and ($pass)" --no-discovery \
        --manifest-tx "$(jq -er .capture.tx "$jpg_attempt/run.json")" \
        --proof-tx "$(jq -er .proof.tx "$jpg_attempt/run.json")"
    network="$variants/unrecognized-circuit.toml"
    python3 tests/integration/prepare.py network "$run/network.toml" "$run/circuit.json" --unrecognized > "$network"
    verify unrecognized-circuit png-v1 "$png_attempt/development.png" UNSUPPORTED 60 \
        'any(.checks[]; .id == "circuit.accepted" and .status == "fail")' --no-discovery
else
    verify metadata-removed '' "$variants/metadata-removed.png" UNSUPPORTED 60 \
        ".mode == \"lookup\" and ($pass)" --lookup --node "$node"
    verify original-jpeg-lookup '' "$jpg_attempt/development.jpg" UNSUPPORTED 60 \
        ".mode == \"lookup\" and ($pass)" --lookup --node "$node"
    verify unregistered '' "$variants/unregistered.png" NOT_FOUND 40 \
        '.mode == "lookup" and (.candidates | length) == 0 and (.trust.ledger_ranges | length) > 0' --lookup --node "$node"
fi
