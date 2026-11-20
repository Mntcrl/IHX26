#!/usr/bin/env bash
# Linker stand-in for aarch64-apple-ios builds on Linux (scripts/prover-ios-lib.sh).
#
# galaxy-prover is built as a static library, which rustc produces without a
# linker; the final link happens in Xcode on the Mac. The only link rustc
# attempts is the cdylib that circom-witnesscalc declares in its own
# crate-type list; that dylib is never used or shipped. Without an Apple SDK
# it cannot be linked, so this script writes a placeholder for exactly that
# output and refuses every other link request.
set -euo pipefail
out=""
prev=""
for a in "$@"; do
    if [ "$prev" = "-o" ]; then out="$a"; fi
    prev="$a"
done
case "$(basename "$out")" in
    libcircom_witnesscalc*.dylib)
        printf 'placeholder: unused circom-witnesscalc cdylib (not linked, see scripts/prover-ios-nolink.sh)\n' > "$out"
        ;;
    *)
        echo "prover-ios-nolink.sh: refusing to fake the link of '$out'" >&2
        exit 1
        ;;
esac
