pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";
include "tags.circom";

// Arity-four path segment. Bits are boolean by caller construction, in
// ascending Morton digit order; highIndex contains the unwalked high digits.
// Each sibling row omits the selected child, preserving child order.
template GalaxyMerklePath(kind, frame, start, levels) {
    signal input leaf;
    signal input bits[2*levels];
    signal input high_index;
    signal input siblings[levels][3];
    signal output root;
    signal h[levels+1];
    signal child[levels][4];
    signal a[levels];
    signal b[levels];
    signal xy[levels];
    component hash[levels];
    h[0] <== leaf;
    for (var l = 0; l < levels; l++) {
        var x = bits[2*l];
        var y = bits[2*l+1];
        var s0 = siblings[l][0];
        var s1 = siblings[l][1];
        var s2 = siblings[l][2];
        // Insert h at index x + 2*y into the strictly ordered sibling vector.
        xy[l] <== x*y;
        child[l][0] <== s0 + (1-x-y+xy[l])*(h[l]-s0);
        a[l] <== s0 + x*(h[l]-s0);
        child[l][1] <== a[l] + y*(s1-a[l]);
        b[l] <== h[l] + x*(s2-h[l]);
        child[l][2] <== s1 + y*(b[l]-s1);
        child[l][3] <== s2 + xy[l]*(h[l]-s2);
        var parent = high_index * (4**(levels-1-l));
        for (var k = 2*(l+1); k < 2*levels; k++) {
            parent += bits[k] * (2**(k-2*(l+1)));
        }
        var aux = kind*(2**24) + frame*(2**20) + (start+l+1)*(2**16);
        hash[l] = Poseidon(5);
        hash[l].inputs[0] <== galaxyTag(4, aux) + parent;
        for (var c = 0; c < 4; c++) {
            hash[l].inputs[c+1] <== child[l][c];
        }
        h[l+1] <== hash[l].out;
    }
    root <== h[levels];
}

// FRAME openings use a global 64x48 grid in 4x4 blocks. This rejects
// both padding tile rows (including arbitrary prover-supplied Merkle paths).
// Policy independently equates these exact coordinates with its projection,
// falling back to block (0,0) when out of bounds.
template GalaxyFramePosition() {
    signal input bx;
    signal input by;
    signal output bits[12];
    signal output pos;
    component x = Num2Bits(6);
    component y = Num2Bits(6);
    x.in <== bx;
    y.in <== by;
    y.out[4] * y.out[5] === 0;
    var sum = 0;
    for (var i = 0; i < 6; i++) {
        bits[2*i] <== x.out[i];
        bits[2*i+1] <== y.out[i];
        sum += x.out[i] * (2**(2*i)) + y.out[i] * (2**(2*i+1));
    }
    pos <== sum;
}
