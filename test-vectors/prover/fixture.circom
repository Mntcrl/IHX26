pragma circom 2.2.0;

// galaxy-prover test fixture (cargo test -p galaxy-prover). Same public
// interface shape as galaxy-pc-v1m (22 public inputs, no outputs), tiny body:
// Poseidon, a 64-bit range check and products binding every public input.
// Rebuild with scripts/prover-build-fixture.sh.

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

template Fixture() {
    signal input pub_in[22];
    signal input secret[2];

    component rc = Num2Bits(64);
    rc.in <== secret[0];

    component h = Poseidon(2);
    h.inputs[0] <== secret[0];
    h.inputs[1] <== secret[1];

    signal prod[22];
    signal acc[23];
    acc[0] <== h.out;
    for (var i = 0; i < 22; i++) {
        prod[i] <== pub_in[i] * secret[1];
        acc[i + 1] <== acc[i] * prod[i];
    }
}

component main {public [pub_in]} = Fixture();
