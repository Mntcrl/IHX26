#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C NO_COLOR=1
cd "$(dirname "$0")/.."
ROOT=$PWD
OUT=circuits/artifacts/v1m
MANIFEST=circuits/artifacts/v1m.json
BUILD=circuits/build
LIB=circuits/node_modules
SRC=circuits/physical-consistency/main_v1m.circom
CIRCOM="$ROOT/.tools/bin/circom"
GRAPH="$ROOT/.tools/bin/build-circuit"
SNARKJS="$ROOT/$LIB/.bin/snarkjs"
export PATH="$ROOT/.tools/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/CircuitCore}"

fail() { echo "circuit: $*" >&2; exit 1; }
sha() { sha256sum -- "$1" | cut -d' ' -f1; }
require_file() { [[ -s "$1" ]] || fail "missing or empty $1"; }

# Run only after obtaining the heavy slot so the measurement is current. The
# machine is shared: require 2 GiB of free RAM and 8 GiB of RAM plus swap.
check_memory() {
    free -g >&2
    local ram swap
    ram=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
    swap=$(awk '/^SwapFree:/ {print $2}' /proc/meminfo)
    [[ "$ram" =~ ^[0-9]+$ && "$swap" =~ ^[0-9]+$ ]] || fail 'cannot read /proc/meminfo'
    (( ram >= 2 * 1024 * 1024 && ram + swap >= 8 * 1024 * 1024 )) ||
        fail 'need at least 2 GiB available RAM and 8 GiB available RAM + swap'
}

source_digest() {
    {
        find circuits/components/core circuits/components/geometry circuits/physical-consistency \
            "$LIB/circomlib/circuits" -type f -name '*.circom' -print
        printf '%s\n' circuits/package-lock.json policies/galaxy-pc-v1m.json scripts/circuit-gen-policy.mjs
    } | sort -u | xargs -d '\n' sha256sum -- | sha256sum | cut -d' ' -f1
}

compile() {
    check_memory
    node scripts/circuit-gen-policy.mjs --check
    [[ "$($CIRCOM --version)" == 'circom compiler 2.2.3' ]] || fail 'expected circom 2.2.3'
    [[ -x "$GRAPH" ]] || fail 'run scripts/prover-install-build-circuit.sh to install build-circuit'
    mkdir -p "$BUILD" "$OUT"
    local version=2.2.3 before after
    tmp=$(mktemp -d "$BUILD/compile.XXXXXXXX")
    trap 'rm -rf -- "$tmp"' EXIT
    before=$(source_digest)
    "$CIRCOM" "$SRC" -l "$LIB" --prime bn128 --O2 --inspect --r1cs --wasm --sym -o "$tmp" 2>&1 | tee "$tmp/inspect.log"
    # The proving key MUST use the graph compiler's R1CS. Equality with the
    # standard compile also establishes the WASM/sym witness wire order.
    "$GRAPH" "$SRC" "$tmp/main_v1m.graph.bin" -l "$LIB" --O2 --r1cs "$tmp/graph.r1cs" \
        --temp-dir "$BUILD" > "$tmp/graph.log" 2>&1 || { tail -25 "$tmp/graph.log" >&2; fail 'graph compilation failed'; }
    cmp -s "$tmp/main_v1m.r1cs" "$tmp/graph.r1cs" || fail 'graph and WASM compiler R1CS differ'
    after=$(source_digest)
    [[ "$before" == "$after" ]] || fail 'circuit sources changed while compiling'
    rm -f -- "$OUT/build.json" "$OUT/setup.json" "$MANIFEST"
    mv -f -- "$tmp/graph.r1cs" "$OUT/main_v1m.r1cs"
    mv -f -- "$tmp/main_v1m.graph.bin" "$tmp/main_v1m.sym" "$tmp/inspect.log" "$tmp/graph.log" "$OUT/"
    mkdir -p "$OUT/main_v1m_js"
    mv -f -- "$tmp/main_v1m_js/"* "$OUT/main_v1m_js/"
    node scripts/circuit-metadata.mjs r1cs "$OUT/main_v1m.r1cs" | tee "$OUT/r1cs-info.json"
    jq -n --arg src "$before" --arg version "$version" \
      --arg r1cs "$(sha "$OUT/main_v1m.r1cs")" --arg wasm "$(sha "$OUT/main_v1m_js/main_v1m.wasm")" \
      --arg graph "$(sha "$OUT/main_v1m.graph.bin")" \
      '{circom_version:$version,source_sha256:$src,r1cs_sha256:$r1cs,wasm_sha256:$wasm,witness_graph_sha256:$graph}' > "$OUT/build.json"
}

