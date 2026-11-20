pragma circom 2.2.2;

include "gadgets.circom";
include "../../physical-consistency/generated/policy_v1m.circom";

// points: packed u + 2^8 v + 2^16 d + 2^32 valid. u < 256 and v < 192 hold
// for caller-derived block origins, d < 2^16 and valid is boolean, so the
// 33-bit decomposition of the selected entry recovers its four fields.
template GeoPlane(bits) {
    signal input packed[1<<bits];
    signal input idx[3];
    signal output coeff[3];
    signal output deg;
    signal choices[1<<bits][1];
    for (var i = 0; i < (1<<bits); i++) choices[i][0] <== packed[i];
    signal vertex[3][4];
    signal selector[3][bits];
    signal picked[3][1];
    signal fields[3][33];
    for (var i = 0; i < 3; i++) {
        selector[i] <== Num2Bits(bits)(idx[i]);
        picked[i] <== GeoMux(bits, 1)(selector[i], choices);
        fields[i] <== Num2Bits(33)(picked[i][0]);
        var u = 0;
        var v = 0;
        var d = 0;
        for (var k = 0; k < 8; k++) {
            u += fields[i][k] * (1 << k);
            v += fields[i][8+k] * (1 << k);
        }
        for (var k = 0; k < 16; k++) d += fields[i][16+k] * (1 << k);
        vertex[i] <== [u, v, d, fields[i][32]];
    }
    signal same[3];
    signal uv[3];
    signal uvFlip[3];
    signal w[3];
    for (var i = 0; i < 3; i++) {
        var a = (i+1)%3;
        var b = (i+2)%3;
        same[i] <== IsEqual()([idx[a], idx[b]]);
        uvFlip[i] <== vertex[b][0]*vertex[a][1];
        uv[i] <== vertex[a][0]*vertex[b][1] - uvFlip[i];
        w[i] <== vertex[a][2]*vertex[b][2];
    }
    component area = GeoAbs(18);
    area.in <== uv[0]+uv[1]+uv[2];
    signal areaBad <== GeoLT(GeoBitsFor(GeoMax(3*255*191,POL_area_min())))(area.out, POL_area_min());
    signal active <== GeoAll(7)([1-same[0], 1-same[1], 1-same[2], vertex[0][3], vertex[1][3], vertex[2][3], 1-areaBad]);
    signal signedActive <== active*(1-2*area.neg);
    signal dp <== w[0]*vertex[0][2];
    signal rawDen <== dp*area.out;
    signal den <== 1 + active*(rawDen-1);
    signal terms[3][3];
    for (var i = 0; i < 3; i++) {
        var a = (i+1)%3;
        var b = (i+2)%3;
        terms[0][i] <== w[i]*(vertex[a][1]-vertex[b][1]);
        terms[1][i] <== w[i]*(vertex[b][0]-vertex[a][0]);
        terms[2][i] <== w[i]*uv[i];
    }
    signal num[3];
    component fit[3];
    signal coeffOk[3];
    for (var i = 0; i < 3; i++) {
        num[i] <== POL_p()*signedActive*(terms[i][0]+terms[i][1]+terms[i][2]);
        fit[i] = GeoFloor(83,57);
        fit[i].num <== num[i];
        fit[i].den <== den;
        coeffOk[i] <== GeoAbsLe(84)(fit[i].q, POL_coefficient_bound()-1);
    }
    deg <== GeoAny(4)([1-active, 1-coeffOk[0], 1-coeffOk[1], 1-coeffOk[2]]);
    for (var i = 0; i < 3; i++) {
        coeff[i] <== (1-deg)*fit[i].q;
    }
}

template GeoResidual() {
    signal input coeff[3];
    signal input u;
    signal input v;
    signal input d; // zero for invalid points, otherwise under 2^13
    signal output out;
    signal au <== coeff[0]*u;
    signal bv <== coeff[1]*v;
    out <== POL_p()-d*(au+bv+coeff[2]);
}

