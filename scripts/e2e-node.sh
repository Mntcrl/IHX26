#!/usr/bin/env bash
# Index the development session's real Testnet ledger range, then test lookup.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
umask 077
if (( $# != 1 )); then
    echo 'usage: scripts/e2e-node.sh PRIVATE_SESSION_DIR' >&2
    exit 2
fi
run=$(realpath -- "$1")
[[ $run == "$(realpath fixtures/real)"/* ]] || { echo 'development: private session must be inside fixtures/real' >&2; exit 2; }
target=${CARGO_TARGET_DIR:-target/Integration2}
node=${GALAXY_NODE:-$target/release/galaxy-node}
port=${GALAXY_E2E_NODE_PORT:-18090}
[[ $port =~ ^[1-9][0-9]{0,4}$ ]] && (( port <= 65535 )) || { echo 'development: invalid loopback node port' >&2; exit 2; }
if [[ ! -x $node ]]; then
    CARGO_TARGET_DIR="$target" scripts/with-slot.sh cargo build --release -p galaxy-node
fi
png_run="$run/$(cat "$run/png-v1.attempt")/run.json"
jpeg_run="$run/$(cat "$run/jpeg-v1.attempt")/run.json"
start=$(jq -sre 'map(.L0) | min' "$png_run" "$jpeg_run")
end=$(jq -sre 'map(.Lproof) | max' "$png_run" "$jpeg_run")
index=$(mktemp -d "$run/node-XXXXXXXX")
python3 tests/integration/prepare.py network "$run/network.toml" "$run/circuit.json" --inception "$start" > "$index/network.toml"
url="http://127.0.0.1:$port"
cat > "$index/node.toml" <<EOF
network_config = "network.toml"
data_dir = "data"
listen = "127.0.0.1:$port"
public_url = "$url"
allow_private_peers = true
live = false
discovery = false
EOF
"$node" run --config "$index/node.toml" > "$index/stdout" 2> "$index/stderr" &
pid=$!
cleanup() { kill "$pid" 2>/dev/null || true; wait "$pid" || true; }
trap cleanup EXIT
for ((i = 0; i < 420; i++)); do
    if ! kill -0 "$pid" 2>/dev/null; then
        echo 'development: lookup node exited before completing the Testnet scan' >&2
        tail -n 12 "$index/stderr" >&2
        exit 1
    fi
    if curl -fsS --connect-timeout 1 --max-time 3 "$url/v1/status" > "$index/status.json" 2>/dev/null \
        && jq -e --argjson start "$start" --argjson end "$end" \
            '.network_id == 1 and .indexed_from == $start and .indexed_through >= $end and .incomplete == false' \
            "$index/status.json" > /dev/null; then
        jq -r '"development: real lookup node indexed \(.indexed_from)–\(.indexed_through)"' "$index/status.json"
        scripts/e2e-images.sh "$run" "$url"
        exit
    fi
    sleep 1
done
echo "development: node did not catch up to a validated tip covering $start–$end" >&2
jq -c '{indexed_ranges,gaps,incomplete,sync_issue}' "$index/status.json" >&2
exit 1
