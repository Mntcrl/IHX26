#!/bin/bash
# macOS only. The Aqua LaunchAgent executes one fixed Xcode command per allowed request.
set -euo pipefail
umask 077
TOOLS=$(cd "$(dirname "$0")" && pwd -P)
CAPTURE=$(cd "$TOOLS/.." && pwd -P)
MIRROR=$(cd "$CAPTURE/../.." && pwd -P)
case "$MIRROR" in
    "$HOME/Workspace/IXH26/app"|"$HOME/Workspace/IXH26/app/"*)
        echo 'Refusing to build in the user workspace' >&2; exit 2 ;;
esac
STATE="$MIRROR/.galaxy-aqua-build"
LABEL=org.galaxy.buildrunner

build() {
    local mode="$1" product target configuration builddir
    local -a signing=()
    case "$mode" in
        capture-off|capture-development|capture-production|capture-compile)
            cd "$CAPTURE"
            target=XRPL_Galaxy
            configuration=Debug
            builddir="$CAPTURE/build/${mode#capture-}"
            case "$mode" in
                capture-development|capture-production)
                    signing=(CODE_SIGN_ENTITLEMENTS=AppAttest.entitlements "APP_ATTEST_ENVIRONMENT=${mode#capture-}") ;;
                capture-compile) signing=(CODE_SIGNING_ALLOWED=NO) ;;
            esac
            ;;
        proverbench|proverbench-fallback)
            cd "$MIRROR/apps/prover-bench-ios"
            target=ProverBench
            configuration=Release
            builddir="$PWD/build"
            # proverbench-fallback (MobileProver, approved by the orchestrator): the free team cannot
            # provision a new bundle id without an Xcode account, so the bench app is signed with the
            # capture app's id and replaces it on the phone for the benchmark window.
            if [ "$mode" = proverbench-fallback ]; then
                signing=(GALAXY_BENCH_BUNDLE_ID=utcq.XRPL-Galaxy)
            else
                signing=(GALAXY_BENCH_BUNDLE_ID=utcq.XRPL-Galaxy.proverbench)
            fi
            ;;
        *) echo 'Unknown or disallowed Aqua build mode' >&2; return 2 ;;
    esac
    /bin/df -h ~
    local free_kb
    free_kb=$(/bin/df -k ~ | /usr/bin/awk 'END {print $4}')
    [ "$free_kb" -ge 3145728 ] || { echo 'less than 3 GiB free on Mac' >&2; return 4; }
    # -derivedDataPath requires -scheme, which cannot resolve the iOS SDK on this host.
    /usr/bin/xcodebuild -quiet -project "$target.xcodeproj" -target "$target" -sdk iphoneos \
        -configuration "$configuration" -allowProvisioningUpdates \
        "SYMROOT=$builddir" "OBJROOT=$builddir/obj" \
        "CLANG_MODULE_CACHE_PATH=$builddir/ModuleCache.noindex" \
        "SWIFT_MODULE_CACHE_PATH=$builddir/ModuleCache.noindex" \
        "SHARED_PRECOMPS_DIR=$builddir/PrecompiledHeaders" \
        ${signing[@]+"${signing[@]}"} build
    product="$builddir/$configuration-iphoneos/$target.app"
    if [ "$mode" != capture-compile ]; then /usr/bin/codesign --verify "$product"; fi
    /usr/bin/du -sh "$builddir" "$product"
    /bin/df -h ~
}

case "${1:-}" in
    worker)
        for request in "$STATE"/queue/job.*; do
            [ -f "$request" ] || continue
            job="$STATE/jobs/${request##*/}"
            if [ ! -d "$job" ]; then /bin/rm -- "$request"; continue; fi
            mode="$(/bin/cat -- "$request")"
            status=0
            /bin/bash "$TOOLS/mac-build-runner.sh" build "$mode" >"$job/build.log" 2>&1 || status=$?
            printf '%s\n' "$status" >"$job/exit.tmp"
            /bin/mv "$job/exit.tmp" "$job/exit"
            /bin/rm -- "$request"
        done
        ;;
    submit)
        case "${2:-}" in
            capture-off|capture-development|capture-production|capture-compile|proverbench|proverbench-fallback) ;;
            *) echo 'Unknown or disallowed Aqua build mode' >&2; exit 2 ;;
        esac
        if [ "$(/usr/bin/stat -f %Su /dev/console)" != "$(/usr/bin/id -un)" ]; then
            echo 'No Aqua console session for the Mac build user' >&2; exit 5
        fi
        domain="gui/$(/usr/bin/id -u)"
        /bin/mkdir -p "$STATE/queue" "$STATE/jobs" "$HOME/Library/LaunchAgents"
        if ! /bin/launchctl print "$domain/$LABEL" >/dev/null 2>&1; then
            plist="$HOME/Library/LaunchAgents/$LABEL.plist"
            tmp=$(/usr/bin/mktemp "$STATE/agent.XXXXXX")
            /usr/bin/plutil -create xml1 "$tmp"
            /usr/bin/plutil -insert Label -string "$LABEL" "$tmp"
            /usr/bin/plutil -insert LimitLoadToSessionType -string Aqua "$tmp"
            /usr/bin/plutil -insert ProgramArguments -json '["/bin/bash"]' "$tmp"
            /usr/bin/plutil -insert ProgramArguments.1 -string "$TOOLS/mac-build-runner.sh" "$tmp"
            /usr/bin/plutil -insert ProgramArguments.2 -string worker "$tmp"
            /usr/bin/plutil -insert QueueDirectories -json '[]' "$tmp"
            /usr/bin/plutil -insert QueueDirectories.0 -string "$STATE/queue" "$tmp"
            /bin/mv "$tmp" "$plist"
            /bin/launchctl bootstrap "$domain" "$plist" || /bin/launchctl print "$domain/$LABEL" >/dev/null
        fi
        job=$(/usr/bin/mktemp -d "$STATE/jobs/job.XXXXXX")
        printf '%s\n' "$2" >"$job/request"
        /bin/mv "$job/request" "$STATE/queue/${job##*/}"
        echo "Aqua build queued ($2)"
        for ((i=0; i<280; i++)); do
            if [ -f "$job/exit" ]; then
                /bin/cat -- "$job/build.log"
                status=$(/bin/cat -- "$job/exit")
                /bin/rm -rf -- "$job"
                exit "$status"
            fi
            /bin/sleep 2
        done
        echo "Aqua build did not finish before timeout; inspect $job/build.log on the Mac" >&2
        [ ! -f "$job/build.log" ] || /bin/cat -- "$job/build.log"
        exit 6
        ;;
    build) build "${2:-}" ;;
    *) echo 'usage: mac-build-runner.sh {submit MODE|build MODE|worker}' >&2; exit 2 ;;
esac