// residualBits are the bits of res + 2^58 (|res| < 2^58, see GEOMETRY.md);
// the caller derives further bands of the same residual from them.
template GeoPlaneSupport(count) {
    signal input points[count][4];
    signal input maskedD[count];
    signal input coeff[3];
    signal input deg;
    signal output supportAll;
    signal output supportCentral;
    signal output inlier[count];
    signal output eligible[count];
    signal output residualBits[count][59];
    signal residual[count];
    signal eps[count];
    var central = 0;
    var whole = 0;
    for (var i = 0; i < count; i++) {
        residual[i] <== GeoResidual()(coeff, points[i][0], points[i][1], maskedD[i]);
        residualBits[i] <== GeoSignedBits(58)(residual[i]);
        eps[i] <== GeoBand(58, POL_eps_log2())(residualBits[i]);
        eligible[i] <== points[i][3]*(1-deg);
        inlier[i] <== eligible[i]*eps[i];
        whole += inlier[i];
        if (i < 128) central += inlier[i];
    }
    supportAll <== whole;
    supportCentral <== central;
}

template GeoPlanarity() {
    signal input d[64][16];
    signal input valid[64][16];
    signal input u0[64];
    signal input v0[64];
    signal input hyp_idx[8][3];
    signal output ok;
    signal output V_all;
    signal output V_c;
    signal output n_hyp_ok_g;
    signal output n_hyp_ok_c;
    signal output max_all;
    signal output max_c;
    signal output j_star;
    signal output tiles_cov;
    signal output tiles_nonplanar;
    signal output tiles_nonplanar_c;
    signal output n_strong;
    signal output tiles_strong;

    signal points[256][4];
    signal central[128][4];
    signal masked[256];
    signal maskedCentral[128];
    signal packed[256];
    signal packedCentral[128];
    for (var b = 0; b < 64; b++) {
        for (var k = 0; k < 4; k++) {
            var i = 4*b+k;
            var x = 1+(k%2);
            var y = 1+(k\2);
            var p = 4*y+x;
            points[i] <== [u0[b]+x, v0[b]+y, d[b][p], valid[b][p]];
            masked[i] <== d[b][p]*valid[b][p];
            packed[i] <== points[i][0] + (1<<8)*points[i][1] + (1<<16)*points[i][2] + (1<<32)*points[i][3];
            if (i < 128) {
                central[i] <== points[i];
                maskedCentral[i] <== masked[i];
                packedCentral[i] <== packed[i];
            }
        }
    }
    component plane[8];
    component support[8];
    signal supportAll[4];
    signal supportCentral[8];
    var hypoGlobal = 0;
    var hypoCentral = 0;
    for (var j = 0; j < 8; j++) {
        if (j < 4) {
            plane[j] = GeoPlane(8);
            plane[j].packed <== packed;
            support[j] = GeoPlaneSupport(256);
            support[j].points <== points;
            support[j].maskedD <== masked;
        } else {
            plane[j] = GeoPlane(7);
            plane[j].packed <== packedCentral;
            support[j] = GeoPlaneSupport(128);
            support[j].points <== central;
            support[j].maskedD <== maskedCentral;
        }
        plane[j].idx <== hyp_idx[j];
        support[j].coeff <== plane[j].coeff;
        support[j].deg <== plane[j].deg;
        supportCentral[j] <== support[j].supportCentral;
        if (j < 4) {
            supportAll[j] <== support[j].supportAll;
            hypoGlobal += 1-plane[j].deg;
        } else {
            hypoCentral += 1-plane[j].deg;
        }
    }
    n_hyp_ok_g <== hypoGlobal;
    n_hyp_ok_c <== hypoCentral;
    component g = GeoFirstMax(4,9);
    g.in <== supportAll;
    component c = GeoFirstMax(8,8);
    c.in <== supportCentral;
    max_all <== g.max;
    max_c <== c.max;
    j_star <== g.index;
    signal starBits[2] <== Num2Bits(2)(j_star);

    // For every global hypothesis j: strong_ij = valid_i ∧ ¬deg_j ∧ ¬sep band,
    // from the residual bits already decomposed for the inlier band. The
    // j_star entries of inlier and strong are then selected per point.
    signal separated[4][256];
    signal strongChoice[4][256];
    signal choices[256][4][2];
    signal picked[256][2];
    signal outlier[256];
    signal strong[256];
    signal slotValid[16];
    signal slotOut[16];
    signal slotStrong[16];
    signal covered[16];
    signal nonplanar[16];
    signal strongTile[16];
    var nValid = 0;
    var nValidC = 0;
    var nStrong = 0;
    var nNonplanarC = 0;
    for (var s = 0; s < 16; s++) {
        var v = 0;
        var o = 0;
        var t = 0;
        for (var k = 0; k < 16; k++) {
            var i = 16*s+k;
            for (var j = 0; j < 4; j++) {
                separated[j][i] <== GeoBand(58, POL_sep_log2())(support[j].residualBits[i]);
                strongChoice[j][i] <== support[j].eligible[i]*(1-separated[j][i]);
                choices[i][j] <== [support[j].inlier[i], strongChoice[j][i]];
            }
            picked[i] <== GeoMux(2,2)(starBits, choices[i]);
            // An inlier of this point implies its validity.
            outlier[i] <== points[i][3] - picked[i][0];
            strong[i] <== picked[i][1];
            v += points[i][3];
            o += outlier[i];
            t += strong[i];
        }
        slotValid[s] <== v;
        slotOut[s] <== o;
        slotStrong[s] <== t;
        covered[s] <== GeoGE(GeoBitsFor(GeoMax(16,POL_tile_valid_min())))(slotValid[s], POL_tile_valid_min());
        nonplanar[s] <== GeoGE(GeoBitsFor(GeoMax(16,POL_tile_out_min())))(slotOut[s], POL_tile_out_min());
        strongTile[s] <== GeoNonzero()(slotStrong[s]);
        nValid += slotValid[s];
        nStrong += slotStrong[s];
        if (s < 8) {
            nValidC += slotValid[s];
            nNonplanarC += nonplanar[s];
        }
    }
    V_all <== nValid;
    V_c <== nValidC;
    n_strong <== nStrong;
    tiles_cov <== GeoCount(16)(covered);
    tiles_nonplanar <== GeoCount(16)(nonplanar);
    tiles_nonplanar_c <== nNonplanarC;
    tiles_strong <== GeoCount(16)(strongTile);

    signal pass[11];
    pass[0] <== GeoGE(GeoBitsFor(GeoMax(256,POL_v_all_min())))(V_all,POL_v_all_min());
    pass[1] <== GeoGE(GeoBitsFor(GeoMax(128,POL_v_c_min())))(V_c,POL_v_c_min());
    pass[2] <== GeoGE(GeoBitsFor(GeoMax(16,POL_tiles_cov_min())))(tiles_cov,POL_tiles_cov_min());
    pass[3] <== GeoNonzero()(n_hyp_ok_g);
    pass[4] <== GeoNonzero()(n_hyp_ok_c);
    pass[5] <== GeoGE(GeoBitsFor(256*GeoMax(100,POL_plane_global_max_pct())))(POL_plane_global_max_pct()*V_all,100*max_all);
    pass[6] <== GeoGE(GeoBitsFor(128*GeoMax(100,POL_plane_central_max_pct())))(POL_plane_central_max_pct()*V_c,100*max_c);
    pass[7] <== GeoGE(GeoBitsFor(GeoMax(16,POL_nonplanar_tiles_min())))(tiles_nonplanar,POL_nonplanar_tiles_min());
    pass[8] <== GeoGE(GeoBitsFor(GeoMax(8,POL_nonplanar_tiles_c_min())))(tiles_nonplanar_c,POL_nonplanar_tiles_c_min());
    pass[9] <== GeoGE(GeoBitsFor(256*GeoMax(100,POL_strong_min_pct())))(100*n_strong,POL_strong_min_pct()*V_all);
    pass[10] <== GeoGE(GeoBitsFor(GeoMax(16,POL_strong_tiles_min())))(tiles_strong,POL_strong_tiles_min());
    ok <== GeoAll(11)(pass);
}
