#!/usr/bin/env bash
# Shared helpers for talking to the Mac build host (spec §19). Source this file:
#   . "$(dirname "$0")/lib-mac.sh"
#   mac_wait_reachable 600
#   mac_ssh 'xcodebuild -version'
#   mac_rsync apps/capture-ios
# Overridable environment (read at call time):
#   MAC_HOST, MAC_KEY, MAC_MIRROR
#   MAC_SSH_TIMEOUT   ssh ConnectTimeout in seconds (default 45)
#   MAC_SSH_ATTEMPTS  attempts per call on connection failure (default 6; backoff 5/10/20/40/60 s)
# The Mac link is a congested Wi-Fi: connections are multiplexed through
# /tmp/galaxy-mac-%C (ControlPersist 15 min) and retried only on connection failures,
# never when the remote command itself failed.
# Never point MAC_MIRROR at the user's workspace /Users/unity/Workspace/IXH26/app/XRPL_Galaxy.

MAC_HOST="${MAC_HOST:-unity@10.164.228.154}"
MAC_KEY="${MAC_KEY:-$HOME/.ssh/id_ihx26}"
MAC_MIRROR="${MAC_MIRROR:-/Users/unity/Workspace/IXH26/galaxy-build}"

GALAXY_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

_MAC_BACKOFF=(5 10 20 40 60)

_mac_quote() { printf '%q' "$1"; }

# Fills the array _mac_ssh_opts from the current environment.
_mac_set_ssh_opts() {
    _mac_ssh_opts=(
        -i "$MAC_KEY"
        -o BatchMode=yes
        -o ConnectTimeout="${MAC_SSH_TIMEOUT:-45}"
        -o ServerAliveInterval=10
        -o ServerAliveCountMax=6
        -o ControlMaster=auto
        -o ControlPath=/tmp/galaxy-mac-%C
        -o ControlPersist=15m
    )
}

# _mac_retry <label> <retryable exit codes (space separated)> <cmd...>
# Runs cmd; on a retryable exit code waits per _MAC_BACKOFF and retries, up to MAC_SSH_ATTEMPTS.
_mac_retry() {
    local label="$1" codes=" $2 " attempts="${MAC_SSH_ATTEMPTS:-6}" attempt=1 rc delay
    shift 2
    while :; do
        "$@"
        rc=$?
        if [ "$rc" -eq 0 ] || [[ "$codes" != *" $rc "* ]] || [ "$attempt" -ge "$attempts" ]; then
            return "$rc"
        fi
        delay="${_MAC_BACKOFF[attempt - 1]:-${_MAC_BACKOFF[-1]}}"
        echo "$label: connection failure (exit $rc) to $MAC_HOST, retry $attempt/$((attempts - 1)) in ${delay}s" >&2
        sleep "$delay"
        attempt=$((attempt + 1))
    done
}

# mac_ssh <command string>: run the command in a login zsh on the Mac (brew/cargo on PATH).
# Retries only on ssh exit code 255 (connection failure).
mac_ssh() {
    if [ "$#" -ne 1 ]; then
        echo "mac_ssh: expected exactly one command string" >&2
        return 2
    fi
    _mac_set_ssh_opts
    _mac_retry mac_ssh "255" ssh "${_mac_ssh_opts[@]}" "$MAC_HOST" "zsh -lc $(_mac_quote "$1")"
}

# mac_wait_reachable [max_seconds]: wait until `mac_ssh true` succeeds (default 600 s).
mac_wait_reachable() {
    local max="${1:-600}" start=$SECONDS
    while :; do
        if MAC_SSH_ATTEMPTS=1 mac_ssh true 2>/dev/null; then
            return 0
        fi
        if [ $((SECONDS - start)) -ge "$max" ]; then
            echo "mac_wait_reachable: $MAC_HOST unreachable after ${max}s" >&2
            return 1
        fi
        echo "mac_wait_reachable: $MAC_HOST unreachable, retrying ($((SECONDS - start))/${max}s)" >&2
        sleep 5
    done
}

# mac_rsync <subtree> [extra rsync args...]: mirror <repo>/<subtree>/ into $MAC_MIRROR/<subtree>/.
# Retries on rsync exit codes 10, 12, 30, 35, 255 (connection/protocol/timeouts).
mac_rsync() {
    if [ "$#" -lt 1 ]; then
        echo "mac_rsync: usage: mac_rsync <subtree> [rsync args...]" >&2
        return 2
    fi
    local subtree="${1%/}"
    shift
    case "$subtree" in
        "" | /* | *..*)
            echo "mac_rsync: subtree must be a relative path inside the repo: '$subtree'" >&2
            return 2
            ;;
    esac
    if [ ! -d "$GALAXY_REPO_ROOT/$subtree" ]; then
        echo "mac_rsync: no such directory: $GALAXY_REPO_ROOT/$subtree" >&2
        return 2
    fi
    case "$MAC_MIRROR" in
        /Users/unity/Workspace/IXH26/app | /Users/unity/Workspace/IXH26/app/*)
            echo "mac_rsync: refusing to write into the user's workspace ($MAC_MIRROR)" >&2
            return 2
            ;;
    esac
    mac_ssh "mkdir -p $(_mac_quote "$MAC_MIRROR/$subtree")" || return
    _mac_set_ssh_opts
    _mac_retry mac_rsync "10 12 30 35 255" \
        rsync -az --delete --partial \
        --exclude 'target/' --exclude 'node_modules/' --exclude 'build/' --exclude 'DerivedData/' \
        -e "ssh ${_mac_ssh_opts[*]}" \
        "$@" \
        "$GALAXY_REPO_ROOT/$subtree/" "$MAC_HOST:$MAC_MIRROR/$subtree/"
}
