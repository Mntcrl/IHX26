#!/usr/bin/env python3
"""Runs a command and reports wall time and peak RSS (getrusage of the child),
a stand-in for `/usr/bin/time -v` on hosts without GNU time.

    scripts/prover-time.py <cmd> [args...]

The report goes to stderr as two lines:
    Elapsed (wall clock) time (seconds): <s>
    Maximum resident set size (kbytes): <kb>
"""
import resource
import subprocess
import sys
import time

if len(sys.argv) < 2:
    sys.exit("usage: prover-time.py <cmd> [args...]")
t0 = time.monotonic()
rc = subprocess.call(sys.argv[1:])
wall = time.monotonic() - t0
ru = resource.getrusage(resource.RUSAGE_CHILDREN)
print(f"Elapsed (wall clock) time (seconds): {wall:.3f}", file=sys.stderr)
print(f"Maximum resident set size (kbytes): {ru.ru_maxrss}", file=sys.stderr)
sys.exit(rc)
