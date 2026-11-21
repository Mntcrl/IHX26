#!/usr/bin/env bash
# Start independent Galaxy nodes, use real Testnet indexing, and check CLI verdicts.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077
if (( $# != 1 )); then
    echo 'usage: scripts/e2e-decentralization.sh PRIVATE_SESSION_DIR' >&2
    exit 2
fi
session=$(realpath -- "$1")
[[ $session == "$(realpath fixtures/real)"/* ]] || { echo 'development: private session must be inside fixtures/real' >&2; exit 2; }
target=${CARGO_TARGET_DIR:-target/Integration}
node=${GALAXY_NODE_FAULTS:-$target/release/galaxy-node}
galaxy=${GALAXY_CLI:-$target/release/galaxy}
if [[ -z ${GALAXY_NODE_FAULTS:-} ]]; then
    CARGO_TARGET_DIR="$target" scripts/with-slot.sh cargo build --release -p galaxy-node --features test-faults
fi
base=${GALAXY_E2E_NODE_PORT:-18100}
[[ $base =~ ^[1-9][0-9]{0,4}$ ]] && (( base <= 65529 )) || { echo 'development: invalid loopback base port' >&2; exit 2; }
work=$(mktemp -d "$session/peers-XXXXXXXX")
png="$session/$(cat "$session/png-v1.attempt")"
jpeg="$session/$(cat "$session/jpeg-v1.attempt")"
# Prefer the newest completed capture to keep fresh Testnet backfills short.
if (( $(jq -er .L0 "$jpeg/run.json") >= $(jq -er .L0 "$png/run.json") )); then
    profile=jpeg-v1; capture="$jpeg"; file="$jpeg/development.jpg"
else
    profile=png-v1; capture="$png"; file="$png/development.png"
fi
start=$(jq -er .L0 "$capture/run.json")
end=$(jq -er .Lproof "$capture/run.json")
# Freeze each independent backfill just after the PROOF. These nodes honestly
# report incomplete coverage relative to the tip; a binding proof still wins.
scan_end=$((end + 1))
photo=$(jq -er .photo_id "$capture/run.json")
manifest=$(jq -er .capture.tx "$capture/run.json")
proof=$(jq -er .proof.tx "$capture/run.json")
recording="$session/$profile.recording.json"
python3 tests/integration/prepare.py images "$png/development.png" "$jpeg/development.jpg" "$jpeg/development-jpeg-raster.png" "$work/variants"
if [[ $profile == jpeg-v1 ]]; then
    # Use the JPEG's unchanged decoded raster, but remove its XMP for lookup.
    python3 - "$jpeg/development-jpeg-raster.png" "$work/variants/metadata-removed.png" <<'PY'
from PIL import Image
import sys
with Image.open(sys.argv[1]) as image:
    image.save(sys.argv[2])
PY
fi
stripped="$work/variants/metadata-removed.png"

pids=() urls=() names=()
stop_nodes() {
    local pid
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    for pid in "${pids[@]}"; do wait "$pid" || true; done
    pids=()
}
trap stop_nodes EXIT
start_node() {
    local name=$1 from=$2 bootstrap=$3 fault=${4-} port=$((base + ${#urls[@]}))
    local url="http://127.0.0.1:$port" dir="$work/$name"
    mkdir -m 700 "$dir"
    python3 tests/integration/prepare.py network "$session/network.toml" "$session/circuit.json" --inception "$from" > "$dir/network.toml"
    cat > "$dir/node.toml" <<EOF
network_config = "network.toml"
data_dir = "data"
listen = "127.0.0.1:$port"
public_url = "$url"
allow_private_peers = true
end_ledger = $scan_end
live = false
discovery = true
bootstrap_peers = $bootstrap
EOF
    if [[ -n $fault ]]; then printf 'test_fault = "%s"\n' "$fault" >> "$dir/node.toml"; fi
    printf '\n[limits]\ndiscovery_interval_secs = 5\n' >> "$dir/node.toml"
    "$node" run --config "$dir/node.toml" > "$dir/stdout" 2> "$dir/stderr" &
    pids+=("$!"); urls+=("$url"); names+=("$name")
}
wait_for() {
    local node_index=$1 path=$2 condition=$3 output_file="$work/${names[$1]}-$2.json"
    local i
    for ((i = 0; i < 420; i++)); do
        if ! kill -0 "${pids[node_index]}" 2>/dev/null; then
            printf 'development: node %s exited\n' "${names[node_index]}" >&2
            tail -n 4 "$work/${names[node_index]}/stderr" >&2
            return 1
        fi
        if curl -fsS --connect-timeout 1 --max-time 3 "${urls[node_index]}/v1/$path" > "$output_file" 2>/dev/null \
            && jq -e "($condition)" "$output_file" > /dev/null; then return; fi
        sleep 1
    done
    printf 'development: node %s did not establish %s\n' "${names[node_index]}" "$path" >&2
    return 1
}
verify() {
    local name=$1 expected=$2 exit_code=$3 condition=$4 image=$5 status=0
    shift 5
    "$galaxy" --network-config "$session/network.toml" --recording "$recording" --json verify "$image" "$@" \
        > "$work/report-$name.json" || status=$?
    if [[ $status != "$exit_code" ]] || ! jq -e --arg state "$expected" ".state == \$state and ($condition)" "$work/report-$name.json" > /dev/null; then
        printf 'development: %s expected %s (exit %s), got exit %s\n' "$name" "$expected" "$exit_code" "$status" >&2
        jq -c '{state,summary}' "$work/report-$name.json" >&2
        return 1
    fi
    printf 'development: %s -> %s\n' "$name" "$expected"
}

bootstrap_for() {
    jq -nc --arg url "http://127.0.0.1:$1" '[$url]'
}
start_node a "$start" "$(bootstrap_for "$((base + 1))")"
start_node b "$start" "$(bootstrap_for "$((base + 2))")"
start_node c "$start" "$(bootstrap_for "$base")"
coverage=".indexed_from == $start and .indexed_through == $scan_end"
pass='.summary | contains("development unattested credential")'
pass="($pass) and (([.checks[] | select(.status == \"pass\") | .id]) as \$passed |
    [\"capture.photo_binding\", \"capture.rgb_binding\", \"registration.txn_chain\",
     \"capture.txn_chain\", \"history.complete\", \"proof.groth16\"] - \$passed == [])"
for i in 0 1 2; do
    wait_for "$i" status "$coverage"
    wait_for "$i" peers '([.peers[].node_id] | unique | length) >= 2'
    result=$(curl -fsS --max-time 4 "${urls[i]}/v1/photos/$photo?network_id=1")
    jq -e --arg manifest "$manifest" --arg proof "$proof" 'any(.captures[]; .manifest_tx == $manifest and .proof_tx == $proof)' <<< "$result" > /dev/null
    verify "node-${names[i]}" UNSUPPORTED 60 "$pass" "$stripped" --lookup --node "${urls[i]}"
done
printf '%s\n' 'development: three independent backfills agreed; each node discovered its two peers from one bootstrap'

start_node wrong "$start" '[]' wrong-references
start_node omit "$start" '[]' omit-captures
start_node fail "$start" '[]' fail-lookup
start_node late "$((start + 1))" '[]'
for i in 3 4 5; do wait_for "$i" status "$coverage"; done
wait_for 6 status ".indexed_from == $((start + 1)) and .indexed_through == $scan_end and .incomplete == true"
verify wrong-with-good UNSUPPORTED 60 "$pass" "$stripped" --lookup --node "${urls[3]}" --node "${urls[0]}"
verify omit-with-good UNSUPPORTED 60 "$pass" "$stripped" --lookup --node "${urls[4]}" --node "${urls[0]}"
verify fail-with-good UNSUPPORTED 60 "$pass" "$stripped" --lookup --node "${urls[5]}" --node "${urls[0]}"
verify only-omitting-index UNAVAILABLE 50 '(.candidates | length) == 0' "$stripped" --lookup --node "${urls[4]}"
verify only-failed-index UNAVAILABLE 50 '(.candidates | length) == 0' "$stripped" --lookup --node "${urls[5]}"
verify only-late-index UNAVAILABLE 50 "any(.trust.ledger_ranges[]; .min == $((start + 1)))" "$stripped" --lookup --node "${urls[6]}"
stop_nodes
verify no-nodes UNSUPPORTED 60 "$pass and (.trust.lookup_endpoints | length) == 0" "$file" --no-discovery
