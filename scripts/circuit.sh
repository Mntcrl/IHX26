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
PROVER="$CARGO_TARGET_DIR/release/galaxy-prover"
CEREMONY=development-single-party

fail() { echo "circuit: $*" >&2; exit 1; }
sha() { sha256sum -- "$1" | cut -d' ' -f1; }
require_file() { [[ -s "$1" ]] || fail "missing or empty $1"; }
# Build the release example before acquiring a heavy slot. During e2e,
# invoke it directly rather than trying to acquire a second slot.
WITNESS_TOOL="$CARGO_TARGET_DIR/release/examples/circuit_tool"
build_witness_tool() {
    scripts/with-slot.sh cargo build --locked --release -q -p galaxy-witness --example circuit_tool
    require_file "$WITNESS_TOOL"
}

# Measure memory after obtaining the heavy slot. The full graph build needs
# more RAM than development setup; also retain headroom in RAM plus swap.
check_memory() {
    local minimum_mib=$1 ram swap
    free -m >&2
    ram=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
    swap=$(awk '/^SwapFree:/ {print $2}' /proc/meminfo)
    [[ "$ram" =~ ^[0-9]+$ && "$swap" =~ ^[0-9]+$ ]] || fail 'cannot read /proc/meminfo'
    printf 'Build memory: %s KiB RAM available, %s KiB swap free\n' "$ram" "$swap" >&2
    (( ram >= minimum_mib * 1024 && ram + swap >= 8 * 1024 * 1024 )) ||
        fail "need at least $minimum_mib MiB available RAM and 8 GiB available RAM + swap"
}

source_digest() {
    {
        find circuits/components/core circuits/components/geometry circuits/physical-consistency \
            "$LIB/circomlib/circuits" -type f -name '*.circom' -print
        printf '%s\n' circuits/package-lock.json policies/galaxy-pc-v1m.json scripts/circuit-gen-policy.mjs
    } | sort -u | xargs -d '\n' sha256sum -- | sha256sum | cut -d' ' -f1
}

