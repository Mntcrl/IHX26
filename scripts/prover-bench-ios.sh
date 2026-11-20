#!/usr/bin/env bash
# Builds apps/prover-bench-ios on the Mac mirror, installs it on the iPhone,
# pushes benchmark artifacts into the app container, runs the proofs and
# verifies every phone proof with snarkjs on Linux.
#
#   scripts/prover-bench-ios.sh build                 # sync + xcodebuild
#   scripts/prover-bench-ios.sh install               # install the built app
#   scripts/prover-bench-ios.sh push <name>...        # copy artifacts to the app's Documents/<name>/
#   scripts/prover-bench-ios.sh run <name>[,<name>] [extra app args]   # launch with --autorun, console log
#   scripts/prover-bench-ios.sh fetch                 # copy Documents/results back and snarkjs-verify
#   scripts/prover-bench-ios.sh cleanup               # delete artifacts/results from the app container
#
# Artifacts: apps/prover-bench-ios/artifacts/<name>/{<name>.graph.bin,<name>.zkey,input.json,vk.json}.
# Device access is serialized with /tmp/galaxy-device.lock on the Mac (spec §19).
# Environment: BUNDLE_ID (default utcq.XRPL-Galaxy.proverbench), DEVICE.
set -euo pipefail
. "$(dirname "$0")/lib-mac.sh"
ROOT="$GALAXY_REPO_ROOT"
BUNDLE_ID="${BUNDLE_ID:-utcq.XRPL-Galaxy.proverbench}"
DEVICE="${DEVICE:-E34AE83F-D43C-510A-94EA-8E032BCD853E}"
APPDIR="$MAC_MIRROR/apps/prover-bench-ios"
ART_MAC="$MAC_MIRROR/prover-bench-artifacts"
APP="$APPDIR/build/Release-iphoneos/ProverBench.app"
RESULTS="$ROOT/apps/prover-bench-ios/artifacts/results"

with_device_lock() {
    # Takes the Mac device lock (retrying), runs the command string, releases it.
    mac_ssh "for i in \$(seq 1 120); do mkdir /tmp/galaxy-device.lock 2>/dev/null && break; sleep 5; done
             [ -d /tmp/galaxy-device.lock ] || { echo 'device lock busy' >&2; exit 3; }
             trap 'rmdir /tmp/galaxy-device.lock' EXIT
             $1"
}

cmd="${1:-}"
shift || true
case "$cmd" in
build)
    [ -f "$ROOT/bindings/swift/galaxy-prover/ios-arm64/libgalaxy_prover.a" ] || "$ROOT/scripts/prover-ios-lib.sh"
    mac_rsync apps/prover-bench-ios --exclude artifacts/
    mac_rsync bindings/swift/galaxy-prover
    # Signing needs the Mac's Aqua session (SSH sessions cannot unlock the keychain):
    # IOS's whitelisted runner. The free team has no Xcode account to provision
    # utcq.XRPL-Galaxy.proverbench, so BUNDLE_ID=utcq.XRPL-Galaxy selects the
    # `proverbench-fallback` mode (replaces the capture app on the phone).
    case "$BUNDLE_ID" in
        utcq.XRPL-Galaxy.proverbench) mode=proverbench ;;
        utcq.XRPL-Galaxy) mode=proverbench-fallback ;;
        *) echo "unsupported BUNDLE_ID $BUNDLE_ID" >&2; exit 2 ;;
    esac
    "$ROOT/scripts/with-slot.sh" "$ROOT/apps/capture-ios/tools/mac-gui-build.sh" "$mode"
    ;;
install)
    with_device_lock "xcrun devicectl device install app --device $DEVICE $(printf %q "$APP") 2>&1 | tail -5"
    ;;
push)
    for name in "$@"; do
        src="$ROOT/apps/prover-bench-ios/artifacts/$name"
        mac_ssh "mkdir -p $ART_MAC/$name"
        rsync -az --info=progress2 -e "ssh ${_mac_ssh_opts[*]}" \
            "$src/$name.graph.bin" "$src/$name.zkey" "$src/input.json" "$MAC_HOST:$ART_MAC/$name/"
        with_device_lock "xcrun devicectl device copy to --device $DEVICE --domain-type appDataContainer \
            --domain-identifier $BUNDLE_ID --source $ART_MAC/$name --destination $DEV_ROOT/$name 2>&1 | tail -3"
    done
    ;;
run)
    names="${1:?names}"
    shift
    with_device_lock "xcrun devicectl device process launch --device $DEVICE --console --terminate-existing \
        $BUNDLE_ID --autorun $names --exit $* 2>&1" | tee -a "$ROOT/apps/prover-bench-ios/artifacts/console.log" \
        | grep -E 'GXBENCH|rror|rash|ermin'
    ;;
cleanup)
    # Spec §19 device data safety: first copy the results off the device (fetch), then the
    # app logs every file under $DEV_ROOT and deletes exactly that directory. The pushed
    # circuit artifacts are copies of apps/prover-bench-ios/artifacts/<name>/ on Linux.
    "$0" fetch
    with_device_lock "xcrun devicectl device process launch --device $DEVICE --console --terminate-existing \
        $BUNDLE_ID --cleanup 2>&1" | tee -a "$ROOT/apps/prover-bench-ios/artifacts/console.log" \
        | grep -E 'GXBENCH|rror|ermin'
    ;;
fetch)
    mac_ssh "rm -rf $ART_MAC/results && mkdir -p $ART_MAC/results"
    with_device_lock "xcrun devicectl device copy from --device $DEVICE --domain-type appDataContainer \
        --domain-identifier $BUNDLE_ID --source $DEV_ROOT/results --destination $ART_MAC/results 2>&1 | tail -2"
    mkdir -p "$RESULTS"
    rsync -az -e "ssh ${_mac_ssh_opts[*]}" "$MAC_HOST:$ART_MAC/results/" "$RESULTS/"
    for p in "$RESULTS"/*.proof.json; do
        base="${p%.proof.json}"
        name="$(basename "$base" | sed -E 's/-[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{6}Z-[0-9]+$//')"
        printf '%s: ' "$(basename "$base")"
        "$ROOT/circuits/node_modules/.bin/snarkjs" groth16 verify \
            "$ROOT/apps/prover-bench-ios/artifacts/$name/vk.json" "$base.public.json" "$p" 2>&1 | tail -1
    done
    ;;
*)
    sed -n '2,15p' "$0"
    exit 2
    ;;
esac
