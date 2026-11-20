pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";
include "tags.circom";

// Each 192-bit limb encodes 24 bytes in big-endian order.
template GalaxyRgbUnpack() {
    signal input packed[2];
    signal output bytes[48];
    component range[2];
    for (var l = 0; l < 2; l++) {
        range[l] = Num2Bits(192);
        range[l].in <== packed[l];
        for (var i = 0; i < 24; i++) {
            var byte = 0;
            for (var b = 0; b < 8; b++) {
                byte += (2**b) * range[l].out[8*(23-i)+b];
            }
            bytes[24*l+i] <== byte;
        }
    }
}

// Each 144-bit limb encodes eight 18-bit entries in big-endian order.
template GalaxyDepthUnpack() {
    signal input packed[2];
    signal output d[16];
    signal output c[16];
    component range[2];
    for (var l = 0; l < 2; l++) {
        range[l] = Num2Bits(144);
        range[l].in <== packed[l];
        for (var i = 0; i < 8; i++) {
            var base = 18*(7-i);
            var depth = 0;
            for (var b = 0; b < 16; b++) {
                depth += (2**b) * range[l].out[base+2+b];
            }
            d[8*l+i] <== depth;
            c[8*l+i] <== range[l].out[base] + 2*range[l].out[base+1];
        }
    }
}

// Kind is one of RGB (1), DEPTH (2), FRAME (3); pos is derived by caller.
template GalaxyLeaf(kind, frame) {
    signal input pos;
    signal input rgb[2 * (kind != 2)];
    signal input depth[2 * (kind != 1)];
    signal output leaf;
    signal output bytes[48 * (kind != 2)];
    signal output d[16 * (kind != 1)];
    signal output c[16 * (kind != 1)];
    component hash = Poseidon(3 + 2*(kind == 3));
    hash.inputs[0] <== galaxyTag(kind, frame*4096) + pos;
    if (kind != 2) {
        component ru = GalaxyRgbUnpack();
        ru.packed <== rgb;
        bytes <== ru.bytes;
        hash.inputs[1] <== rgb[0];
        hash.inputs[2] <== rgb[1];
    }
    if (kind != 1) {
        component du = GalaxyDepthUnpack();
        du.packed <== depth;
        d <== du.d;
        c <== du.c;
        hash.inputs[1 + 2*(kind == 3)] <== depth[0];
        hash.inputs[2 + 2*(kind == 3)] <== depth[1];
    }
    leaf <== hash.out;
}
