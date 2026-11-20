#!/usr/bin/env bash
# Submit a whitelisted build in the Mac user's Aqua session; no device operations.
set -euo pipefail
. "$(dirname "$0")/../../../scripts/lib-mac.sh"
mode="${1:-}"
case "$mode" in
    capture-off|capture-development|capture-production|capture-compile|proverbench|proverbench-fallback) ;;
    *) echo 'usage: mac-gui-build.sh {capture-off|capture-development|capture-production|capture-compile|proverbench|proverbench-fallback}' >&2; exit 2 ;;
esac
mac_rsync apps/capture-ios/tools
runner="$MAC_MIRROR/apps/capture-ios/tools/mac-build-runner.sh"
mac_ssh "/bin/bash $(_mac_quote "$runner") submit $mode"
