#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
STATE="$ROOT/target/photon-node"
TEMPLATE="$ROOT/deployments/node/photon-demo.toml"
BIN=${PHOTON_NODE_BIN:-"$ROOT/target/NodeOps1/release/galaxy-node"}
TUNNEL=photon-tunnel-http2.service
INGRESS=photon-node-ingress.service
ACCOUNT=photon-node-account.service
LEDGER=photon-node.service

active() { systemctl --user is-active --quiet "$1"; }
fail() { printf '%s\n' "$*" >&2; exit 1; }

# Read only this invocation: journal entries from a previous quick tunnel are stale.
tunnel_url() {
    local invocation
    active "$TUNNEL" || return 1
    invocation=$(systemctl --user show "$TUNNEL" -p InvocationID --value)
    [[ $invocation =~ ^[[:xdigit:]]{32}$ ]] || return 1
    journalctl --user -q -o cat "_SYSTEMD_INVOCATION_ID=$invocation" |
        grep -Eo 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -n 1
}

write_state() {
    printf '%s\n' "$2" > "$STATE/$1.tmp"
    mv -- "$STATE/$1.tmp" "$STATE/$1"
}

run_node() {
    local profile=$1 unit=$2 port=$3 origin=$4
    active "$unit" && return 0
    awk -v origin="$origin" -v profile="$profile" -v port="$port" '
        /^network_config / { print "network_config = \"../../deployments/networks/xrpl-testnet.toml\""; next }
        /^data_dir / { print "data_dir = \"" profile "\""; next }
        /^listen / { print "listen = \"127.0.0.1:" port "\""; next }
        /^public_url / { print "public_url = \"" origin "\""; next }
        /^sync_mode / {
            print "sync_mode = \"" profile "\""
            if (profile == "account") {
                print "discover_accounts = true"
                print "accounts = [\"rG4e3iS9LGom8QqsZrbq7E6HMCivJwcRre\", \"rPPKkQ5PirkwBJV4nvcp3hSf68HhxKkHvg\", \"rGSjyXZSiWxrFaaSSit9FPdgLS2ZQaAaAp\", \"rnzVNEmtDoY4rHmbdnwzrsAzqqsccAJKfW\"]"
            }
            next
        }
        /^sync_requests_per_second / && profile == "account" { print "sync_requests_per_second = 4"; next }
        { print }
    ' "$TEMPLATE" > "$STATE/$profile.toml.tmp"
    mv -- "$STATE/$profile.toml.tmp" "$STATE/$profile.toml"
    systemd-run --user --unit="$unit" --property=Type=exec --property=Restart=on-failure \
        --property=RestartSec=3s --property=UMask=0077 -- "$(realpath -- "$BIN")" run --config "$STATE/$profile.toml"
}

proxy() {
    # Quick tunnels append X-Forwarded-For; the node requires one address.
    # Only forward the Cloudflare edge's single, valid client IP, never inbound XFF.
    exec node --input-type=commonjs - "$STATE/upstream-port" <<'NODE'
const { readFileSync } = require('node:fs');
const { createServer, request } = require('node:http');
const { isIP } = require('node:net');
const route = process.argv[2];

function unavailable(res) {
  if (res.headersSent) return res.destroy();
  res.writeHead(502, { 'cache-control': 'no-store', 'access-control-allow-origin': '*' });
  res.end();
}

createServer({ requestTimeout: 10_000, headersTimeout: 5_000 }, (req, res) => {
  if (!['GET', 'HEAD', 'OPTIONS'].includes(req.method)) {
    res.writeHead(405, { allow: 'GET, HEAD, OPTIONS' }).end();
    return;
  }
  let port;
  try { port = readFileSync(route, 'utf8').trim(); } catch { return unavailable(res); }
  if (port !== '8099' && port !== '8100') return unavailable(res);
  const ips = req.headersDistinct['cf-connecting-ip'];
  const ip = ips?.length === 1 ? ips[0].trim() : '';
  const headers = !req.headers['cf-worker'] && !ip.includes('%') && isIP(ip)
    ? { 'x-forwarded-for': ip } : {};
  const upstream = request({ host: '127.0.0.1', port, path: req.url, method: req.method, headers }, (reply) => {
    const { connection, 'keep-alive': keepAlive, 'transfer-encoding': encoding, ...responseHeaders } = reply.headers;
    res.writeHead(reply.statusCode, responseHeaders);
    reply.on('error', () => res.destroy());
    reply.pipe(res);
  });
  upstream.setTimeout(10_000, () => upstream.destroy());
  upstream.on('error', () => unavailable(res));
  res.on('close', () => { if (!res.writableEnded) upstream.destroy(); });
  upstream.end();
}).listen(8098, '127.0.0.1');
NODE
}