check_build() {
    local key file
    require_file "$OUT/build.json"
    [[ "$(source_digest)" == "$(jq -er .source_sha256 "$OUT/build.json")" ]] || fail 'source digest changed; run build'
    while read -r key file; do
        [[ "$(sha "$OUT/$file")" == "$(jq -er ".$key" "$OUT/build.json")" ]] || fail "stale $file; run build"
    done <<'EOF'
r1cs_sha256 main_v1m.r1cs
wasm_sha256 main_v1m_js/main_v1m.wasm
witness_graph_sha256 main_v1m.graph.bin
EOF
}

setup() {
    check_memory
    check_build
    local count power entry name ptau
    count=$(jq -er .n_constraints "$OUT/r1cs-info.json")
    power=${GALAXY_PTAU_POWER:-20}
    [[ "$power" =~ ^(20|21)$ ]] || fail 'GALAXY_PTAU_POWER must be 20 or 21'
    (( count + 22 < (1 << power) )) || fail "capacity of pinned power $power is too small for $count constraints + 22 public inputs"
    entry=$(jq -ce --argjson n "$power" '.[] | select(.power == $n and .verified == true)' circuits/ptau.lock.json)
    name=$(jq -er .name <<< "$entry")
    ptau="$ROOT/.tools/ptau/$name"
    require_file "$ptau"
    [[ "$(stat -c %s "$ptau")" == "$(jq -er .size <<< "$entry")" ]] || fail 'ptau size mismatch'
    [[ "$(sha "$ptau")" == "$(jq -er .sha256 <<< "$entry")" ]] || fail 'ptau SHA-256 mismatch'
    [[ "$(b2sum -- "$ptau" | cut -d' ' -f1)" == "$(jq -er .blake2b <<< "$entry")" ]] || fail 'ptau published BLAKE2b mismatch'
    tmp=$(mktemp -d "$BUILD/setup.XXXXXXXX")
    trap 'rm -rf -- "$tmp"' EXIT
    export FORCE_COLOR=0 TERM=dumb
    echo 'Development single-party setup only; do not trust its keys in production.' >&2
    "$SNARKJS" groth16 setup "$OUT/main_v1m.r1cs" "$ptau" "$tmp/initial.zkey" 2>&1 | tee "$tmp/setup-transcript.log"
    require_file "$tmp/initial.zkey"
    # Supply entropy over a pipe: no environment/argv or on-disk secret.
    { od -An -v -tx1 -N64 /dev/urandom | tr -d ' \n'; printf '\n'; } |
      "$SNARKJS" zkey contribute "$tmp/initial.zkey" "$tmp/main_v1m.zkey" --name=development-single-party 2>&1 |
      tee -a "$tmp/setup-transcript.log"
    require_file "$tmp/main_v1m.zkey"
    rm -f -- "$tmp/initial.zkey"
    "$SNARKJS" zkey verify "$OUT/main_v1m.r1cs" "$ptau" "$tmp/main_v1m.zkey" 2>&1 | tee -a "$tmp/setup-transcript.log"
    "$SNARKJS" zkey export verificationkey "$tmp/main_v1m.zkey" "$tmp/verification_key.json"
    jq -n --arg ptau "$name" --arg ptau_sha "$(jq -er .sha256 <<< "$entry")" \
      --arg transcript "$(sha "$tmp/setup-transcript.log")" --arg zkey "$(sha "$tmp/main_v1m.zkey")" \
      --arg r1cs "$(jq -er .r1cs_sha256 "$OUT/build.json")" \
      '{ptau_name:$ptau,ptau_sha256:$ptau_sha,setup_transcript_sha256:$transcript,zkey_sha256:$zkey,r1cs_sha256:$r1cs}' > "$tmp/setup.json"
    rm -f -- "$OUT/setup.json" "$MANIFEST"
    mv -f -- "$tmp/main_v1m.zkey" "$tmp/verification_key.json" "$tmp/setup-transcript.log" "$OUT/"
    mv -f -- "$tmp/setup.json" "$OUT/"
}

