#!/usr/bin/env bash
# Replay a public Testnet CAPTURE/PROOF without distributing the private photo.
set -euo pipefail
if (( $# != 3 && $# != 4 )); then
    echo 'usage: tests/integration/replay-recorded.sh NETWORK_TOML RECORDING_JSON CAPTURE_TX [HISTORY_PAGES]' >&2
    exit 2
fi
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
galaxy=${GALAXY_CLI:-${CARGO_TARGET_DIR:-target/Integration}/release/galaxy}
history=()
if (( $# == 4 )); then history=(--history-pages "$4"); fi
status=0
report=$("$galaxy" --network-config "$1" --recording "$2" "${history[@]}" --json inspect "$3") || status=$?
if [[ $status != 60 ]] || ! jq -e '. as $report |
    .state == "UNSUPPORTED" and .mode == "proof-only" and
    (.summary | contains("development unattested credential")) and
    any(.checks[]; .id == "proof.groth16" and .status == "pass") and
    all(["registration.txn_chain", "capture.txn_chain", "history.complete"][];
        . as $id | $report.checks | any(.id == $id and .status == "pass")) and
    any(.checks[]; .id == "capture.photo_binding" and .status == "not_applicable")' <<< "$report" > /dev/null; then
    printf 'development: offline proof replay failed (exit %s)\n' "$status" >&2
    jq -c '{state,summary}' <<< "$report" >&2
    exit 1
fi
jq -r '"development: offline CAPTURE \(.capture.manifest_tx), PROOF \(.proof.proof_tx): \(.state) (Groth16 passed, image not checked)"' <<< "$report"