usage() { fail "Usage: ${0##*/} start [account|ledger] | stop | status | url | promote"; }

case ${1:-} in
    _proxy) proxy ;;
    url) tunnel_url || fail 'No active quick-tunnel origin.' ;;
    status)
        for unit in "$TUNNEL" "$INGRESS" "$ACCOUNT" "$LEDGER"; do
            printf '%s: %s\n' "$unit" "$(systemctl --user is-active "$unit" || true)"
        done
        origin=$(tunnel_url || true)
        recorded=$(cat "$STATE/public-origin" 2>/dev/null || true)
        port=$(cat "$STATE/upstream-port" 2>/dev/null || true)
        printf 'Public URL: %s\n' "${origin:-unavailable}"
        if [[ -n $recorded && $recorded != "$origin" ]]; then
            printf 'URL changed or unavailable; last configured: %s\n' "$recorded"
        fi
        if [[ -n $origin ]] && ! curl -fsS --max-time 5 "$origin/v1/status" >/dev/null; then
            printf 'Public endpoint is not reachable.\n'
        fi
        printf 'Public backend: %s\n' "${port:-not configured}"
        for port in 8099 8100; do
            if result=$(curl -fsS --max-time 5 "http://127.0.0.1:$port/v1/status" 2>/dev/null); then
                printf '%s: ' "$port"
                jq -c '{incomplete, latest_validated_ledger, indexed_ranges, gaps, account_coverage, checkpoints}' <<< "$result"
            fi
        done
        ;;
    start|stop|promote)
        umask 077
        mkdir -p -- "$STATE"
        exec 9> "$STATE/control.lock"
        flock 9
        case $1 in
            start)
                profile=${2:-both}
                [[ $# -le 2 && $profile =~ ^(account|ledger|both)$ ]] || usage
                [[ -x $BIN ]] || fail "Missing release binary: $BIN (see docs/node.md)."
                if ! active "$TUNNEL"; then
                    systemd-run --user --unit="$TUNNEL" --property=Type=exec --property=Restart=no -- \
                        /usr/bin/cloudflared tunnel --no-autoupdate --protocol http2 --url http://127.0.0.1:8098
                fi
                origin=
                for ((attempt=0; attempt<30; attempt++)); do
                    origin=$(tunnel_url) && break
                    sleep 1
                done
                [[ -n $origin ]] || fail "Quick tunnel did not announce a URL; see journalctl --user-unit=$TUNNEL."
                recorded=$(cat "$STATE/public-origin" 2>/dev/null || true)
                if [[ -n $recorded && $recorded != "$origin" ]]; then
                    printf 'URL changed: %s → %s\n' "$recorded" "$origin" >&2
                    for unit in "$ACCOUNT" "$LEDGER"; do
                        if active "$unit"; then systemctl --user stop "$unit"; fi
                    done
                fi
                write_state public-origin "$origin"
                [[ -f $STATE/upstream-port ]] || write_state upstream-port 8099
                if [[ $profile != ledger ]]; then run_node account "$ACCOUNT" 8099 "$origin"; fi
                if [[ $profile != account ]]; then run_node ledger "$LEDGER" 8100 "$origin"; fi
                if ! active "$INGRESS"; then
                    systemd-run --user --unit="$INGRESS" --property=Type=exec --property=Restart=on-failure \
                        --property=RestartSec=3s -- "$ROOT/scripts/photon-node.sh" _proxy
                fi
                printf '%s\n' "$origin"
                ;;
            stop)
                [[ $# -eq 1 ]] || usage
                for unit in "$INGRESS" "$ACCOUNT" "$LEDGER" "$TUNNEL"; do
                    if [[ $(systemctl --user show "$unit" -p LoadState --value) != not-found ]]; then
                        systemctl --user stop "$unit"
                    fi
                done
                ;;
            promote)
                [[ $# -eq 1 ]] || usage
                origin=$(tunnel_url) || fail 'No active quick-tunnel origin.'
                [[ $(cat "$STATE/public-origin") == "$origin" ]] || fail 'Quick-tunnel URL changed; run start to update the node descriptors.'
                curl -fsS --max-time 5 http://127.0.0.1:8100/v1/status |
                    jq -e '.incomplete == false and .network_id == 1' >/dev/null ||
                    fail 'Ledger backend is not caught up; leaving the public route unchanged.'
                write_state upstream-port 8100
                printf '%s now serves the ledger backend.\n' "$origin"
                ;;
        esac
        ;;
    *) usage ;;
esac
