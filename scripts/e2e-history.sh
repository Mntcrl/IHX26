#!/usr/bin/env bash
# Prove that exhausting account_tx pagination does not change a live Testnet verdict.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077
if (( $# != 2 )); then
    echo 'usage: scripts/e2e-history.sh PRIVATE_SESSION_DIR FUNDED_THIRD_PARTY_WALLET' >&2
    exit 2
fi
run=$(realpath -- "$1")
[[ $run == "$(realpath fixtures/real)"/* ]] || { echo 'development: private session must be inside fixtures/real' >&2; exit 2; }
galaxy=${GALAXY_CLI:-${CARGO_TARGET_DIR:-target/Integration}/release/galaxy}
network="$run/network.toml"
work=$(mktemp -d "$run/history-XXXXXXXX")
profile=jpeg-v1
attempt="$run/$(cat "$run/$profile.attempt")"
image="$attempt/development.jpg"
account=$(jq -er .address "$run/wallet.json")
capture=$(jq -er .capture.tx "$attempt/run.json")
proof_ledger=$(jq -er .Lproof "$attempt/run.json")
registration=$(jq -er '.transactions[-1].ledger' "$run/device.json")
echo "development: history results in $work"

node tests/integration/history-payments.mjs "$2" "$account" "$proof_ledger" > "$work/payments.jsonl"
last=$(jq -ser 'last.ledger' "$work/payments.jsonl")
for budget in 1 0; do
    prefix="$work/$profile-history-$budget"
    status=0
    "$galaxy" --network-config "$network" --history-pages "$budget" --record "$prefix.recording.json" \
        --json verify --no-discovery "$image" > "$prefix.report.json" || status=$?
    [[ $status == 60 ]] && jq -e --slurpfile expected "$run/$profile.verify.json" '
        . as $actual | $expected[0] as $before |
        .state == "UNSUPPORTED" and .state == $before.state and .summary == $before.summary and
        .capture.manifest_tx == $before.capture.manifest_tx and .proof.proof_tx == $before.proof.proof_tx and
        all(["capture.photo_binding", "capture.rgb_binding", "registration.txn_chain",
             "capture.txn_chain", "history.complete", "proof.groth16"][];
            . as $id | $actual.checks | any(.id == $id and .status == "pass"))
    ' "$prefix.report.json" > /dev/null || { echo "development: history verification changed at $budget pages (exit $status)" >&2; exit 1; }

    # Match the exact revocation request/response rather than trusting aggregate
    # report counters: the one-page run must encounter a real marker on Testnet.
    if (( budget == 1 )); then
        jq -e --arg account "$account" --argjson first "$registration" --argjson last "$last" '
            any(.[]; .method == "account_tx" and .params.account == $account and
                .params.limit == 200 and .params.ledger_index_min == $first and
                .params.ledger_index_max >= $last and .result.marker != null and
                (.result.transactions | length) == 200)
        ' "$prefix.recording.json" > /dev/null
    fi
    detail=$(jq -er '.checks[] | select(.id == "history.complete") | .detail' "$prefix.report.json")
    [[ $detail =~ [1-9][0-9]*\ AccountTxnID\ steps ]] || { echo 'development: verifier did not walk AccountTxnID' >&2; exit 1; }
    echo "development: $budget page(s), UNSUPPORTED; $detail"
    GALAXY_CLI="$galaxy" tests/integration/replay-recorded.sh "$network" "$prefix.recording.json" "$capture" "$budget"
done