export_bundle() {
    check_build
    require_file "$OUT/setup.json"
    [[ "$(jq -er .r1cs_sha256 "$OUT/build.json")" == "$(jq -er .r1cs_sha256 "$OUT/setup.json")" ]] || fail 'setup r1cs mismatch'
    [[ "$(sha "$OUT/main_v1m.zkey")" == "$(jq -er .zkey_sha256 "$OUT/setup.json")" ]] || fail 'zkey digest mismatch'
    [[ "$(sha "$OUT/setup-transcript.log")" == "$(jq -er .setup_transcript_sha256 "$OUT/setup.json")" ]] || fail 'setup transcript digest mismatch'
    scripts/with-slot.sh cargo run --locked -q -p galaxy-witness --example circuit_tool -- \
        bundle "$OUT" "$MANIFEST" policies/galaxy-pc-v1m.json
}

# Constraint report: each production template compiled alone as `main` with the
# production flags (--O2). Counts are per instance; CONSTRAINTS.md multiplies
# them by the instance count in main_v1m and reconciles with the full R1CS.
constraints() {
    local dir="$BUILD/constraints" name expr wrapped
    mkdir -p "$dir"
    node scripts/circuit-gen-policy.mjs --check
    while read -r name expr; do
        wrapped="$dir/$name.circom"
        printf 'pragma circom 2.2.3;\ninclude "%s/circuits/components/core/openings.circom";\ninclude "%s/circuits/components/core/sampling.circom";\ninclude "%s/circuits/components/core/scalars.circom";\ninclude "%s/circuits/components/core/commitment.circom";\ninclude "%s/circuits/components/geometry/policy_v1m.circom";\ncomponent main = %s;\n' \
            "$ROOT" "$ROOT" "$ROOT" "$ROOT" "$ROOT" "$expr" > "$wrapped"
        "$CIRCOM" "$wrapped" -l "$LIB" --O2 --r1cs --inspect -o "$dir" > "$dir/$name.log" 2>&1 ||
            { tail -20 "$dir/$name.log" >&2; fail "compile of $name failed"; }
        printf '%-16s %s\n' "$name" "$(node scripts/circuit-metadata.mjs r1cs "$dir/$name.r1cs" --summary)"
        rm -f -- "$dir/$name.r1cs"
    done <<'EOF'
sampling GalaxySampling()
rgb_tile GalaxyCenterTile(1)
depth_tile GalaxyCenterTile(2)
frame_minus GalaxyTemporalBlock(0)
frame_plus GalaxyTemporalBlock(2)
commitment GalaxyEvidenceCommitment()
camera GalaxyCamera()
imu GalaxyImu()
pixel_valid GeoPixelValid()
quality GeoQuality()
relative_pose GeoRelativePose()
motion GeoMotion()
planarity GeoPlanarity()
rgb_depth GeoRgbDepth()
temporal GeoTemporal()
policy GalaxyPolicyV1m()
EOF
    if [[ -e "$OUT/build.json" ]]; then
        printf '%-16s %s\n' full "$(node scripts/circuit-metadata.mjs r1cs "$OUT/main_v1m.r1cs" --summary)"
    fi
}

case "${1:-}" in
    build) scripts/with-slot.sh --heavy "$0" _build ;;
    _build) compile ;;
    constraints) scripts/with-slot.sh --heavy "$0" _constraints ;;
    _constraints) constraints ;;
    test)
        scripts/with-slot.sh node --test circuits/tests/core/core.test.mjs
        npm --prefix circuits run test:geometry
        ;;
    setup-dev)
        [[ -e "$OUT/build.json" ]] || "$0" build
        scripts/with-slot.sh --heavy "$0" _setup
        export_bundle
        ;;
    _setup) setup ;;
    export) export_bundle ;;
    clean) rm -rf -- "$OUT" "$BUILD"; rm -f -- "$MANIFEST" ;;
    *) echo 'usage: scripts/circuit.sh build|test|constraints|setup-dev|export|clean' >&2; exit 2 ;;
esac
