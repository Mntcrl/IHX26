pragma circom 2.2.3;

include "circomlib/circuits/bitify.circom";


template GalaxyCamera() {
    signal input e[54];
    signal output fx[3]; signal output fy[3]; signal output cx[3]; signal output cy[3];
    signal output R[3][9]; signal output T[3][3];
    signal output ts[3]; signal output track[3];
    component intrinsics[3][4];
    component pose[3][12];
    component time_range[3];
    component track_range[3];
    for (var f = 0; f < 3; f++) {
        for (var i = 0; i < 4; i++) {
            intrinsics[f][i] = Num2Bits(26);
            intrinsics[f][i].in <== e[18*f+i];
            _ <== intrinsics[f][i].out;
        }
        fx[f] <== e[18*f];
        fy[f] <== e[18*f+1];
        cx[f] <== e[18*f+2];
        cy[f] <== e[18*f+3];
        for (var i = 0; i < 12; i++) {
            pose[f][i] = Num2Bits(32);
            pose[f][i].in <== e[18*f+4+i] + 2**31;
            _ <== pose[f][i].out;
            if (i < 9) { R[f][i] <== e[18*f+4+i]; }
            else { T[f][i-9] <== e[18*f+4+i]; }
        }
        ts[f] <== e[18*f+16];
        track[f] <== e[18*f+17];
        time_range[f] = Num2Bits(48);
        track_range[f] = Num2Bits(2);
        time_range[f].in <== ts[f];
        track_range[f].in <== track[f];
        _ <== time_range[f].out;
        _ <== track_range[f].out;
    }
}

template GalaxyImuSample() {
    signal input e;
    signal output dt;
    signal output w[3];
    component range = Num2Bits(128);
    range.in <== e;
    var dt_value = 0;
    for (var b = 0; b < 32; b++) {
        dt_value += (2**b)*range.out[96+b];
    }
    dt <== dt_value;
    for (var a = 0; a < 3; a++) {
        var bias = 0;
        for (var b = 0; b < 32; b++) {
            bias += (2**b)*range.out[32*(2-a)+b];
        }
        w[a] <== bias - 2**31;
    }
}

template GalaxyImu() {
    signal input e[133];
    signal output g0[3];
    signal output a_peak;
    signal output imu_dt[2][64];
    signal output imu_w[2][64][3];
    component gravity[3];
    for (var a = 0; a < 3; a++) {
        g0[a] <== e[a];
        gravity[a] = Num2Bits(32);
        gravity[a].in <== e[a] + 2**31;
        _ <== gravity[a].out;
    }
    component peak = Num2Bits(32);
    a_peak <== e[3];
    peak.in <== a_peak;
    _ <== peak.out;
    e[4] === 0;
    component sample[2][64];
    for (var t = 0; t < 2; t++) {
        for (var i = 0; i < 64; i++) {
            sample[t][i] = GalaxyImuSample();
            sample[t][i].e <== e[5 + 64*t + i];
            imu_dt[t][i] <== sample[t][i].dt;
            imu_w[t][i] <== sample[t][i].w;
        }
    }
}
