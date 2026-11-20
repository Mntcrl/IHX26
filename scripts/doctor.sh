#!/usr/bin/env bash
# Environment report for the XRPL Galaxy monorepo (`make doctor`).
# Prints one line per check. Exits 1 only when a hard requirement is missing
# (local Rust toolchain/targets, circom, snarkjs, wasm-bindgen, node); network,
# Mac, device and ptau checks are reported but never fail the run.
# Environment: DEVICE, MAC_HOST, MAC_KEY, MAC_MIRROR, NETWORK (see root Makefile).

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib-mac.sh
. "$ROOT/scripts/lib-mac.sh"

DEVICE="${DEVICE:-E34AE83F-D43C-510A-94EA-8E032BCD853E}"
NETWORK="${NETWORK:-xrpl-testnet}"
TOOLS="$ROOT/.tools"
MAC_SSH_TIMEOUT=5
MAC_SSH_ATTEMPTS=1 # doctor reports reachability; no retry backoff

hard_fail=0

row() { printf '%-6s %-22s %s\n' "$1" "$2" "$3"; }
ok() { row "ok" "$1" "$2"; }
warn() { row "warn" "$1" "$2"; }
missing_hard() { row "FAIL" "$1" "$2"; hard_fail=1; }

first_line() { "$@" 2>&1 | head -n 1; }

check_cmd_hard() { # name, binary path or command, version args...
    local name="$1" bin="$2"
    shift 2
    if command -v "$bin" >/dev/null 2>&1; then
        ok "$name" "$(first_line "$bin" "$@") ($(command -v "$bin"))"
    else
        missing_hard "$name" "not found ($bin)"
    fi
}

check_cmd_soft() {
    local name="$1" bin="$2"
    shift 2
    if command -v "$bin" >/dev/null 2>&1; then
        ok "$name" "$(first_line "$bin" "$@")"
    else
        warn "$name" "not found ($bin)"
    fi
}

echo "XRPL Galaxy doctor — repo $ROOT"
echo

echo "== Linux toolchain"
cd "$ROOT" || exit 1
check_cmd_hard rustc rustc --version
check_cmd_hard cargo cargo --version
if command -v rustup >/dev/null 2>&1; then
    installed_targets="$(rustup target list --installed 2>/dev/null)"
    for t in wasm32-unknown-unknown aarch64-apple-ios; do
        if grep -qx "$t" <<<"$installed_targets"; then
            ok "target $t" "installed for $(rustup show active-toolchain 2>/dev/null | cut -d' ' -f1)"
        else
            missing_hard "target $t" "missing (rustup target add $t)"
        fi
    done
else
    missing_hard rustup "not found"
fi
check_cmd_hard circom "$TOOLS/bin/circom" --version
if [ -x "$ROOT/circuits/node_modules/.bin/snarkjs" ]; then
    ok snarkjs "$(cd "$ROOT/circuits" && npx --no-install snarkjs --version 2>/dev/null | grep -m1 snarkjs) (circuits/node_modules)"
else
    missing_hard snarkjs "not installed (cd circuits && npm ci)"
fi
check_cmd_hard wasm-bindgen "$TOOLS/bin/wasm-bindgen" --version
check_cmd_soft wasm-opt "$TOOLS/bin/wasm-opt" --version
check_cmd_hard node node --version
check_cmd_soft npm npm --version
check_cmd_soft docker docker --version
check_cmd_soft sqlite3 sqlite3 --version

echo
echo "== Powers of Tau (.tools/ptau, pinned in circuits/ptau.lock.json)"
lock="$ROOT/circuits/ptau.lock.json"
if [ ! -f "$lock" ]; then
    warn "ptau.lock.json" "missing"
elif ! command -v jq >/dev/null 2>&1; then
    warn "ptau.lock.json" "jq is required to read the lock"
else
    # Size comparison only; hashes are verified when the lock is written (sha256sum/b2sum).
    while IFS=$'\t' read -r name size verified; do
        f="$TOOLS/ptau/$name"
        if [ ! -f "$f" ]; then
            warn "ptau" "$name missing (download url in ptau.lock.json)"
        elif [ "$(wc -c <"$f" | tr -d ' ')" != "$size" ]; then
            warn "ptau" "$name size $(wc -c <"$f" | tr -d ' ') != locked $size (incomplete download?)"
        else
            ok "ptau" "$name $size bytes, published checksum verified=$verified"
        fi
    done < <(jq -r '.[] | [.name, .size, .verified] | @tsv' "$lock")
fi

echo
echo "== Mac build host ($MAC_HOST)"
if mac_ssh 'true' >/dev/null 2>&1; then
    ok "ssh" "reachable (BatchMode, key $MAC_KEY)"
    ok "mac disk" "$(mac_ssh 'df -h ~ | tail -n 1' 2>/dev/null | awk '{print $4 " free of " $2 " (" $5 " used)"}')"
    if mac_ssh "test -d $(printf '%q' "$MAC_MIRROR")" >/dev/null 2>&1; then
        ok "mac mirror" "$MAC_MIRROR exists"
    else
        warn "mac mirror" "$MAC_MIRROR missing"
    fi
    devices="$(mac_ssh 'xcrun devicectl list devices' 2>/dev/null)"
    dev_line="$(grep -F "$DEVICE" <<<"$devices" | tr -s ' ')"
    if [ -n "$dev_line" ]; then
        ok "device" "$dev_line"
    else
        warn "device" "$DEVICE not listed by xcrun devicectl"
    fi
    if mac_ssh 'test -d /tmp/galaxy-device.lock' >/dev/null 2>&1; then
        warn "device lock" "/tmp/galaxy-device.lock is held"
    else
        ok "device lock" "free"
    fi
else
    warn "ssh" "unreachable (timeout 5 s); Mac checks skipped"
fi

echo
echo "== XRPL ($NETWORK)"
case "$NETWORK" in
    xrpl-testnet) rpc_url="https://s.altnet.rippletest.net:51234" ;;
    xrpl-devnet) rpc_url="https://s.devnet.rippletest.net:51234" ;;
    *) rpc_url="" ;;
esac
if [ -z "$rpc_url" ]; then
    warn "server_info" "no JSON-RPC URL known for NETWORK=$NETWORK"
elif ! command -v curl >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
    warn "server_info" "curl and jq are required for this check"
else
    info="$(curl -sS --max-time 10 -H 'Content-Type: application/json' \
        -d '{"method":"server_info","params":[{}]}' "$rpc_url" 2>&1)"
    summary="$(jq -r '.result.info | "build \(.build_version), network_id \(.network_id), state \(.server_state), validated \(.validated_ledger.seq)"' <<<"$info" 2>/dev/null)"
    if [ -n "$summary" ] && [ "$summary" != "null" ]; then
        ok "server_info" "$rpc_url: $summary"
    else
        warn "server_info" "$rpc_url: $(head -c 200 <<<"$info")"
    fi
fi

echo
if [ "$hard_fail" -ne 0 ]; then
    echo "doctor: hard requirements missing (FAIL rows above)"
    exit 1
fi
echo "doctor: all hard requirements present"
