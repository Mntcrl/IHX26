pragma circom 2.2.2;

include "gadgets.circom";

template GeoRelativePose() {
    signal input R[3][9];
    signal input T[3][3];
    signal output relativeR[2][9];
    signal output relativeT[2][3];
    signal output vee[2][3];
    signal rTerm[2][9][3];
    signal tTerm[2][3][3];
    for (var ji = 0; ji < 2; ji++) {
        var j = 2*ji;
        for (var a = 0; a < 3; a++) {
            for (var b = 0; b < 3; b++) {
                var ab = 3*a+b;
                for (var k = 0; k < 3; k++) rTerm[ji][ab][k] <== R[j][3*k+a]*R[1][3*k+b];
                relativeR[ji][ab] <== GeoShiftFloor(64,30)(rTerm[ji][ab][0]+rTerm[ji][ab][1]+rTerm[ji][ab][2]);
            }
            for (var k = 0; k < 3; k++) tTerm[ji][a][k] <== R[j][3*k+a]*(T[1][k]-T[j][k]);
            relativeT[ji][a] <== GeoShiftFloor(65,30)(tTerm[ji][a][0]+tTerm[ji][a][1]+tTerm[ji][a][2]);
        }
        vee[ji] <== [relativeR[ji][7]-relativeR[ji][5], relativeR[ji][2]-relativeR[ji][6], relativeR[ji][3]-relativeR[ji][1]];
    }
}
