pragma circom 2.2.2;

include "../../node_modules/circomlib/circuits/bitify.circom";
include "../../node_modules/circomlib/circuits/comparators.circom";

function GeoBitsFor(maximum) {
    assert(maximum >= 0);
    var n = 1;
    while (maximum >= (1 << n)) {
        n++;
    }
    return n;
}

function GeoMax(a, b) {
    return a > b ? a : b;
}

// The calling expression guarantees -2^n <= in < 2^n unless the gadget
// itself is being used to enforce an input range. The top bit is the sign
// complement; the low bits are in two's-complement order.
template GeoSignedBits(n) {
    assert(n > 0 && n < 249);
    signal input in;
    signal output bits[n+1];
    bits <== Num2Bits(n+1)(in + (1 << n));
}

// Difference, not either operand separately, must fit the signed width.
template GeoLT(n) {
    signal input a;
    signal input b;
    signal output out;
    component diff = GeoSignedBits(n);
    diff.in <== a - b;
    out <== 1 - diff.bits[n];
}

template GeoGE(n) {
    signal input a;
    signal input b;
    signal output out;
    signal less <== GeoLT(n)(a,b);
    out <== 1 - less;
}

template GeoNonzero() {
    signal input in;
    signal output out;
    signal zero <== IsZero()(in);
    out <== 1 - zero;
}

template GeoAbs(n) {
    signal input in;
    signal output out;
    signal output neg;
    component signedBits = GeoSignedBits(n);
    signedBits.in <== in;
    neg <== 1 - signedBits.bits[n];
    out <== in - 2*neg*in;
}

// Exact half-open band [-2^k, 2^k) from the bits of in + 2^n, given
// -2^n <= in < 2^n (bits from GeoSignedBits(n)). With H = bits[k..n) as an
// integer and s = bits[n] (1 iff in >= 0): in >= 0 needs H = 0, in < 0 needs
// H = 2^(n-k) - 1. G = H + 1 - s is 0 or 2^(n-k) exactly in those cases and
// lies in [0, 2^(n-k)] otherwise, so one product and IsZero decide it.
template GeoBand(n, k) {
    assert(k >= 0);
    signal input bits[n+1];
    signal output out;
    if (k >= n) {
        out <== 1;
    } else {
        var high = 0;
        for (var j = k; j < n; j++) {
            high += bits[j] * (1 << (j-k));
        }
        signal edge <== high + 1 - bits[n];
        signal product <== edge * (edge - (1 << (n-k)));
        out <== IsZero()(product);
    }
}

// Exact half-open interval [-2^k, 2^k), given -2^n <= in < 2^n.
template GeoInterval(n, k) {
    signal input in;
    signal output out;
    signal bits[n+1] <== GeoSignedBits(n)(in);
    out <== GeoBand(n, k)(bits);
}

// Booleans used by the policy are derived from bit decompositions, IsZero,
// and these polynomials. Bool is for a caller supplying an unproven bit.
template GeoBool() {
    signal input in;
    in * (in - 1) === 0;
}

template GeoAll(n) {
    assert(n > 0);
    signal input in[n];
    signal output out;
    signal prefix[n];
    prefix[0] <== in[0];
    for (var i = 1; i < n; i++) {
        prefix[i] <== prefix[i-1]*in[i];
    }
    out <== prefix[n-1];
}

template GeoAny(n) {
    signal input in[n];
    signal output out;
    signal none[n];
    for (var i = 0; i < n; i++) {
        none[i] <== 1 - in[i];
    }
    signal allNone <== GeoAll(n)(none);
    out <== 1 - allNone;
}

template GeoCount(n) {
    signal input in[n];
    signal output out;
    var sum = 0;
    for (var i = 0; i < n; i++) {
        sum += in[i];
    }
    out <== sum;
}

