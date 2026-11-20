pragma circom 2.1.0;

// x * y + z == out, with three public inputs (y, z, out) and private x.
template Tiny() {
    signal input x;
    signal input y;
    signal input z;
    signal input out;
    signal xy;
    xy <== x * y;
    out === xy + z;
}

component main {public [y, z, out]} = Tiny();
