pragma circom 2.2.3;

include "../components/core/sampling.circom";
include "../components/core/openings.circom";
include "../components/core/scalars.circom";
include "../components/core/commitment.circom";
include "../components/geometry/policy_v1m.circom";

template GalaxyPhysicalConsistencyProofV1() {
    signal input rgb_root;
    signal input evidence_root;
    signal input capture_id_hi;
    signal input capture_id_lo;
    signal input seed_hi;
    signal input seed_lo;
    signal input sampled_tiles[16];

    signal input roots[3]; // DEPTH0, FRAME_MINUS, FRAME_PLUS
    signal input salts[8]; // salted slots, in slot order
    signal input camera[54];
    signal input imu[133];
    signal input bluetooth[24];
    signal input noise[18];
    signal input meta[9];
    signal input blk_rgb[16][4][2];
    signal input blk_depth[16][4][2];
    signal input blk_rgb_siblings[16][4][3][3];
    signal input blk_depth_siblings[16][4][3][3];
    signal input tile_rgb_siblings[16][3][3];
    signal input tile_depth_siblings[16][3][3];
    signal input pr_bx[64];
    signal input pr_by[64];
    signal input pr_rgb[64][2];
    signal input pr_depth[64][2];
    signal input pr_siblings[64][6][3];

    component sampling = GalaxySampling();
    sampling.limbs <== [capture_id_hi, capture_id_lo, seed_hi, seed_lo];
    sampling.sampled_tiles <== sampled_tiles;

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
    component sensors = GalaxyImu();
    sensors.e <== imu;
    component policy = GalaxyPolicyV1m();
    policy.fx <== cal.fx;
    policy.fy <== cal.fy;
    policy.cx <== cal.cx;
    policy.cy <== cal.cy;
    policy.R <== cal.R;
    policy.T <== cal.T;
    policy.ts <== cal.ts;
    policy.track <== cal.track;
    policy.imu_dt <== sensors.imu_dt;
    policy.imu_w <== sensors.imu_w;
    policy.g0 <== sensors.g0;
    policy.a_peak <== sensors.a_peak;
    policy.blk_u0 <== sampling.blk_u0;
    policy.blk_v0 <== sampling.blk_v0;
    policy.tmp_ix <== sampling.tmp_ix;
    policy.tmp_iy <== sampling.tmp_iy;
    policy.hyp_idx <== sampling.hyp_idx;
    policy.pr_bx <== pr_bx;
    policy.pr_by <== pr_by;

    component rgb_tile[16];
    component depth_tile[16];
    for (var s = 0; s < 16; s++) {
        rgb_tile[s] = GalaxyCenterTile(1);
        depth_tile[s] = GalaxyCenterTile(2);
        rgb_tile[s].tile_bits <== sampling.tile[s];
        depth_tile[s].tile_bits <== sampling.tile[s];
        rgb_tile[s].packed <== blk_rgb[s];
        depth_tile[s].packed <== blk_depth[s];
        rgb_tile[s].block_siblings <== blk_rgb_siblings[s];
        depth_tile[s].block_siblings <== blk_depth_siblings[s];
        rgb_tile[s].tile_siblings <== tile_rgb_siblings[s];
        depth_tile[s].tile_siblings <== tile_depth_siblings[s];
        for (var q = 0; q < 4; q++) {
            rgb_tile[s].block_bits[q] <== sampling.blk[4*s+q];
            depth_tile[s].block_bits[q] <== sampling.blk[4*s+q];
        }
        rgb_tile[s].root === rgb_root;
        depth_tile[s].root === roots[0];
        for (var q = 0; q < 4; q++) {
            var b = 4*s+q;
            for (var k = 0; k < 16; k++) {
                policy.blk_r[b][k] <== rgb_tile[s].bytes[q][3*k];
                policy.blk_g[b][k] <== rgb_tile[s].bytes[q][3*k+1];
                policy.blk_b[b][k] <== rgb_tile[s].bytes[q][3*k+2];
            }
            policy.blk_d[b] <== depth_tile[s].d[q];
            policy.blk_c[b] <== depth_tile[s].c[q];
        }
    }
    component temporal[64];
    for (var p = 0; p < 64; p++) {
        var side = p % 2;
        temporal[p] = GalaxyTemporalBlock(2*side);
        temporal[p].bx <== pr_bx[p];
        temporal[p].by <== pr_by[p];
        temporal[p].rgb <== pr_rgb[p];
        temporal[p].depth <== pr_depth[p];
        temporal[p].siblings <== pr_siblings[p];
        temporal[p].root === roots[1+side];
        for (var k = 0; k < 16; k++) {
            policy.pr_r[p][k] <== temporal[p].bytes[3*k];
            policy.pr_g[p][k] <== temporal[p].bytes[3*k+1];
            policy.pr_b[p][k] <== temporal[p].bytes[3*k+2];
        }
        policy.pr_d[p] <== temporal[p].d;
        policy.pr_c[p] <== temporal[p].c;
    }

    // Selection and commitments are hard constraints above. No public output.
    signal first <== policy.quality_ok * policy.geometry_ok;
    signal second <== policy.rgb_depth_ok * policy.temporal_ok;
    signal sensor_ok <== first * policy.motion_ok;
    signal PASS <== sensor_ok * second;
    PASS === 1;
    _ <== [policy.st_V_all, policy.st_V_c, policy.st_n_hyp_ok_g, policy.st_n_hyp_ok_c,
        policy.st_max_all, policy.st_max_c, policy.st_j_star, policy.st_tiles_cov,
        policy.st_tiles_nonplanar, policy.st_tiles_nonplanar_c, policy.st_n_strong,
        policy.st_tiles_strong, policy.st_N_disc, policy.st_N_match, policy.st_N_usable,
        policy.st_N_occ, policy.st_N_dok, policy.st_N_inf, policy.st_N_pok];
}

component main {public [rgb_root, evidence_root, capture_id_hi, capture_id_lo, seed_hi, seed_lo, sampled_tiles]} = GalaxyPhysicalConsistencyProofV1();
