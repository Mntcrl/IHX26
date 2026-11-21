#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/lib-mac.sh"

subtree=apps/capture-ios
appdir="$MAC_MIRROR/$subtree"
bundle=utcq.XRPL-Galaxy
device="$(_mac_quote "${DEVICE:-E34AE83F-D43C-510A-94EA-8E032BCD853E}")"
core=bindings/swift/GalaxyCore
artifactdir=circuits/artifacts/v1m
networkdir=deployments/networks
descriptor="$GALAXY_REPO_ROOT/$artifactdir/descriptor.json"
network="$GALAXY_REPO_ROOT/$networkdir/xrpl-testnet.toml"
core_info="$GALAXY_REPO_ROOT/$core/BUILD-INFO.txt"
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

require_build_inputs() {
    local kind expected actual
    if [ ! -s "$core_info" ] || [ ! -s "$GALAXY_REPO_ROOT/$core/ios-arm64/libgalaxy_ffi.a" ]; then
        echo 'Missing GalaxyCore arm64 archive or build info. Run scripts/ios-rust-lib.sh with the approved inputs.' >&2
        return 2
    fi
    for kind in 'descriptor JSON' 'network TOML'; do
        expected=$(awk -v suffix="  build input ($kind)" 'substr($0, 65) == suffix { print $1 }' "$core_info")
        case "$kind" in
            'descriptor JSON') actual=$(sha256sum "$descriptor");;
            'network TOML') actual=$(sha256sum "$network");;
        esac
        if [[ "$expected" != "${actual%% *}" ]]; then
            echo "GalaxyCore build info does not match the current $kind; rebuild GalaxyCore first." >&2
            return 2
        fi
    done
    (
        cd "$GALAXY_REPO_ROOT/$core"
        awk '$2 ~ /^(ios-arm64|swift|include)\// { print }' BUILD-INFO.txt | sha256sum --check --status -
    ) || { echo 'GalaxyCore archive, bindings or headers do not match BUILD-INFO.txt.' >&2; return 2; }
}

sync_build() {
    require_build_inputs
    mac_rsync "$subtree"
    mac_rsync "$core"
    mac_rsync "$artifactdir" --include='/descriptor.json' --exclude='*'
    mac_rsync "$networkdir" --include='/xrpl-testnet.toml' --exclude='*'
}

push_artifacts() (
    require_build_inputs
    # Verify the public artifacts before copying anything; openProver verifies them again on the phone.
    local src id expected stage install_id destination
    src="$GALAXY_REPO_ROOT/$artifactdir"
    id=$({ printf 'GALAXY/CIRCUIT/v1\0'; cat "$descriptor"; } | sha256sum)
    id=${id%% *}
    [[ "$id" =~ ^[a-f0-9]{64}$ ]]
    expected=$(jq -er '.witness_graph_sha256, .zkey_sha256 | select(test("^[a-f0-9]{64}$"))' "$descriptor")
    [[ "$expected" == *$'\n'* ]]
    (
        cd "$src"
        printf '%s  main_v1m.graph.bin\n%s  main_v1m.zkey\n' "${expected%%$'\n'*}" "${expected##*$'\n'}" | sha256sum --check -
    )
    # Use a new immutable slot under the circuit ID. A partial copy gets no READY
    # marker and can never replace a slot mapped by an earlier process.
    stage=$(mac_ssh "set -e; umask 077
        free_kb=\$(/bin/df -k ~ | /usr/bin/awk 'END {print \$4}')
        [ \"\$free_kb\" -ge 3145728 ] || { echo 'less than 3 GiB free on Mac' >&2; exit 4; }
        mktemp -d $(_mac_quote "$appdir/build").artifacts.XXXXXXXX")
    trap 'mac_ssh "rm -rf -- $(_mac_quote "$stage")"' EXIT
    install_id="$(date -u +%Y%m%dT%H%M%SZ)-${stage##*.}"
    destination="Library/Application Support/galaxy-artifacts/$id/$install_id"
    mac_ssh "mkdir -m 700 $(_mac_quote "$stage/pair"); printf '%s\n' $id > $(_mac_quote "$stage/READY")"
    _mac_set_ssh_opts
    _mac_retry mac_artifacts "10 12 30 35 255" rsync -rt --partial -e "ssh ${_mac_ssh_opts[*]}" \
        "$src/main_v1m.graph.bin" "$src/main_v1m.zkey" "$MAC_HOST:$stage/pair/"
    with_device_lock "
        xcrun devicectl --timeout 300 device copy to --device $device --domain-type appDataContainer \
            --domain-identifier $bundle --source $(_mac_quote "$stage/pair") --destination $(_mac_quote "$destination")
        xcrun devicectl --timeout 30 device copy to --device $device --domain-type appDataContainer \
            --domain-identifier $bundle --source $(_mac_quote "$stage/READY") --destination $(_mac_quote "$destination/READY")"
    echo "Proving files installed for $id. Relaunch the capture app to verify and open them."
)

# Signing needs the unlocked login keychain, which only the Mac user's Aqua session has
# (errSecInternalComponent over SSH): builds are submitted to the Aqua build runner
# (ADR 0071). IOS_SSH_BUILD=1 builds directly in the SSH session instead.
build() {
    sync_build
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
    sync-build) sync_build ;;
    push-artifacts) push_artifacts ;;
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
    *) echo 'usage: scripts/ios.sh {sync-build|build|install|push-artifacts|run [app args...]|logs [app args...]|pull-fixtures [raw|appattest]}' >&2; exit 2 ;;
esac
