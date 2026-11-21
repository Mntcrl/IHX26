pragma circom 2.2.3;

include "../../components/core/sampling.circom";
include "../../components/core/openings.circom";
include "../../components/core/scalars.circom";
include "../../components/core/commitment.circom";

// Use the same root binding/ports as main, exposing only values the test
// needs to cross-check against an independently built path or unpacking.
template GalaxyTileAudit(kind) {
    signal input tile;
    signal input block[4][2];
    signal input packed[4][2];
    signal input siblings[4][3][3];
    signal input upper[3][3];
    signal input root;
    signal output pixels[4][48*(kind == 1) + 32*(kind == 2)];
    component pos = GalaxyTile(1);
    pos.label <== tile;
    component tile_path = GalaxyCenterTile(kind);
    for (var k = 0; k < 3; k++) {
        tile_path.tile_bits[2*k] <== pos.x[k];
        tile_path.tile_bits[2*k+1] <== pos.y[k];
    }
    component bx[4];
    component by[4];
    for (var q = 0; q < 4; q++) {
        bx[q] = Num2Bits(3);
        by[q] = Num2Bits(3);
        bx[q].in <== block[q][0];
        by[q].in <== block[q][1];
        for (var k = 0; k < 3; k++) {
            tile_path.block_bits[q][2*k] <== bx[q].out[k];
            tile_path.block_bits[q][2*k+1] <== by[q].out[k];
        }
    }
    tile_path.packed <== packed;
    tile_path.block_siblings <== siblings;
    tile_path.tile_siblings <== upper;
    tile_path.root === root;
    if (kind == 1) { pixels <== tile_path.bytes; }
    else {
        for (var q = 0; q < 4; q++) {
            for (var k = 0; k < 16; k++) {
                pixels[q][2*k] <== tile_path.d[q][k];
                pixels[q][2*k+1] <== tile_path.c[q][k];
            }
        }
    }
}

// One frame-1 tile wired to the challenge exactly as in main_v1m.
template GalaxySampledTileAudit() {
    signal input limbs[4];
    signal input sampled_tiles[16];
    signal input packed[4][2];
    signal input siblings[4][3][3];
    signal input upper[3][3];
    signal input root;
    component sampling = GalaxySampling();
    sampling.limbs <== limbs;
    sampling.sampled_tiles <== sampled_tiles;
    component opening = GalaxyCenterTile(1);
    opening.tile_bits <== sampling.tile[0];
    for (var q = 0; q < 4; q++) {
        opening.block_bits[q] <== sampling.blk[q];
    }
    opening.packed <== packed;
    opening.block_siblings <== siblings;
    opening.tile_siblings <== upper;
    opening.root === root;
}

template GalaxyFrameAudit(frame) {
    signal input bx;
    signal input by;
    signal input rgb[2];
    signal input depth[2];
    signal input siblings[6][3];
    signal input root;
    signal output d[16];
    signal output c[16];
    signal output bytes[48];
    component opening = GalaxyTemporalBlock(frame);
    opening.bx <== bx;
    opening.by <== by;
    opening.rgb <== rgb;
    opening.depth <== depth;
    opening.siblings <== siblings;
    opening.root === root;
    d <== opening.d;
    c <== opening.c;
    bytes <== opening.bytes;
}

template GalaxyCommitmentAudit() {
    signal input rgb_root;
    signal input roots[3];
    signal input salts[8];
    signal input camera[54];
    signal input imu[133];
    signal input bluetooth[24];
    signal input noise[18];
    signal input meta[9];
    signal input evidence_root;
    component commitment = GalaxyEvidenceCommitment();
    commitment.rgb_root <== rgb_root;
    commitment.roots <== roots;
    commitment.salts <== salts;
    commitment.camera <== camera;
    commitment.imu <== imu;
    commitment.bluetooth <== bluetooth;
    commitment.noise <== noise;
    commitment.meta <== meta;
    commitment.root === evidence_root;
    component cal = GalaxyCamera();
    cal.e <== camera;
    _ <== [cal.fx, cal.fy, cal.cx, cal.cy, cal.ts, cal.track];
    _ <== cal.R;
    _ <== cal.T;
    component sensor = GalaxyImu();
    sensor.e <== imu;
    _ <== sensor.g0;
    _ <== sensor.a_peak;
    _ <== sensor.imu_dt;
    _ <== sensor.imu_w;
}

// Used only to recompute a non-canonical AliasCheck trace that the negative
// tests copy into the production sampling R1CS witness; witness generation is
// never relied upon by the constraint-level checks.
template AliasTrace() {
    signal input bits[254];
    component alias = AliasCheck();
    alias.in <== bits;
}
