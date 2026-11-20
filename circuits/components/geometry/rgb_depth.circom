pragma circom 2.2.2;

include "gadgets.circom";
include "../../physical-consistency/generated/policy_v1m.circom";

template GeoPixelValid() {
    signal input d;
    signal input c;
    signal output out;
    signal lower <== GeoGE(16)(d, POL_d_min_mm());
    signal upper <== GeoGE(16)(POL_d_max_mm(), d);
    signal confidence <== GeoGE(GeoBitsFor(GeoMax(3,POL_conf_min())))(c, POL_conf_min());
    out <== GeoAll(3)([lower, upper, confidence]);
}

template GeoRgbDepth() {
    signal input d[64][16];
    signal input y[64][16];
    signal input valid[64][16];
    signal output ok;
    signal output N_disc;
    signal output N_match;

    // dir=0: horizontal (vary x in the gradient); dir=1: vertical.
    // Along the gradient, x/y belongs to {1,2}; across it, y/x to {0..3}.
    var gradWidth = GeoBitsFor(65280 + POL_grad_min_y());
    var discWidth = GeoBitsFor(65535*200 + 131070*POL_disc_pct());
    signal grad[64][2][4][2];
    signal discontinuous[64][2][2][2];
    signal adjValid[64][2][2][2];
    signal disc[64][2][2][2];
    signal edge[64][2][2][2];
    signal matched[64][2][2][2];
    var discCount = 0;
    var matchCount = 0;
    for (var b = 0; b < 64; b++) {
        for (var dir = 0; dir < 2; dir++) {
            var h = dir == 0 ? 1 : 4;
            var v = dir == 0 ? 4 : 1;
            for (var row = 0; row < 4; row++) {
                for (var col = 0; col < 2; col++) {
                    var center = row*v + (col+1)*h;
                    grad[b][dir][row][col] <== GeoAbsGe(gradWidth)(y[b][center+h]-y[b][center-h], POL_grad_min_y());
                }
            }
            for (var row = 0; row < 2; row++) {
                for (var col = 0; col < 2; col++) {
                    var center = (row+1)*v + (col+1)*h;
                    var lo = center-h;
                    var hi = center+h;
                    adjValid[b][dir][row][col] <== valid[b][lo]*valid[b][hi];
                    discontinuous[b][dir][row][col] <== GeoAbsGe(discWidth)(200*(d[b][hi]-d[b][lo]), POL_disc_pct()*(d[b][hi]+d[b][lo]));
                    disc[b][dir][row][col] <== adjValid[b][dir][row][col]*discontinuous[b][dir][row][col];
                    edge[b][dir][row][col] <== GeoAny(3)([grad[b][dir][row][col], grad[b][dir][row+1][col], grad[b][dir][row+2][col]]);
                    matched[b][dir][row][col] <== disc[b][dir][row][col]*edge[b][dir][row][col];
                    discCount += disc[b][dir][row][col];
                    matchCount += matched[b][dir][row][col];
                }
            }
        }
    }
    N_disc <== discCount;
    N_match <== matchCount;
    signal enough <== GeoGE(GeoBitsFor(GeoMax(512,POL_disc_min())))(N_disc, POL_disc_min());
    signal ratio <== GeoGE(GeoBitsFor(512*GeoMax(100,POL_edge_match_pct())))(100*N_match, POL_edge_match_pct()*N_disc);
    ok <== enough*ratio;
}
