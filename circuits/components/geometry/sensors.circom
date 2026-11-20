pragma circom 2.2.2;

include "gadgets.circom";
include "../../physical-consistency/generated/policy_v1m.circom";

template GeoClosedRange(bits) {
    signal input in;
    signal input lo;
    signal input hi;
    signal output out;
    signal under <== GeoLT(bits)(in,lo);
    signal over <== GeoLT(bits)(hi,in);
    out <== (1-under)*(1-over);
}

template GeoFrameQuality() {
    signal input track;
    signal input intrinsic[4];
    signal input R[9];
    signal output ok;
    var minI[4] = [POL_fx_min_q16(), POL_fx_min_q16(), POL_cx_min_q16(), POL_cy_min_q16()];
    var maxI[4] = [POL_fx_max_q16(), POL_fx_max_q16(), POL_cx_max_q16(), POL_cy_max_q16()];
    signal pass[12];
    pass[0] <== IsEqual()([track,2]);
    for (var i = 0; i < 4; i++) {
        pass[1+i] <== GeoClosedRange(GeoBitsFor(GeoMax(1<<26, GeoMax(minI[i],maxI[i]))))(intrinsic[i],minI[i],maxI[i]);
    }
    signal dotTerm[6][3];
    var pair = 0;
    for (var a = 0; a < 3; a++) {
        for (var b = a; b < 3; b++) {
            var identity = a == b ? (1<<60) : 0;
            for (var k = 0; k < 3; k++) dotTerm[pair][k] <== R[3*a+k]*R[3*b+k];
            pass[5+pair] <== GeoAbsLe(GeoBitsFor((1<<64)+POL_ortho_tol_q60()))(
                dotTerm[pair][0]+dotTerm[pair][1]+dotTerm[pair][2]-identity, POL_ortho_tol_q60());
            pair++;
        }
    }
    signal crossFirst[3];
    signal cross[3];
    signal detTerm[3];
    for (var k = 0; k < 3; k++) {
        var a = (k+1)%3;
        var b = (k+2)%3;
        crossFirst[k] <== R[a]*R[3+b];
        cross[k] <== crossFirst[k]-R[b]*R[3+a];
        detTerm[k] <== cross[k]*R[6+k];
    }
    pass[11] <== GeoLT(96)(0,detTerm[0]+detTerm[1]+detTerm[2]);
    ok <== GeoAll(12)(pass);
}

template GeoQuality() {
    signal input fx[3];
    signal input fy[3];
    signal input cx[3];
    signal input cy[3];
    signal input track[3];
    signal input R[3][9];
    signal input ts[3];
    signal output ok;
    signal frameOk[3];
    signal dtOk[2];
    for (var f = 0; f < 3; f++) frameOk[f] <== GeoFrameQuality()(track[f],[fx[f],fy[f],cx[f],cy[f]],R[f]);
    for (var w = 0; w < 2; w++) {
        dtOk[w] <== GeoClosedRange(GeoBitsFor((1<<48)+GeoMax(POL_dt_min_us(),POL_dt_max_us())))(
            ts[w+1]-ts[w], POL_dt_min_us(), POL_dt_max_us());
    }
    ok <== GeoAll(5)([frameOk[0],frameOk[1],frameOk[2],dtOk[0],dtOk[1]]);
}

template GeoImuWindow() {
    signal input dt[64];
    signal input omega[64][3];
    signal input windowDuration;
    signal output theta[3];
    signal output ok;
    signal isPad[64];
    signal within[64];
    signal omegaZero[64][3];
    signal stationary[64];
    signal integral[64][3];
    signal padOk[64];
    signal sampleOk[64];
    signal padTail[63];
    var sampleCount = 0;
    for (var k = 0; k < 64; k++) {
        isPad[k] <== IsZero()(dt[k]);
        sampleCount += 1 - isPad[k];
        within[k] <== GeoGE(GeoBitsFor(GeoMax(1<<32,POL_imu_dt_max_us())))(POL_imu_dt_max_us(),dt[k]);
        for (var a = 0; a < 3; a++) {
            omegaZero[k][a] <== IsZero()(omega[k][a]);
            integral[k][a] <== omega[k][a]*dt[k];
        }
        stationary[k] <== GeoAll(3)(omegaZero[k]);
        padOk[k] <== 1 - isPad[k]*(1-stationary[k]);
        sampleOk[k] <== within[k]*padOk[k];
    }
    for (var k = 0; k < 63; k++) padTail[k] <== 1-isPad[k]*(1-isPad[k+1]);
    for (var a = 0; a < 3; a++) {
        var sum = 0;
        for (var k = 0; k < 64; k++) sum += integral[k][a];
        theta[a] <== sum;
    }
    signal totalDt <== GeoCount(64)(dt);
    signal sufficient <== GeoGE(GeoBitsFor(GeoMax(64,POL_imu_min_samples())))(sampleCount,POL_imu_min_samples());
    signal coverage <== GeoAbsLe(GeoBitsFor((1<<49)+POL_imu_cover_tol_us()))(totalDt-windowDuration,POL_imu_cover_tol_us());
    signal validSamples <== GeoAll(64)(sampleOk);
    signal validTail <== GeoAll(63)(padTail);
    ok <== GeoAll(4)([sufficient,coverage,validSamples,validTail]);
}

