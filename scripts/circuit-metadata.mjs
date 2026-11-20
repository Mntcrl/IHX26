#!/usr/bin/env node
// Reads the header section of an iden3 .r1cs file without loading the
// constraint section (the production R1CS is several hundred MB).
// Usage: node scripts/circuit-metadata.mjs r1cs <file.r1cs> [--summary]
import { openSync, readSync, closeSync, fstatSync } from 'node:fs';

function fail(message) {
    console.error(`circuit-metadata: ${message}`);
    process.exit(1);
}

function readAt(fd, size, offset, length) {
    if (offset + length > size) fail('truncated r1cs file');
    const buf = Buffer.alloc(length);
    if (readSync(fd, buf, 0, length, offset) !== length) fail('short read');
    return buf;
}

function r1csHeader(path) {
    const fd = openSync(path, 'r');
    try {
        const { size } = fstatSync(fd);
        const head = readAt(fd, size, 0, 12);
        if (head.toString('latin1', 0, 4) !== 'r1cs') fail('not an r1cs file');
        if (head.readUInt32LE(4) !== 1) fail('unsupported r1cs version');
        const sections = head.readUInt32LE(8);
        let offset = 12;
        for (let i = 0; i < sections; i++) {
            const sec = readAt(fd, size, offset, 12);
            const type = sec.readUInt32LE(0);
            const length = Number(sec.readBigUInt64LE(4));
            offset += 12;
            if (type === 1) {
                const fieldSize = readAt(fd, size, offset, 4).readUInt32LE(0);
                if (fieldSize !== 32) fail('unexpected field size');
                const prime = readAt(fd, size, offset + 4, 32);
                const bn254 = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
                if (BigInt('0x' + Buffer.from(prime).reverse().toString('hex')) !== bn254) fail('r1cs is not over BN254');
                const h = readAt(fd, size, offset + 36, 28);
                return {
                    curve: 'bn254',
                    n_wires: h.readUInt32LE(0),
                    n_public_outputs: h.readUInt32LE(4),
                    n_public_inputs: h.readUInt32LE(8),
                    n_private_inputs: h.readUInt32LE(12),
                    n_labels: Number(h.readBigUInt64LE(16)),
                    n_constraints: h.readUInt32LE(24),
                };
            }
            offset += length;
        }
        fail('r1cs header section not found');
    } finally {
        closeSync(fd);
    }
}

const [kind, file, flag] = process.argv.slice(2);
if (kind !== 'r1cs' || !file || (flag !== undefined && flag !== '--summary')) {
    fail('usage: circuit-metadata.mjs r1cs <file.r1cs> [--summary]');
}
const info = r1csHeader(file);
if (flag === '--summary') {
    console.log(`constraints=${info.n_constraints} wires=${info.n_wires} public=${info.n_public_inputs + info.n_public_outputs} private=${info.n_private_inputs}`);
} else {
    console.log(JSON.stringify(info, null, 2));
}
