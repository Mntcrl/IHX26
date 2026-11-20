pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";
include "tags.circom";

// Committed-only aggregate slots contain opaque canonical field elements;
// there is intentionally no policy interpretation of any of their values.
template GalaxyScalarComponent(slot, count) {
    signal input salt;
    signal input e[count];
    signal output out;
    var chunks = (count + 3) \ 4;
    component header = Poseidon(3);
    header.inputs <== [galaxyTag(7, slot), salt, count];
    component absorb[chunks];
    for (var i = 0; i < chunks; i++) {
        absorb[i] = Poseidon(5);
        if (i == 0) { absorb[i].inputs[0] <== header.out; }
        else { absorb[i].inputs[0] <== absorb[i-1].out; }
        for (var j = 0; j < 4; j++) {
            if (4*i+j < count) { absorb[i].inputs[1+j] <== e[4*i+j]; }
            else { absorb[i].inputs[1+j] <== 0; }
        }
    }
    out <== absorb[chunks-1].out;
}

template GalaxyEvidenceCommitment() {
    signal input rgb_root;
    signal input roots[3];
    signal input salts[8];
    signal input camera[54];
    signal input imu[133];
    signal input bluetooth[24];
    signal input noise[18];
    signal input meta[9];
    signal output root;
    signal slots[16];
    slots[0] <== rgb_root;
    component salt_range[8];
    for (var i = 0; i < 8; i++) {
        salt_range[i] = Num2Bits(248);
        salt_range[i].in <== salts[i];
        _ <== salt_range[i].out;
    }
    component salted_tree[3];
    for (var i = 0; i < 3; i++) {
        salted_tree[i] = Poseidon(3);
        salted_tree[i].inputs <== [galaxyTag(6, i+1), salts[i], roots[i]];
        slots[i+1] <== salted_tree[i].out;
    }
    component ca = GalaxyScalarComponent(4, 54);
    ca.salt <== salts[3]; ca.e <== camera; slots[4] <== ca.out;
    component im = GalaxyScalarComponent(5, 133);
    im.salt <== salts[4]; im.e <== imu; slots[5] <== im.out;
    component bt = GalaxyScalarComponent(6, 24);
    bt.salt <== salts[5]; bt.e <== bluetooth; slots[6] <== bt.out;
    component no = GalaxyScalarComponent(7, 18);
    no.salt <== salts[6]; no.e <== noise; slots[7] <== no.out;
    component me = GalaxyScalarComponent(8, 9);
    me.salt <== salts[7]; me.e <== meta; slots[8] <== me.out;
    component pad[7];
    for (var i = 0; i < 7; i++) {
        pad[i] = Poseidon(2);
        pad[i].inputs <== [galaxyTag(9, i+9), 0];
        slots[i+9] <== pad[i].out;
    }
    component sub[4];
    component top = Poseidon(5);
    top.inputs[0] <== galaxyTag(8, 2*(2**16));
    for (var i = 0; i < 4; i++) {
        sub[i] = Poseidon(5);
        sub[i].inputs[0] <== galaxyTag(8, 2**16+i);
        for (var j = 0; j < 4; j++) {
            sub[i].inputs[j+1] <== slots[4*i+j];
        }
        top.inputs[i+1] <== sub[i].out;
    }
    root <== top.out;
}