// Strict improvement selects the new index. Ties keep the first index.
template GeoFirstMax(n, bits) {
    signal input in[n];
    signal output max;
    signal output index;
    signal best[n];
    signal arg[n];
    signal better[n-1];
    best[0] <== in[0];
    arg[0] <== 0;
    for (var i = 1; i < n; i++) {
        better[i-1] <== GeoLT(bits)(best[i-1], in[i]);
        best[i] <== best[i-1] + better[i-1]*(in[i]-best[i-1]);
        arg[i] <== arg[i-1] + better[i-1]*(i-arg[i-1]);
    }
    max <== best[n-1];
    index <== arg[n-1];
}

// Little-endian selector bits, proven boolean by the calling decomposition.
template GeoMux(n, width) {
    var count = 1 << n;
    signal input select[n];
    signal input in[count][width];
    signal output out[width];
    signal tree[count-1][width];
    for (var i = 0; i < count/2; i++) {
        for (var k = 0; k < width; k++) {
            tree[count/2-1+i][k] <== in[2*i][k] + select[0]*(in[2*i+1][k]-in[2*i][k]);
        }
    }
    for (var level = 1; level < n; level++) {
        var start = (count >> (level+1))-1;
        for (var i = start; i < 2*start+1; i++) {
            for (var k = 0; k < width; k++) {
                tree[i][k] <== tree[2*i+1][k] + select[level]*(tree[2*i+2][k]-tree[2*i+1][k]);
            }
        }
    }
    out <== tree[0];
}

// Gate before dividing: inactive num/den becomes 0/1, not an unsatisfiable
// division by zero or an unbounded quotient from a rejected sample.
template GeoSafeDen() {
    signal input enabled;
    signal input num;
    signal input den;
    signal output safeNum;
    signal output safeDen;
    safeNum <== enabled*num;
    safeDen <== 1 + enabled*(den-1);
}

// Signed Euclidean division. The caller proves the numerator bound and that
// den fits dBits; r and den-1-r prove positivity and a unique remainder.
// qBits+dBits < 249 ensures den*q cannot wrap for any prover-supplied quotient.
template GeoFloor(qBits, dBits) {
    assert(qBits+dBits < 249);
    signal input num;
    signal input den;
    signal output q;
    signal output r;
    q <-- num < 0 ? -1 - ((-1-num) \ den) : num \ den;
    r <== num - q*den;
    component qRange = GeoSignedBits(qBits);
    qRange.in <== q;
    component low = Num2Bits(dBits);
    component high = Num2Bits(dBits);
    low.in <== r;
    high.in <== den-1-r;
}

// For projection the guarded quotient is never negative and fits qBits;
// its little-endian bits are exposed for position decoding.
template GeoUnsignedFloor(qBits, dBits) {
    assert(qBits+dBits < 249);
    signal input num;
    signal input den;
    signal output q;
    signal output r;
    signal output bits[qBits];
    q <-- num \ den;
    r <== num - q*den;
    bits <== Num2Bits(qBits)(q);
    component low = Num2Bits(dBits);
    component high = Num2Bits(dBits);
    low.in <== r;
    high.in <== den-1-r;
}

// Floor division by a power of two: low bits are r, higher bits encode q.
template GeoShiftFloor(n, shift) {
    assert(shift >= 0 && shift <= n);
    signal input in;
    signal output q;
    component bounded = GeoSignedBits(n);
    bounded.in <== in;
    var high = 0;
    for (var i = shift; i <= n; i++) {
        high += bounded.bits[i] * (1 << (i-shift));
    }
    q <== high - (1 << (n-shift));
}

// For inclusive absolute thresholds. Select a width for the mathematical
// expression (including the static threshold), not the field element.
template GeoAbsLe(n) {
    signal input in;
    signal input lim;
    signal output out;
    signal tooSmall <== GeoLT(n)(in, -lim);
    signal tooLarge <== GeoLT(n)(lim, in);
    // The inequalities cannot both be true for nonnegative lim.
    out <== 1 - tooLarge - tooSmall;
}

template GeoAbsGe(n) {
    signal input in;
    signal input lim;
    signal output out;
    signal positive <== GeoGE(n)(in, lim);
    signal negative <== GeoGE(n)(-lim, in);
    out <== positive + negative - positive*negative;
}
