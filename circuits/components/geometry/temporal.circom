pragma circom 2.2.2;

include "rgb_depth.circom";

template GeoTemporalPair() {
    signal input P[3];
    signal input scaledF; // 2^30 * fx_1 * fy_1
    signal input relativeR[9];
    signal input relativeT[3];
    signal input intrinsic[4]; // fx, fy, cx, cy of the reference frame j
    signal input sourceValid;
    signal input sourceY;
    signal input informative;
    signal input y[16];
    signal input d[16];
    signal input c[16];
    signal input bx;
    signal input by;
    signal output usable;
    signal output occluded;
    signal output depthOk;
    signal output nonOccInformative;
    signal output photoOk;

    signal qRot[3][3];
    signal qTrans[3];
    signal Q[3];
    for (var a = 0; a < 3; a++) {
        for (var k = 0; k < 3; k++) qRot[a][k] <== relativeR[3*a+k]*P[k];
        qTrans[a] <== scaledF*relativeT[a];
        Q[a] <== 1000*(qRot[a][0]+qRot[a][1]+qRot[a][2]) + qTrans[a];
    }

    signal den <== (1<<16)*Q[2];
    signal zpos <== GeoLT(118)(0,Q[2]);
    signal numerator[2];
    signal perspective[2];
    signal inside[2];
    signal nonnegative[2];
    signal below[2];
    for (var a = 0; a < 2; a++) {
        var bound = a == 0 ? 256 : 192;
        perspective[a] <== intrinsic[a]*Q[a];
        numerator[a] <== perspective[a]+(intrinsic[2+a]+(1<<15))*Q[2];
        nonnegative[a] <== GeoGE(146)(numerator[a],0);
        below[a] <== GeoLT(147)(numerator[a],bound*den);
        inside[a] <== nonnegative[a]*below[a];
    }
    signal inb <== GeoAll(3)([zpos,inside[0],inside[1]]);
    component guarded[2];
    component quotient[2];
    signal posBits[2][8];
    signal projectedBlock[2];
    for (var a = 0; a < 2; a++) {
        guarded[a] = GeoSafeDen();
        guarded[a].enabled <== inb;
        guarded[a].num <== numerator[a];
        guarded[a].den <== den;
        quotient[a] = GeoUnsignedFloor(8,134);
        quotient[a].num <== guarded[a].safeNum;
        quotient[a].den <== guarded[a].safeDen;
        posBits[a] <== quotient[a].bits;
        var block = 0;
        for (var k = 2; k < 8; k++) block += posBits[a][k]*(1<<(k-2));
        projectedBlock[a] <== block;
    }
    bx === projectedBlock[0];
    by === projectedBlock[1];

    signal target[16][2];
    for (var k = 0; k < 16; k++) target[k] <== [d[k],c[k]];
    signal pixel[2] <== GeoMux(4,2)([posBits[0][0],posBits[0][1],posBits[1][0],posBits[1][1]],target);
    signal targetValid <== GeoPixelValid()(pixel[0],pixel[1]);
    usable <== GeoAll(3)([inb,sourceValid,targetValid]);

    signal dm <== (1000*scaledF)*pixel[0];
    signal front <== GeoLT(129)(1000*dm,(1000-POL_occ_pm())*Q[2]);
    occluded <== usable*front;
    signal clear <== usable - occluded;
    var depthWidth = GeoBitsFor((1<<129) + (1<<108)*POL_dcons_pm());
    signal closeDepth <== GeoAbsLe(depthWidth)(1000*(dm-Q[2]),POL_dcons_pm()*dm);
    depthOk <== clear*closeDepth;
    nonOccInformative <== clear*informative;
    signal photoPixel[16];
    for (var k = 0; k < 16; k++) {
        photoPixel[k] <== GeoAbsLe(GeoBitsFor(65280+POL_photo_tol_y()))(sourceY-y[k],POL_photo_tol_y());
    }
    signal anyPhoto <== GeoAny(16)(photoPixel);
    photoOk <== nonOccInformative*anyPhoto;
}

