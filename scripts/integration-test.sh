#!/usr/bin/env bash
# Run the live desktop development flow. Private captures never leave this directory.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077

base=${GALAXY_E2E_CONFIG:-deployments/networks/xrpl-testnet.toml}
artifacts=${GALAXY_E2E_ARTIFACTS:-circuits/artifacts/v1m}
sources=${GALAXY_E2E_SOURCES:-fixtures/real/calibration/classes/genuine-3d}
wallet=${GALAXY_E2E_WALLET:-galaxy-e2e-account-txn-id}
device=${GALAXY_E2E_DEVICE:-dev-device}
target=${CARGO_TARGET_DIR:-target/Integration}
dev=${GALAXY_DEV:-$target/release/galaxy-dev}
galaxy=${GALAXY_CLI:-$target/release/galaxy}

if [[ ! -x $dev || ! -x $galaxy ]]; then
    CARGO_TARGET_DIR="$target" scripts/with-slot.sh cargo build --release -p galaxy-devtools -p galaxy-cli
fi
for f in circuit_id.hex main_v1m.graph.bin main_v1m.zkey; do
    [[ -s $artifacts/$f ]] || { printf 'development: missing artifact %s\n' "$artifacts/$f" >&2; exit 2; }
done
shopt -s nullglob
captures=("$sources"/*.gxrw)
(( ${#captures[@]} >= 2 )) || { echo 'development: need at least two separate genuine-3d GXRW captures' >&2; exit 2; }
mkdir -p fixtures/real/e2e-development
run=$(mktemp -d fixtures/real/e2e-development/session-XXXXXXXX)
echo "development: private run directory $run"

python3 tests/integration/prepare.py configured-circuit "$base" "$artifacts/circuit_id.hex" > "$run/circuit.json"
cp -- "$base" "$run/network.toml"
circuit_tx=$(jq -er .registration_tx "$run/circuit.json")

cli=("$dev" --config "$base" --wallet "$wallet")
if ! "${cli[@]}" wallet show-address > "$run/wallet.json"; then
    "${cli[@]}" wallet new > "$run/wallet.json"
fi
if ! "${cli[@]}" wallet balance > "$run/balance.json"; then
    "${cli[@]}" wallet fund
    for ((i = 0; i < 10; i++)); do
        if "${cli[@]}" wallet balance > "$run/balance.json"; then break; fi
        sleep 3
    done
    jq -e '.balance_drops_before_reserve > 0' "$run/balance.json" > /dev/null
fi
"${cli[@]}" register-dev-device --device "$device" > "$run/device.json"
device_tx=$(jq -er .registration_tx "$run/device.json")

verify_report() {
    jq -e --arg mode "$1" '
        .state == "UNSUPPORTED" and .mode == $mode and
        (.summary | contains("development unattested credential")) and
        (([.checks[] | select(.status == "pass") | .id]) as $passed |
            ["capture.photo_binding", "capture.rgb_binding", "registration.txn_chain",
             "capture.txn_chain", "history.complete", "proof.groth16"] - $passed == [])
    ' "$2" > /dev/null
}

index=0
for profile in png-v1 jpeg-v1; do
    while true; do
        if (( index >= ${#captures[@]} )); then
            echo "development: exhausted distinct captures without completing $profile" >&2
            exit 1
        fi
        index=$((index + 1))
        attempt="$run/attempt-$index"
        # A completed INCONCLUSIVE run is never re-challenged. Use the next source for either profile.
        status=0
        scripts/with-slot.sh --heavy "${cli[@]}" capture --profile "$profile" \
            --raw "${captures[index - 1]}" --run-dir "$attempt" --device "$device" \
            --device-tx "$device_tx" --circuit-tx "$circuit_tx" \
            --graph "$artifacts/main_v1m.graph.bin" --zkey "$artifacts/main_v1m.zkey" \
            > "$run/attempt-$index.json" || status=$?
        if (( status == 30 )); then
            jq -er '"development: \(.state) CAPTURE \(.capture.tx) (Lsel \(.Lsel))"' "$run/attempt-$index.json"
            continue
        elif (( status == 17 )); then
            jq -e '.mode == "development" and .state == "CAPTURE_ALREADY_USED"' "$run/attempt-$index.json" > /dev/null
            continue
        elif (( status != 0 )); then
            exit "$status"
        fi
        printf '%s\n' "attempt-$index" > "$run/$profile.attempt"
        image="$attempt/$(jq -er .export "$attempt/run.json")"
        status=0
        "$galaxy" --network-config "$run/network.toml" --json verify --no-discovery "$image" \
            > "$run/$profile.verify.json" || status=$?
        [[ $status == 60 ]] && verify_report direct "$run/$profile.verify.json" ||
            { echo "development: direct verification did not pass for $profile (exit $status)" >&2; exit 1; }
        status=0
        "$galaxy" --network-config "$run/network.toml" --json verify --no-discovery "$image" \
            --manifest-tx "$(jq -er .capture.tx "$attempt/run.json")" \
            --proof-tx "$(jq -er .proof.tx "$attempt/run.json")" \
            > "$run/$profile.manual.json" || status=$?
        [[ $status == 60 ]] && verify_report manual "$run/$profile.manual.json" ||
            { echo "development: manual verification did not pass for $profile (exit $status)" >&2; exit 1; }
        "$dev" --config "$run/network.toml" record "$image" --output "$run/$profile.recording.json" \
            > "$run/$profile.record.json"
        jq -er --arg profile "$profile" '"development: \($profile) \(.state) after \(.rpc_exchanges) recorded RPC exchanges"' "$run/$profile.record.json"
        break
    done
done
GALAXY_CLI="$galaxy" scripts/e2e-images.sh "$run"
if [[ -n ${GALAXY_E2E_NODE:-} ]]; then
    GALAXY_CLI="$galaxy" scripts/e2e-images.sh "$run" "$GALAXY_E2E_NODE"
else
    GALAXY_CLI="$galaxy" scripts/e2e-node.sh "$run"
fi
