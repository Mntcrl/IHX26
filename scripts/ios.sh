#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib-mac.sh"

subtree=apps/capture-ios
appdir="$MAC_MIRROR/$subtree"
bundle=utcq.XRPL-Galaxy
device="$(_mac_quote "${DEVICE:-E34AE83F-D43C-510A-94EA-8E032BCD853E}")"
attest="${APP_ATTEST:-off}"
case "$attest" in
    off) ;;
    development|production) ;;
    *) echo 'APP_ATTEST must be off, development or production' >&2; exit 2 ;;
esac
builddir="$appdir/build/$attest"
app="$builddir/Debug-iphoneos/XRPL_Galaxy.app"

# Protect all device operations, including console launches. Only the invocation
# that successfully acquired the directory may remove it. Each CoreDevice command
# has a deadline; lock holders never wait indefinitely for a diagnostic app to exit.
with_device_lock() {
    mac_ssh "set -e
        acquired=no
        for i in {1..115}; do
            if mkdir /tmp/galaxy-device.lock 2>/dev/null; then acquired=yes; break; fi
            sleep 5
        done
        [ \"\$acquired\" = yes ] || { echo 'device lock busy' >&2; exit 3; }
        trap 'rmdir /tmp/galaxy-device.lock' EXIT
        trap 'exit 130' INT
        trap 'exit 143' HUP TERM
        $1"
}

# Signing needs the unlocked login keychain, which only the Mac user's Aqua session has
# (errSecInternalComponent over SSH): builds are submitted to the Aqua build runner
# (ADR 0071). IOS_SSH_BUILD=1 builds directly in the SSH session instead.
build() {
    mac_rsync "$subtree"
    local runner="$appdir/tools/mac-build-runner.sh"
    local action=submit
    if [ "${IOS_SSH_BUILD:-0}" = 1 ]; then action=build; fi
    mac_ssh "/bin/bash $(_mac_quote "$runner") $action capture-$attest"
}

install="xcrun devicectl --timeout 90 device install app --device $device $(_mac_quote "$app")"
launch="xcrun devicectl --timeout 120 device process launch --device $device --terminate-existing"
cmd="${1:-}"
if [ "$#" -gt 0 ]; then shift; fi
case "$cmd" in
    build) build ;;
    install) build; with_device_lock "$install" ;;
    run|logs)
        app_args=""
        if [ "$#" -gt 0 ]; then printf -v app_args '%q ' "$@"; app_args="-- $app_args"; fi
        if [ "$cmd" = run ]; then
            build
            with_device_lock "$install; $launch $bundle $app_args"
        else
            with_device_lock "$launch --console $bundle $app_args"
        fi
        ;;
    pull-fixtures)
        case "${1:-raw}" in
            raw) source=Documents/galaxy-fixtures; target=capture-ios ;;
            appattest) source=Documents/appattest; target=appattest ;;
            *) echo 'usage: scripts/ios.sh pull-fixtures [raw|appattest]' >&2; exit 2 ;;
        esac
        umask 077
        destination="$GALAXY_REPO_ROOT/fixtures/real/$target"
        mkdir -p "$destination"
        # mktemp isolates concurrent invocations; the transient Mac copy stays
        # private and is removed even on a failed pull.
        stage=$(mac_ssh "umask 077; mktemp -d $(_mac_quote "$appdir/build").pull.XXXXXX")
        trap 'mac_ssh "rm -rf -- $(_mac_quote "$stage")"' EXIT
        with_device_lock "umask 077; xcrun devicectl --timeout 120 device copy from --device $device \
            --domain-type appDataContainer --domain-identifier $bundle --source $source \
            --destination $(_mac_quote "$stage/files")"
        rsync -rt --chmod=D700,F600 -e "ssh ${_mac_ssh_opts[*]}" \
            "$MAC_HOST:$stage/files/" "$destination/"
        ;;
    *) echo 'usage: scripts/ios.sh {build|install|run [app args...]|logs [app args...]|pull-fixtures [raw|appattest]}' >&2; exit 2 ;;
esac