template GeoTemporal() {
    signal input blk_d[64][16];
    signal input blk_y[64][16];
    signal input blk_valid[64][16];
    signal input blk_u0[64];
    signal input blk_v0[64];
    signal input tmp_ix[32];
    signal input tmp_iy[32];
    signal input pr_y[64][16];
    signal input pr_d[64][16];
    signal input pr_c[64][16];
    signal input pr_bx[64];
    signal input pr_by[64];
    signal input fx[3];
    signal input fy[3];
    signal input cx[3];
    signal input cy[3];
    signal input relativeR[2][9];
    signal input relativeT[2][3];
    signal output ok;
    signal output N_usable;
    signal output N_occ;
    signal output N_dok;
    signal output N_inf;
    signal output N_pok;

    signal F <== fx[1]*fy[1];
    signal candidates[32][4][7];
    signal source[32][7];
    signal hasGrad[32][4];
    signal anyGrad[32];
    signal informative[32];
    signal pointX[32];
    signal pointY[32];
    signal P[32][3];
    for (var c = 0; c < 32; c++) {
        var b = 4*(c\2)+POL_temporal_quadrants(c%2);
        for (var i = 0; i < 4; i++) {
            var k = 5+(i%2)+4*(i\2);
            candidates[c][i] <== [blk_d[b][k],blk_valid[b][k],blk_y[b][k],blk_y[b][k-1],blk_y[b][k+1],blk_y[b][k-4],blk_y[b][k+4]];
        }
        // Caller-derived offsets are in {1,2}; this keeps the mux selectors boolean.
        (tmp_ix[c]-1)*(tmp_ix[c]-2) === 0;
        (tmp_iy[c]-1)*(tmp_iy[c]-2) === 0;
        source[c] <== GeoMux(2,7)([tmp_ix[c]-1,tmp_iy[c]-1],candidates[c]);
        for (var i = 0; i < 4; i++) {
            hasGrad[c][i] <== GeoAbsGe(GeoBitsFor(65280+POL_g_inf_y()))(source[c][2]-source[c][3+i],POL_g_inf_y());
        }
        anyGrad[c] <== GeoAny(4)(hasGrad[c]);
        informative[c] <== source[c][1]*anyGrad[c];
        pointX[c] <== (((1<<16)*(blk_u0[b]+tmp_ix[c]))-cx[1])*source[c][0];
        pointY[c] <== (((1<<16)*(blk_v0[b]+tmp_iy[c]))-cy[1])*source[c][0];
        P[c][0] <== pointX[c]*fy[1];
        P[c][1] <== pointY[c]*fx[1];
        P[c][2] <== source[c][0]*F;
    }
    component pair[64];
    var usable = 0;
    var occ = 0;
    var dok = 0;
    var inf = 0;
    var pok = 0;
    for (var p = 0; p < 64; p++) {
        var c = p\2;
        var ji = p%2;
        var j = 2*ji;
        pair[p] = GeoTemporalPair();
        pair[p].P <== P[c];
        pair[p].scaledF <== (1<<30)*F;
        pair[p].relativeR <== relativeR[ji];
        pair[p].relativeT <== relativeT[ji];
        pair[p].intrinsic <== [fx[j],fy[j],cx[j],cy[j]];
        pair[p].sourceValid <== source[c][1];
        pair[p].sourceY <== source[c][2];
        pair[p].informative <== informative[c];
        pair[p].y <== pr_y[p];
        pair[p].d <== pr_d[p];
        pair[p].c <== pr_c[p];
        pair[p].bx <== pr_bx[p];
        pair[p].by <== pr_by[p];
        usable += pair[p].usable;
        occ += pair[p].occluded;
        dok += pair[p].depthOk;
        inf += pair[p].nonOccInformative;
        pok += pair[p].photoOk;
    }
    N_usable <== usable;
    N_occ <== occ;
    N_dok <== dok;
    N_inf <== inf;
    N_pok <== pok;
    signal pass[5];
    pass[0] <== GeoGE(GeoBitsFor(GeoMax(64,POL_t_usable_min())))(N_usable,POL_t_usable_min());
    pass[1] <== GeoGE(GeoBitsFor(64*GeoMax(100,POL_occ_max_pct())))(POL_occ_max_pct()*N_usable,100*N_occ);
    pass[2] <== GeoGE(GeoBitsFor(64*GeoMax(100,POL_dok_min_pct())))(100*N_dok,POL_dok_min_pct()*(N_usable-N_occ));
    pass[3] <== GeoGE(GeoBitsFor(GeoMax(64,POL_inf_min())))(N_inf,POL_inf_min());
    pass[4] <== GeoGE(GeoBitsFor(64*GeoMax(100,POL_pok_min_pct())))(100*N_pok,POL_pok_min_pct()*N_inf);
    ok <== GeoAll(5)(pass);
}
