pragma circom 2.2.2;

include "planarity.circom";
include "temporal.circom";
include "pose.circom";
include "sensors.circom";

// Protocol §20. All physical predicates are computed; the caller constrains
// their conjunction to 1 after verifying the openings and sampling.
template GalaxyPolicyV1m() {
    signal input blk_r[64][16]; signal input blk_g[64][16]; signal input blk_b[64][16];
    signal input blk_d[64][16]; signal input blk_c[64][16];
    signal input blk_u0[64]; signal input blk_v0[64];
    signal input tmp_ix[32]; signal input tmp_iy[32];
    signal input hyp_idx[8][3];
    signal input pr_r[64][16]; signal input pr_g[64][16]; signal input pr_b[64][16]; signal input pr_d[64][16]; signal input pr_c[64][16];
    signal input pr_bx[64]; signal input pr_by[64];
    signal input fx[3]; signal input fy[3]; signal input cx[3]; signal input cy[3];
    signal input R[3][9]; signal input T[3][3]; signal input ts[3]; signal input track[3];
    signal input imu_dt[2][64]; signal input imu_w[2][64][3]; signal input g0[3]; signal input a_peak;
    signal output quality_ok; signal output geometry_ok; signal output rgb_depth_ok; signal output temporal_ok; signal output motion_ok;
    signal output st_V_all; signal output st_V_c; signal output st_n_hyp_ok_g; signal output st_n_hyp_ok_c; signal output st_max_all; signal output st_max_c;
    signal output st_j_star; signal output st_tiles_cov; signal output st_tiles_nonplanar; signal output st_tiles_nonplanar_c; signal output st_n_strong; signal output st_tiles_strong;
    signal output st_N_disc; signal output st_N_match; signal output st_N_usable; signal output st_N_occ; signal output st_N_dok; signal output st_N_inf; signal output st_N_pok;

    // Validity is read only for the 12 non-corner pixels of a block: the
    // planarity points and temporal sources are interior, and the §8.3
    // neighbours of interior pixels never reach a corner. Corner entries are
    // fixed to 0 and no predicate reads them.
    component pixelValid[64][16];
    signal valid[64][16];
    signal y[64][16];
    signal py[64][16];
    for (var b = 0; b < 64; b++) {
        for (var k = 0; k < 16; k++) {
            if (k == 0 || k == 3 || k == 12 || k == 15) {
                valid[b][k] <== 0;
            } else {
                pixelValid[b][k] = GeoPixelValid();
                pixelValid[b][k].d <== blk_d[b][k];
                pixelValid[b][k].c <== blk_c[b][k];
                valid[b][k] <== pixelValid[b][k].out;
            }
            y[b][k] <== 77*blk_r[b][k]+150*blk_g[b][k]+29*blk_b[b][k];
            py[b][k] <== 77*pr_r[b][k]+150*pr_g[b][k]+29*pr_b[b][k];
        }
    }

    component quality = GeoQuality();
    quality.fx <== fx;
    quality.fy <== fy;
    quality.cx <== cx;
    quality.cy <== cy;
    quality.track <== track;
    quality.R <== R;
    quality.ts <== ts;
    quality_ok <== quality.ok;

    component pose = GeoRelativePose();
    pose.R <== R;
    pose.T <== T;
    component motion = GeoMotion();
    motion.relativeT <== pose.relativeT;
    motion.vee <== pose.vee;
    motion.R1 <== R[1];
    motion.ts <== ts;
    motion.dt <== imu_dt;
    motion.omega <== imu_w;
    motion.g0 <== g0;
    motion.a_peak <== a_peak;
    motion_ok <== motion.ok;

    component planarity = GeoPlanarity();
    planarity.d <== blk_d;
    planarity.valid <== valid;
    planarity.u0 <== blk_u0;
    planarity.v0 <== blk_v0;
    planarity.hyp_idx <== hyp_idx;
    geometry_ok <== planarity.ok;
    st_V_all <== planarity.V_all;
    st_V_c <== planarity.V_c;
    st_n_hyp_ok_g <== planarity.n_hyp_ok_g;
    st_n_hyp_ok_c <== planarity.n_hyp_ok_c;
    st_max_all <== planarity.max_all;
    st_max_c <== planarity.max_c;
    st_j_star <== planarity.j_star;
    st_tiles_cov <== planarity.tiles_cov;
    st_tiles_nonplanar <== planarity.tiles_nonplanar;
    st_tiles_nonplanar_c <== planarity.tiles_nonplanar_c;
    st_n_strong <== planarity.n_strong;
    st_tiles_strong <== planarity.tiles_strong;

    component rgbDepth = GeoRgbDepth();
    rgbDepth.d <== blk_d;
    rgbDepth.y <== y;
    rgbDepth.valid <== valid;
    rgb_depth_ok <== rgbDepth.ok;
    st_N_disc <== rgbDepth.N_disc;
    st_N_match <== rgbDepth.N_match;

    component temporal = GeoTemporal();
    temporal.blk_d <== blk_d;
    temporal.blk_y <== y;
    temporal.blk_valid <== valid;
    temporal.blk_u0 <== blk_u0;
    temporal.blk_v0 <== blk_v0;
    temporal.tmp_ix <== tmp_ix;
    temporal.tmp_iy <== tmp_iy;
    temporal.pr_y <== py;
    temporal.pr_d <== pr_d;
    temporal.pr_c <== pr_c;
    temporal.pr_bx <== pr_bx;
    temporal.pr_by <== pr_by;
    temporal.fx <== fx;
    temporal.fy <== fy;
    temporal.cx <== cx;
    temporal.cy <== cy;
    temporal.relativeR <== pose.relativeR;
    temporal.relativeT <== pose.relativeT;
    temporal_ok <== temporal.ok;
    st_N_usable <== temporal.N_usable;
    st_N_occ <== temporal.N_occ;
    st_N_dok <== temporal.N_dok;
    st_N_inf <== temporal.N_inf;
    st_N_pok <== temporal.N_pok;
}
