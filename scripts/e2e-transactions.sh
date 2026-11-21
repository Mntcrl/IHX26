#!/usr/bin/env bash
# Exercise persisted submit recovery, identical duplicate, and proof rejection on Testnet.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077
if (( $# < 1 || $# > 2 )) || [[ ${2-} != '' && ${2-} != --expired-proof ]]; then
    echo 'usage: scripts/e2e-transactions.sh PRIVATE_SESSION_DIR [--expired-proof]' >&2
    exit 2
fi
session=$(realpath -- "$1")
[[ $session == "$(realpath fixtures/real)"/* ]] || { echo 'development: private session must be inside fixtures/real' >&2; exit 2; }
artifacts=${GALAXY_E2E_ARTIFACTS:-circuits/artifacts/v1m}
sources=${GALAXY_E2E_SOURCES:-fixtures/real/calibration/classes/genuine-3d}
wallet=${GALAXY_E2E_FAULT_WALLET:-galaxy-e2e-development-faults}
target=${CARGO_TARGET_DIR:-target/Integration}
dev=${GALAXY_DEV:-$target/release/galaxy-dev}
galaxy=${GALAXY_CLI:-$target/release/galaxy}
shopt -s nullglob
captures=("$sources"/*.gxrw)
(( ${#captures[@]} >= 2 )) || { echo 'development: need separate raw captures for each transaction experiment' >&2; exit 2; }
run=$(mktemp -d "$session/transactions-XXXXXXXX")
cli=("$dev" --config "$session/network.toml" --wallet "$wallet")
if ! "${cli[@]}" wallet show-address > "$run/wallet.json" 2>/dev/null; then
    "${cli[@]}" wallet new > "$run/wallet.json"
fi
if ! "${cli[@]}" wallet balance > "$run/balance.json" 2>/dev/null; then
    "${cli[@]}" wallet fund
    for ((i = 0; i < 10; i++)); do
        if "${cli[@]}" wallet balance > "$run/balance.json" 2>/dev/null; then break; fi
        sleep 3
    done
    jq -e '.balance_drops_before_reserve > 0' "$run/balance.json" > /dev/null
fi
"${cli[@]}" register-dev-device > "$run/device.json"
args=(--device-tx "$(jq -er .registration_tx "$run/device.json")" --circuit-tx "$(jq -er .registration_tx "$session/circuit.json")"
      --graph "$artifacts/main_v1m.graph.bin" --zkey "$artifacts/main_v1m.zkey")
cases=(recovery invalid-proof)
if [[ ${2-} == --expired-proof ]]; then cases+=(expired-proof); fi
index=${#captures[@]}
for case_name in "${cases[@]}"; do
    while true; do
        if (( index == 0 )); then
            echo "development: exhausted distinct captures without completing $case_name" >&2
            exit 1
        fi
        index=$((index - 1))
        work="$run/$case_name-$index"
        extra=(--case "$case_name")
        case $case_name in
            recovery) extra=(--interrupt-after-capture) ;;
            invalid-proof) extra+=(--duplicate-capture) ;;
            expired-proof) extra+=(--defer-expired-assertion) ;;
        esac
        status=0
        scripts/with-slot.sh --heavy "${cli[@]}" capture "${args[@]}" --run-dir "$work" --raw "${captures[index]}" \
            "${extra[@]}" > "$work.initial.json" || status=$?
        if (( status == 17 || status == 30 )); then continue; fi
        if (( status != 0 )); then exit "$status"; fi
        case $case_name in
            recovery)
                scripts/with-slot.sh --heavy "${cli[@]}" capture "${args[@]}" --run-dir "$work" \
                    > "$work.resumed.json" || status=$?
                if (( status == 30 )); then continue; fi
                if (( status != 0 )); then exit "$status"; fi
                jq -ers 'select(.[0].capture_tx == .[1].capture.tx and .[1].transaction_count == 2) |
                    "development: recovered identical CAPTURE \(.[1].capture.tx), validated PROOF \(.[1].proof.tx)"' \
                    "$work.initial.json" "$work.resumed.json"
                ;;
            invalid-proof)
                jq -er 'select(.transaction_count == 2 and .duplicate_capture_engine_result != null) |
                    "development: duplicate CAPTURE \(.capture.tx) submitted: \(.capture_engine_result) / \(.duplicate_capture_engine_result)"' "$work/run.json"
                "$galaxy" --network-config "$session/network.toml" --json proof verify "$(jq -er .proof.tx "$work/run.json")" \
                    > "$work.verify.json" || status=$?
                [[ $status == 20 ]]
                jq -er 'select(.state == "INVALID" and any(.checks[]; .id == "proof.groth16" and .status == "fail")) |
                    "development: canonical invalid PROOF rejected (\(.state))"' "$work.verify.json"
                ;;
            expired-proof)
                expiry_args=(--case expired-proof --run-dir "$work")
                "${cli[@]}" capture "${args[@]}" "${expiry_args[@]}" > "$work.expired.json"
                jq -es '.[0].expired_proof.state == "SIGNED_NOT_SUBMITTED" and
                    .[0].expired_proof.tx == .[1].expired_proof.tx and
                    .[1].expired_proof.attempted_at_or_after_ledger > .[1].proof_deadline' \
                    "$work.initial.json" "$work.expired.json" > /dev/null
                jq -er 'select(.transaction_count == 1 and .expired_proof.engine_result == "tefMAX_LEDGER") |
                    "development: expired PROOF \(.expired_proof.tx) rejected by Testnet (tefMAX_LEDGER)"' "$work/run.json"
                ;;
        esac
        break
    done
done
