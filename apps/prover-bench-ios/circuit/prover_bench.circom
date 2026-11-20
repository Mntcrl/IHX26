pragma circom 2.2.0;

// XRPL Galaxy prover benchmark circuit (MobileProver, ADR 0080).
//
// Synthetic, Poseidon-heavy workload shaped like galaxy-pc-v1m (spec §9):
// 22 public inputs (same count/order slots as the real circuit, values are
// arbitrary field elements), no public outputs, a long chain of circomlib
// Poseidon(5) permutations and Num2Bits range checks on every private input.
// It is a performance benchmark only: it proves nothing about photos.

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

template ProverBench(STEPS) {
    signal input pub_in[22];
    signal input priv_in[STEPS][4];

    // Absorb the 22 public inputs: Poseidon5(pub[0..5]) then 5 x Poseidon5(h, 4 more)
    // (the last call is zero padded), so every public input is constrained.
    component absorb[6];
    absorb[0] = Poseidon(5);
    for (var k = 0; k < 5; k++) {
        absorb[0].inputs[k] <== pub_in[k];
    }
    for (var c = 1; c < 6; c++) {
        absorb[c] = Poseidon(5);
        absorb[c].inputs[0] <== absorb[c - 1].out;
        for (var k = 0; k < 4; k++) {
            var idx = 5 + (c - 1) * 4 + k;
            if (idx < 22) {
                absorb[c].inputs[k + 1] <== pub_in[idx];
            } else {
                absorb[c].inputs[k + 1] <== 0;
            }
        }
    }

    // Main chain: per step, range-check four 64-bit private inputs and hash them
    // together with the running state.
    component range[STEPS][4];
    component step[STEPS];
    for (var s = 0; s < STEPS; s++) {
        step[s] = Poseidon(5);
        if (s == 0) {
            step[s].inputs[0] <== absorb[5].out;
        } else {
            step[s].inputs[0] <== step[s - 1].out;
        }
        for (var k = 0; k < 4; k++) {
            range[s][k] = Num2Bits(64);
            range[s][k].in <== priv_in[s][k];
            step[s].inputs[k + 1] <== priv_in[s][k];
        }
    }

    // Bind the final state with a non-linear constraint so it cannot be pruned.
    signal sq;
    sq <== step[STEPS - 1].out * step[STEPS - 1].out;
}
