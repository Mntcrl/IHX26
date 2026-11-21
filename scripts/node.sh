#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."

case "${1:-}" in
    build) shift; exec scripts/with-slot.sh "${CARGO:-cargo}" build -p galaxy-node "$@" ;;
    run) shift; exec scripts/with-slot.sh "${CARGO:-cargo}" run -p galaxy-node -- run --config "${GALAXY_NODE_CONFIG:-deployments/node/testnet.local.toml}" "$@" ;;
    test) shift; exec scripts/with-slot.sh "${CARGO:-cargo}" test -p galaxy-node "$@" ;;
    *) printf 'Usage: %s build|run|test [arguments]\n' "$0" >&2; exit 2 ;;
esac