compile() {
    check_memory 4608
    [[ "$($CIRCOM --version)" == 'circom compiler 2.2.3' ]] || fail 'expected circom 2.2.3'
    [[ -x "$GRAPH" ]] || fail 'run scripts/prover-install-build-circuit.sh to install build-circuit'
    mkdir -p "$BUILD" "$OUT"
    local version=2.2.3 before after
    tmp=$(mktemp -d "$BUILD/compile.XXXXXXXX")
    trap 'rm -rf -- "$tmp"' EXIT
    before=$(source_digest)
    node scripts/circuit-gen-policy.mjs --check
    "$CIRCOM" "$SRC" -l "$LIB" --prime bn128 --O2 --inspect --r1cs --wasm --sym -o "$tmp" 2>&1 | tee "$tmp/inspect.log"
    # The proving key MUST use the graph compiler's R1CS. Equality with the
    # standard compile also establishes the WASM/sym witness wire order.
    "$GRAPH" "$SRC" "$tmp/main_v1m.graph.bin" -l "$LIB" --O2 --r1cs "$tmp/graph.r1cs" \
        --temp-dir "$BUILD" > "$tmp/graph.log" 2>&1 || { tail -25 "$tmp/graph.log" >&2; fail 'graph compilation failed'; }
    cmp -s "$tmp/main_v1m.r1cs" "$tmp/graph.r1cs" || fail 'graph and WASM compiler R1CS differ'
    node scripts/circuit-metadata.mjs r1cs "$tmp/graph.r1cs" | tee "$tmp/r1cs-info.json"
    jq -n --arg src "$before" --arg version "$version" \
      --arg r1cs "$(sha "$tmp/graph.r1cs")" --arg wasm "$(sha "$tmp/main_v1m_js/main_v1m.wasm")" \
      --arg graph "$(sha "$tmp/main_v1m.graph.bin")" \
      '{circom_version:$version,source_sha256:$src,r1cs_sha256:$r1cs,wasm_sha256:$wasm,witness_graph_sha256:$graph}' > "$tmp/build.json"
    after=$(source_digest)
    [[ "$before" == "$after" ]] || fail 'circuit sources changed while compiling'
    rm -f -- "$OUT/build.json" "$OUT/setup.json" "$MANIFEST"
    mv -f -- "$tmp/graph.r1cs" "$OUT/main_v1m.r1cs"
    mv -f -- "$tmp/main_v1m.graph.bin" "$tmp/main_v1m.sym" "$tmp/inspect.log" "$tmp/graph.log" "$OUT/"
    mkdir -p "$OUT/main_v1m_js"
    mv -f -- "$tmp/main_v1m_js/"* "$OUT/main_v1m_js/"
    mv -f -- "$tmp/r1cs-info.json" "$tmp/build.json" "$OUT/"
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

# Development setup with galaxy-prover's low-memory tool
# (crates/galaxy-prover/src/dev_setup.rs): `snarkjs groth16 setup` needs about
# 10 GB for this circuit, more than the shared build machine has. Like
# `snarkjs zkey new` the tool consumes the verified phase-1 Lagrange powers of
# the pinned ptau; phase 2 is a single party (delta from the OS RNG inside the
# process, then dropped). Not production-trusted
# (docs/adr/0043-groth16-setup-ceremony.md).
prover_digest() {
    { find crates/galaxy-prover/src -type f -print; printf '%s\n' crates/galaxy-prover/Cargo.toml Cargo.lock; } |
        sort -u | xargs -d '\n' sha256sum -- | sha256sum | cut -d' ' -f1
}

build_prover() {
    scripts/with-slot.sh cargo build --locked --release -q -p galaxy-prover --bin galaxy-prover
    require_file "$PROVER"
}

# Pinned, checksum-verified phase-1 file with capacity for the circuit.
verified_ptau() {
    local count power entry name ptau
    count=$(jq -er .n_constraints "$OUT/r1cs-info.json")
    power=${GALAXY_PTAU_POWER:-20}
    [[ "$power" =~ ^(20|21)$ ]] || fail 'GALAXY_PTAU_POWER must be 20 or 21'
    (( count + 22 + 1 <= (1 << power) )) || fail "capacity of pinned power $power is too small for $count constraints + 22 public inputs"
    entry=$(jq -ce --argjson n "$power" '.[] | select(.power == $n and .verified == true)' circuits/ptau.lock.json)
    name=$(jq -er .name <<< "$entry")
    ptau="$ROOT/.tools/ptau/$name"
    require_file "$ptau"
    [[ "$(stat -c %s "$ptau")" == "$(jq -er .size <<< "$entry")" ]] || fail 'ptau size mismatch'
    [[ "$(sha "$ptau")" == "$(jq -er .sha256 <<< "$entry")" ]] || fail 'ptau SHA-256 mismatch'
    [[ "$(b2sum -- "$ptau" | cut -d' ' -f1)" == "$(jq -er .blake2b <<< "$entry")" ]] || fail 'ptau published BLAKE2b mismatch'
    printf '%s %s\n' "$name" "$(jq -er .sha256 <<< "$entry")"
}

setup() {
    check_memory 2048
    check_build
    require_file "$PROVER"
    local r1cs_sha zkey_sha log ptau_name ptau_sha
    read -r ptau_name ptau_sha < <(verified_ptau)
    [[ -n "$ptau_sha" ]] || fail 'ptau verification failed'
    r1cs_sha=$(jq -er .r1cs_sha256 "$OUT/build.json")
    tmp=$(mktemp -d "$BUILD/setup.XXXXXXXX")
    trap 'rm -rf -- "$tmp"' EXIT
    log="$tmp/setup-transcript.log"
    echo 'Development single-party setup only; do not trust its keys in production.' >&2
    {
        echo 'GALAXY Groth16 setup transcript'
        echo "ceremony: $CEREMONY"
        echo 'tool: galaxy-prover dev-setup (crates/galaxy-prover/src/dev_setup.rs)'
        echo "tool_source_sha256: $(prover_digest)"
        echo "tool_binary_sha256: $(sha "$PROVER")"
        echo "phase1: $ptau_name sha256 $ptau_sha (Lagrange powers consumed as by snarkjs zkey new)"
        echo 'phase2: single party; delta sampled from the OS RNG inside the process and dropped; no further contributions'
        echo "r1cs: main_v1m.r1cs sha256 $r1cs_sha"
    } > "$log"
    scripts/prover-time.py "$PROVER" dev-setup --r1cs "$OUT/main_v1m.r1cs" \
        --ptau "$ROOT/.tools/ptau/$ptau_name" --ptau-sha256 "$ptau_sha" --out "$tmp/main_v1m.zkey" 2>&1 | tee -a "$log"
    require_file "$tmp/main_v1m.zkey"
    zkey_sha=$(sha "$tmp/main_v1m.zkey")
    echo "zkey: main_v1m.zkey sha256 $zkey_sha" >> "$log"
    FORCE_COLOR=0 TERM=dumb "$SNARKJS" zkey export verificationkey "$tmp/main_v1m.zkey" "$tmp/verification_key.json" >&2
    echo "verification_key.json sha256 $(sha "$tmp/verification_key.json")" >> "$log"
    jq -n --arg ceremony "$CEREMONY" --arg ptau "$ptau_name" --arg ptau_sha "$ptau_sha" --arg transcript "$(sha "$log")" \
      --arg zkey "$zkey_sha" --arg r1cs "$r1cs_sha" \
      '{ceremony:$ceremony,ptau_name:$ptau,ptau_sha256:$ptau_sha,setup_transcript_sha256:$transcript,zkey_sha256:$zkey,r1cs_sha256:$r1cs}' \
      > "$tmp/setup.json"
    rm -f -- "$OUT/setup.json" "$MANIFEST"
    mv -f -- "$tmp/main_v1m.zkey" "$tmp/verification_key.json" "$log" "$OUT/"
    mv -f -- "$tmp/setup.json" "$OUT/"
}

check_setup() {
    check_build
    require_file "$OUT/setup.json"
    [[ "$(jq -er .r1cs_sha256 "$OUT/build.json")" == "$(jq -er .r1cs_sha256 "$OUT/setup.json")" ]] || fail 'setup r1cs mismatch'
    [[ "$(sha "$OUT/main_v1m.zkey")" == "$(jq -er .zkey_sha256 "$OUT/setup.json")" ]] || fail 'zkey digest mismatch'
    [[ "$(sha "$OUT/setup-transcript.log")" == "$(jq -er .setup_transcript_sha256 "$OUT/setup.json")" ]] || fail 'setup transcript digest mismatch'
}

export_bundle() {
    build_witness_tool
    check_setup
    require_file "$PROVER"
    # Independent VK encoding: galaxy-prover reads the zkey directly, galaxy-crypto
    # the snarkjs verification_key.json export. The bytes must be identical.
    tmp=$(mktemp -d "$BUILD/export.XXXXXXXX")
    trap 'rm -rf -- "$tmp"' EXIT
    ln -- "$OUT/build.json" "$OUT/setup.json" "$OUT/r1cs-info.json" "$OUT/verification_key.json" "$tmp/"
    "$WITNESS_TOOL" bundle "$tmp" "$tmp/v1m.json" policies/galaxy-pc-v1m.json
    "$PROVER" vk --zkey "$OUT/main_v1m.zkey" --out "$tmp/vk.bin"
    cmp -- "$tmp/vk.bin" "$tmp/verification_key.bin" || fail 'galaxy-prover and galaxy-crypto VK bytes differ'
    mv -f -- "$tmp/verification_key.bin" "$tmp/descriptor.json" "$tmp/circuit_id.hex" "$tmp/circuit_bundle.bin" "$OUT/"
    mv -f -- "$tmp/v1m.json" "$MANIFEST"
    echo 'VK bytes: galaxy-prover (zkey) == galaxy-crypto (verification_key.json)'
}

# Synthetic (development-only) passing room capture -> galaxy-witness input ->
# WASM witness (all circuit assertions) -> graph witness (the prover's path),
# equal wire by wire, every R1CS constraint checked, public wires equal to the
# public inputs reconstructed from the photo and the ledger statement.
# input.json, salts.json and the witness hold private (synthetic) evidence data: never kept.
witness_check() {
    local dir=$1
    rm -rf -- "$dir"
    mkdir -p "$dir"
    # shellcheck disable=SC2064 # expand now: $dir is local
    trap "rm -f -- '$dir/input.json' '$dir/salts.json' '$dir/witness.wtns'" EXIT
    "$WITNESS_TOOL" e2e-input "$dir"
    FORCE_COLOR=0 TERM=dumb "$SNARKJS" wtns calculate "$OUT/main_v1m_js/main_v1m.wasm" "$dir/input.json" "$dir/witness.wtns"
    echo 'WASM witness: all circuit assertions hold'
    "$WITNESS_TOOL" check-witness "$dir" "$OUT/main_v1m.r1cs" "$OUT/main_v1m.graph.bin"
}

# End to end: witness_check, then a galaxy-prover proof from the graph witness
# -> snarkjs verify AND galaxy-crypto verify with the reconstructed public inputs.
e2e() {
    check_setup
    require_file "$OUT/verification_key.bin"
    require_file "$PROVER"
    local dir="$BUILD/e2e"
    witness_check "$dir"
    "$PROVER" prove --graph "$OUT/main_v1m.graph.bin" --zkey "$OUT/main_v1m.zkey" --input "$dir/input.json" --out "$dir"
    cmp -- "$dir/vk.bin" "$OUT/verification_key.bin" || fail 'prover VK differs from the exported VK bytes'
    FORCE_COLOR=0 TERM=dumb "$SNARKJS" groth16 verify "$OUT/verification_key.json" "$dir/public.json" "$dir/proof.json"
    "$WITNESS_TOOL" verify "$dir" "$OUT/verification_key.bin"
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
        build_prover
        scripts/with-slot.sh --heavy "$0" _setup
        export_bundle
        ;;
    _setup) setup ;;
    export) build_prover; export_bundle ;;
    e2e) build_prover; build_witness_tool; scripts/with-slot.sh --heavy "$0" _e2e ;;
    _e2e) e2e ;;
    witness) check_build; build_witness_tool; witness_check "$BUILD/witness" ;;
    clean) rm -rf -- "$OUT" "$BUILD"; rm -f -- "$MANIFEST" ;;
    *) echo 'usage: scripts/circuit.sh build|test|constraints|witness|setup-dev|export|e2e|clean' >&2; exit 2 ;;
esac
