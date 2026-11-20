pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";
include "tags.circom";

// Compact dimensions: the low three tile bits are x, the high three are y.
template GalaxyTile(central) {
    signal input label;
    signal output x[3];
    signal output y[3];
    component bits = Num2Bits(6);
    bits.in <== label;
    for (var k = 0; k < 3; k++) {
        x[k] <== bits.out[k];
        y[k] <== bits.out[k + 3];
    }
    y[1] * y[2] === 0; // y < 6 (and therefore label < 48)
    // For x=2..5 the middle and high bits differ. With y<6, exclude y=0,5.
    signal central_x <== x[1] + x[2] - 2*x[1]*x[2];
    signal y01 <== y[0] + y[1] - y[0]*y[1];
    signal central_y <== y01 + y[2] - 2*y[2]*y01;
    central_x * central_y === central;
}

template GalaxySampling() {
    signal input limbs[4];
    signal input sampled_tiles[16];
    signal output tile[16][6]; // Morton bits, with x first in each digit
    signal output blk[64][6];
    signal output blk_u0[64];
    signal output blk_v0[64];
    signal output tmp_ix[32];
    signal output tmp_iy[32];
    signal output hyp_idx[8][3];

    component context = Poseidon(5);
    context.inputs[0] <== galaxyTag(10, 0);
    component limb_range[4];
    for (var i = 0; i < 4; i++) {
        limb_range[i] = Num2Bits(128);
        limb_range[i].in <== limbs[i];
        _ <== limb_range[i].out;
        context.inputs[i + 1] <== limbs[i];
    }
    component position[16];
    component asc[14];
    component sample_hash[16];
    component sample_bits[16];
    for (var s = 0; s < 16; s++) {
        position[s] = GalaxyTile(s < 8);
        position[s].label <== sampled_tiles[s];
        var tx = 0;
        var ty = 0;
        for (var k = 0; k < 3; k++) {
            tile[s][2*k] <== position[s].x[k];
            tile[s][2*k+1] <== position[s].y[k];
            tx += position[s].x[k] * (2**k);
            ty += position[s].y[k] * (2**k);
        }
        if (s % 8 != 0) {
            var i = s - 1 - (s \ 8);
            asc[i] = LessThan(6);
            asc[i].in <== [sampled_tiles[s - 1], sampled_tiles[s]];
            asc[i].out === 1;
        }
        sample_hash[s] = Poseidon(3);
        sample_hash[s].inputs <== [galaxyTag(11, s), context.out, sampled_tiles[s]];
        sample_bits[s] = Num2Bits_strict();
        sample_bits[s].in <== sample_hash[s].out;
        for (var q = 0; q < 4; q++) {
            var b = 4*s + q;
            // The hash supplies rx then ry; interleave them for the Merkle path.
            for (var k = 0; k < 2; k++) {
                blk[b][2*k] <== sample_bits[s].out[4*q + k];
                blk[b][2*k+1] <== sample_bits[s].out[4*q + k + 2];
            }
            blk_u0[b] <== 32*tx + 4*(4*(q % 2) + blk[b][0] + 2*blk[b][2]);
            blk_v0[b] <== 32*ty + 4*(4*(q \ 2) + blk[b][1] + 2*blk[b][3]);
            blk[b][4] <== q % 2;
            blk[b][5] <== q \ 2;
            if (q == 0 || q == 3) {
                tmp_ix[2*s + (q \ 3)] <== 1 + sample_bits[s].out[16 + 2*q];
                tmp_iy[2*s + (q \ 3)] <== 1 + sample_bits[s].out[17 + 2*q];
            } else {
                _ <== sample_bits[s].out[16 + 2*q];
                _ <== sample_bits[s].out[17 + 2*q];
            }
        }
        for (var k = 24; k < 254; k++) {
            _ <== sample_bits[s].out[k];
        }
    }
    component hyp_hash[8];
    component hyp_bits[8];
    for (var j = 0; j < 8; j++) {
        hyp_hash[j] = Poseidon(2);
        hyp_hash[j].inputs <== [galaxyTag(12, j), context.out];
        hyp_bits[j] = Num2Bits_strict();
        hyp_bits[j].in <== hyp_hash[j].out;
        var n = 7 + (j < 4);
        for (var m = 0; m < 3; m++) {
            var idx = 0;
            for (var k = 0; k < n; k++) {
                idx += hyp_bits[j].out[n*m+k] * (2**k);
            }
            hyp_idx[j][m] <== idx;
        }
        for (var k = 3*n; k < 254; k++) {
            _ <== hyp_bits[j].out[k];
        }
    }
}