template GeoMotion() {
    signal input relativeT[2][3];
    signal input vee[2][3];
    signal input R1[9];
    signal input ts[3];
    signal input dt[2][64];
    signal input omega[2][64][3];
    signal input g0[3];
    signal input a_peak;
    signal output ok;
    signal baselineTerm[2][3];
    signal rotOk[2][3];
    signal baselineOk[2];
    signal imuOk[2];
    signal theta[2][3];
    signal thetaCam[2][3];
    signal gyroOk[2][3];
    component imu[2];
    var minBase2 = POL_baseline_min_um()*POL_baseline_min_um();
    var gyroTol = 2*POL_gyro_tol_urad()*(1<<30);
    for (var w = 0; w < 2; w++) {
        var gyroSign = w == 0 ? -1 : 1;
        imu[w] = GeoImuWindow();
        imu[w].dt <== dt[w];
        imu[w].omega <== omega[w];
        imu[w].windowDuration <== ts[w+1]-ts[w];
        imuOk[w] <== imu[w].ok;
        theta[w] <== imu[w].theta;
        for (var a = 0; a < 3; a++) {
            baselineTerm[w][a] <== relativeT[w][a]*relativeT[w][a];
            rotOk[w][a] <== GeoAbsLe(GeoBitsFor((1<<35)+POL_rot_max_q30()))(vee[w][a],POL_rot_max_q30());
            thetaCam[w][a] <== POL_m_cd(a,0)*theta[w][0]+POL_m_cd(a,1)*theta[w][1]+POL_m_cd(a,2)*theta[w][2];
            gyroOk[w][a] <== GeoAbsLe(GeoBitsFor((1<<81)+gyroTol))(1000000*vee[w][a]+gyroSign*(1<<11)*thetaCam[w][a],gyroTol);
        }
        baselineOk[w] <== GeoGE(GeoBitsFor(GeoMax(1<<72,minBase2)))(
            baselineTerm[w][0]+baselineTerm[w][1]+baselineTerm[w][2], minBase2);
    }
    signal gc[3];
    signal normTerm[3];
    signal gravTerm[3];
    for (var a = 0; a < 3; a++) {
        gc[a] <== POL_m_cd(a,0)*g0[0]+POL_m_cd(a,1)*g0[1]+POL_m_cd(a,2)*g0[2];
        normTerm[a] <== gc[a]*gc[a];
        gravTerm[a] <== -R1[3+a]*gc[a];
    }
    var gravCos = POL_grav_cos_milli()*(1<<50);
    signal normOk <== GeoAbsLe(GeoBitsFor((1<<64)+POL_grav_norm_tol_q40()))(
        normTerm[0]+normTerm[1]+normTerm[2]-(1<<40),POL_grav_norm_tol_q40());
    signal gravOk <== GeoGE(GeoBitsFor((1<<74)+gravCos))(1000*(gravTerm[0]+gravTerm[1]+gravTerm[2]),gravCos);
    signal accelOk <== GeoGE(GeoBitsFor(GeoMax(1<<32,POL_acc_max_mg())))(POL_acc_max_mg(),a_peak);
    ok <== GeoAll(19)([baselineOk[0],baselineOk[1],
        rotOk[0][0],rotOk[0][1],rotOk[0][2],rotOk[1][0],rotOk[1][1],rotOk[1][2],
        imuOk[0],imuOk[1],gyroOk[0][0],gyroOk[0][1],gyroOk[0][2],gyroOk[1][0],gyroOk[1][1],gyroOk[1][2],
        normOk,gravOk,accelOk]);
}
