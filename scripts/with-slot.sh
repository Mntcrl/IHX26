#!/usr/bin/env bash
# Counting semaphore for memory-heavy jobs shared by all agents on this machine.
#
#   scripts/with-slot.sh [--heavy] <cmd...>
#
# Default: hold one of GALAXY_SLOTS (default 3) slot locks /tmp/galaxy-slot.<i>.lock
# while <cmd> runs, with CARGO_BUILD_JOBS defaulting to 2.
# --heavy: additionally hold the single /tmp/galaxy-heavy.lock (circom compiles of big
# circuits, snarkjs setup, desktop proving of large circuits). The heavy lock is taken
# before the slot, so heavy waiters never sit on a slot.
# Exit status: the command's exit status (2 on usage errors).

set -u

heavy=0
if [ "${1:-}" = "--heavy" ]; then
    heavy=1
    shift
fi
if [ "$#" -eq 0 ]; then
    echo "usage: $0 [--heavy] <cmd...>" >&2
    exit 2
fi
if ! command -v flock >/dev/null 2>&1; then
    echo "with-slot: flock(1) not found" >&2
    exit 2
fi

slots="${GALAXY_SLOTS:-3}"
case "$slots" in
    '' | *[!0-9]* | 0)
        echo "with-slot: GALAXY_SLOTS must be a positive integer (got '$slots')" >&2
        exit 2
        ;;
esac

heavy_fd=
if [ "$heavy" -eq 1 ]; then
    exec {heavy_fd}>/tmp/galaxy-heavy.lock
    if ! flock -n "$heavy_fd"; then
        echo "with-slot: waiting for heavy lock /tmp/galaxy-heavy.lock" >&2
        flock "$heavy_fd"
    fi
fi

slot_fd=
slot=
announced=0
while [ -z "$slot" ]; do
    for ((i = 0; i < slots; i++)); do
        exec {fd}>"/tmp/galaxy-slot.$i.lock"
        if flock -n "$fd"; then
            slot_fd=$fd
            slot=$i
            break
        fi
        exec {fd}>&-
    done
    if [ -z "$slot" ]; then
        if [ "$announced" -eq 0 ]; then
            echo "with-slot: waiting for slot (all $slots busy)" >&2
            announced=1
        fi
        sleep 2
    fi
done

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Rayon-based tools (galaxy-prover, arkworks) default to one thread per CPU; keep
# them bounded so concurrent heavy jobs do not oversubscribe the 8-CPU host.
export RAYON_NUM_THREADS="${RAYON_NUM_THREADS:-4}"

# Guard against runaway thread creation: on 2026-11-20 a leaking job reached
# 16k threads and exhausted the user's pid budget (fork failures for every
# process of the session). RLIMIT_NPROC counts all of this user's threads, so
# the cap must stay well above normal usage (~1-2k) while failing a runaway
# job long before the cgroup limit (~36k). Override with GALAXY_NPROC_LIMIT.
ulimit -u "${GALAXY_NPROC_LIMIT:-12000}" 2>/dev/null || true

# The command must not inherit the lock descriptors (a daemon it spawns would keep the slot).
if [ -n "$heavy_fd" ]; then
    "$@" {slot_fd}>&- {heavy_fd}>&-
else
    "$@" {slot_fd}>&-
fi
exit $?
