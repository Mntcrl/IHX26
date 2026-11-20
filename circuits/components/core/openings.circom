pragma circom 2.2.3;

include "leaf.circom";
include "merkle.circom";

// One tile-to-root opening per sampled tile and tree. All four sampled
// leaves must independently hash to the SAME level-3 tile node first.
template GalaxyCenterTile(kind) {
    signal input tile_bits[6];
    signal input block_bits[4][6];
    signal input packed[4][2];
    signal input block_siblings[4][3][3];
    signal input tile_siblings[3][3];
    signal output root;
    signal output bytes[4][48*(kind == 1)];
    signal output d[4][16*(kind == 2)];
    signal output c[4][16*(kind == 2)];
    var tile_pos = 0;
    for (var k = 0; k < 6; k++) {
        tile_pos += tile_bits[k]*(2**k);
    }
    component leaves[4];
    component inner[4];
    for (var q = 0; q < 4; q++) {
        var pos = 64*tile_pos;
        for (var k = 0; k < 6; k++) {
            pos += block_bits[q][k]*(2**k);
        }
        leaves[q] = GalaxyLeaf(kind, 1);
        leaves[q].pos <== pos;
        if (kind == 1) {
            leaves[q].rgb <== packed[q];
            bytes[q] <== leaves[q].bytes;
        } else {
            leaves[q].depth <== packed[q];
            d[q] <== leaves[q].d;
            c[q] <== leaves[q].c;
        }
        inner[q] = GalaxyMerklePath(kind, 1, 0, 3);
        inner[q].bits <== block_bits[q];
        inner[q].siblings <== block_siblings[q];
        inner[q].high_index <== tile_pos;
        inner[q].leaf <== leaves[q].leaf;
        if (q != 0) {
            inner[q].root === inner[0].root;
        }
    }
    component upper = GalaxyMerklePath(kind, 1, 3, 3);
    upper.bits <== tile_bits;
    upper.siblings <== tile_siblings;
    upper.high_index <== 0;
    upper.leaf <== inner[0].root;
    root <== upper.root;
}

template GalaxyTemporalBlock(frame) {
    signal input bx;
    signal input by;
    signal input rgb[2];
    signal input depth[2];
    signal input siblings[6][3];
    signal output root;
    signal output bytes[48];
    signal output d[16];
    signal output c[16];
    component position = GalaxyFramePosition();
    position.bx <== bx;
    position.by <== by;
    component leaf = GalaxyLeaf(3, frame);
    leaf.pos <== position.pos;
    leaf.rgb <== rgb;
    leaf.depth <== depth;
    bytes <== leaf.bytes;
    d <== leaf.d;
    c <== leaf.c;
    component path = GalaxyMerklePath(3, frame, 0, 6);
    path.bits <== position.bits;
    path.siblings <== siblings;
    path.high_index <== 0;
    path.leaf <== leaf.leaf;
    root <== path.root;
}
