pragma circom 2.2.3;

function galaxyTag(code, aux) {
    return 0x47414C58 * (2**96) + 2**80 + code * (2**64) + aux;
}
