import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { wasm } from 'circom_tester';
import { buildPoseidon } from 'circomlibjs';

const here = dirname(fileURLToPath(import.meta.url));
const circuits = join(here, '../..');
const P = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const poseidon = await buildPoseidon();
const hash = (...args) => poseidon.F.toObject(poseidon(args));
const tag = (kind, aux) => (0x47414c58n << 96n) + (1n << 80n) + (BigInt(kind) << 64n) + BigInt(aux);
const list = (n, make) => Array.from({ length: n }, (_, i) => make(i));
const bits = (n, value) => list(n, i => value >> BigInt(i) & 1n);
const morton = (x, y) => list(6, i => ((x >> i & 1) + 2*(y >> i & 1)) * 4**i).reduce((a, b) => a + b);
const include = join(circuits, 'node_modules');

// The witness calculator stops at the first failing `===`, so it cannot
// produce the full trace of a modified input. The trace calculator is built
// from a copy of the same sources with every `===` statement removed; the
// witness wire layout (.sym) must be identical to the audited circuit, so the
// values it computes can be copied wire-by-wire into the audited witness.
// Constraint checks always use the audited circuit's own R1CS.
async function copyWithoutEqualities(src, dst) {
    await mkdir(dst, { recursive: true });
    for (const entry of await readdir(src, { withFileTypes: true })) {
        const from = join(src, entry.name), to = join(dst, entry.name);
        if (entry.isDirectory()) await copyWithoutEqualities(from, to);
        else if (entry.name.endsWith('.circom')) {
            const text = (await readFile(from, 'utf8'))
                .replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
                .replace(/[^;{}\n]*===[^;]*;/g, '');
            await writeFile(to, text);
        }
    }
}

const traceRoot = await mkdtemp(join(tmpdir(), 'galaxy-core-trace-'));
for (const dir of ['components', 'physical-consistency', 'tests/core', 'node_modules/circomlib/circuits']) {
    await copyWithoutEqualities(join(circuits, dir), join(traceRoot, dir));
}
process.on('exit', () => execFileSync('rm', ['-rf', traceRoot]));

const compile = async (templateName, ...templateParams) => {
    const root = await mkdtemp(join(tmpdir(), 'galaxy-core-'));
    const source = harness => `pragma circom 2.2.3;\ninclude "${harness}";\n`
        + `component main = ${templateName}(${templateParams.join(',')});\n`;
    try {
        const main = join(root, 'audit.circom');
        await writeFile(main, source(join(here, 'harness.circom')));
        const c = await wasm(main, { O: 0, include });
        await c.loadSymbols();
        await c.loadConstraints();
        const traceMain = join(root, 'trace', 'audit.circom');
        await mkdir(dirname(traceMain));
        await writeFile(traceMain, source(join(traceRoot, 'tests/core/harness.circom')));
        execFileSync('circom', [traceMain, '-l', join(traceRoot, 'node_modules'), '--O0', '--sym', '--wasm',
            '-o', dirname(traceMain)]);
        assert((await readFile(join(dirname(traceMain), 'audit.sym'))).equals(await readFile(join(c.dir, 'audit.sym'))),
            'trace calculator must have the exact witness wire layout of the audited circuit');
        const trace = await wasm(traceMain, { O: 0, include, output: dirname(traceMain), recompile: false });
        c.recalculate = input => trace.calculateWitness(input, false);
        return c;
    } finally {
        await rm(root, { recursive: true, force: true });
    }
};

function signal(c, name) {
    const i = c.symbols[`main.${name}`]?.varIdx;
    assert(i > 0, `missing witness wire ${name}`);
    return i;
}

// Start from the valid witness and update all affected wires using a trace
// calculator for the same R1CS. Count failed equations to ensure the
// rejection actually comes from the intended check, not stale hash values.
async function reject(c, honest, input, expectedFailures) {
    await assert.rejects(c.calculateWitness(input, true), /Assert Failed/i);
    const trace = await c.recalculate(input);
    const modified = honest.slice();
    trace.forEach((v, i) => { if (v !== honest[i]) modified[i] = v; });
    await checkRejected(c, honest, modified, expectedFailures);
}

async function checkRejected(c, honest, modified, expectedFailures) {
    assert(modified.some((v, i) => v !== honest[i]), 'test did not modify any witness wires');
    await assert.rejects(c.checkConstraints(modified), /Constraint doesn't match/);
    const { F } = c;
    const evaluate = terms => Object.entries(terms).reduce(
        (acc, [idx, coef]) => F.add(acc, F.mul(coef, modified[Number(idx)])), F.zero,
    );
    let failures = 0;
    for (const [a, b, z] of c.constraints) {
        if (!F.eq(F.mul(evaluate(a), evaluate(b)), evaluate(z))) failures++;
    }
    assert.equal(failures, expectedFailures, 'modified trace must leave only the intended R1CS rows unsatisfied');
}

const node = (kind, frame, level, index, children) => hash(
    tag(4, (kind << 24) + (frame << 20) + (level << 16) + index), ...children,
);
const without = (row, i) => row.filter((_, j) => i !== j);
function pathRoot(kind, frame, pos, siblings, leaf, start = 0) {
    for (const [i, s] of siblings.entries()) {
        const children = s.slice();
        children.splice(pos % 4, 0, leaf);
        pos >>= 2;
        leaf = node(kind, frame, start+i+1, pos, children);
    }
    return leaf;
}
const rgbBytes = (i, image) => list(48, k => BigInt((i*37 + k*17 + image*41) % 256));
const depthPixels = i => list(16, k => [500n + BigInt(i*13 + k), BigInt((i+k) % 3)]);
const pack = (width, chunk, data) => [0, 1].map(i => data.slice(i*chunk, (i+1)*chunk)
    .reduce((acc, v) => acc * 2n**BigInt(width) + v, 0n));
const rgbPack = (i, image) => pack(8, 24, rgbBytes(i, image));
const depthPack = i => pack(18, 8, depthPixels(i).map(([d, c]) => 4n*d + c));

function tileInput(kind, image) {
    const tile = 19;
    const tilePos = morton(tile % 8, tile >> 3);
    const block = [[3, 2], [4, 3], [0, 6], [7, 7]];
    const pos = block.map(([x, y]) => morton(x, y));
    const packed = list(64, i => kind === 1 ? rgbPack(i, image) : depthPack(i));
    const leaves = packed.map((l, i) => hash(tag(kind, 4096 + 64*tilePos + i), ...l));
    const levels = [leaves];
    for (let l = 1; l <= 3; l++) {
        const prev = levels[l-1];
        levels.push(list(prev.length / 4, i => node(kind, 1, l,
            tilePos * 4**(3-l) + i, prev.slice(4*i, 4*i+4))));
    }
    const siblings = pos.map(p => list(3, l => {
        const child = p >> (2*l), start = child & ~3;
        return without(levels[l].slice(start, start + 4), child % 4);
    }));
    const upper = list(3, l => list(3, j => BigInt(123 + 57*l + 17*j)));
    const root = pathRoot(kind, 1, tilePos, upper, levels[3][0], 3);
    const pixels = pos.map(i => kind === 1 ? rgbBytes(i, image) : depthPixels(i).flat());
    return { input: { tile, block, packed: pos.map(i => packed[i]), siblings, upper, root }, pixels };
}

test('RGB path binds all four blocks to the committed image and constrains unpacked bytes', async () => {
    const c = await compile('GalaxyTileAudit', 1);
    const { input, pixels } = tileInput(1, 0);
    const honest = await c.calculateWitness(input, true);
    await c.checkConstraints(honest);
    await c.assertOut(honest, { pixels });

    // All paths/packing come from a second image whose claimed root is replaced.
    const other = tileInput(1, 1).input;
    await c.checkConstraints(await c.calculateWitness(other, true));
    assert.notEqual(other.root, input.root);
    await reject(c, honest, { ...other, root: input.root }, 1);
});

test('depth path rejects incorrect sibling and incorrect depth with recomputed hashes', async () => {
    const c = await compile('GalaxyTileAudit', 2);
    const { input, pixels } = tileInput(2, 0);
    const honest = await c.calculateWitness(input, true);
    await c.checkConstraints(honest);
    await c.assertOut(honest, { pixels });
    const wrongSibling = structuredClone(input);
    wrongSibling.siblings[3][0][1] += 1n;
    await reject(c, honest, wrongSibling, 1); // Shared level-3 tile equality only.
    const wrongDepth = structuredClone(input);
    wrongDepth.packed[1][0] += 4n;
    await reject(c, honest, wrongDepth, 1);
});

function temporalInput(frame, bx, by, overflow = false) {
    const pos = morton(bx, by);
    const rgb = rgbPack(pos, frame);
    if (overflow) rgb[0] += 1n << 192n;
    const depth = depthPack(pos);
    const siblings = list(6, l => list(3, j => BigInt(304 + 79*l + 31*j)));
    const leaf = hash(tag(3, frame*4096 + pos), ...rgb, ...depth);
    return { bx, by, rgb, depth, siblings, root: pathRoot(3, frame, pos, siblings, leaf) };
}

test('FRAME paths bind position using both frame tags, reject padding and high-byte overflow', async () => {
    for (const frame of [0, 2]) {
        const c = await compile('GalaxyFrameAudit', frame);
        // (0,0) is the required real opening even for an out-of-bounds projection.
        const bx = frame === 0 ? 0 : 31, by = frame === 0 ? 0 : 47;
        const input = temporalInput(frame, bx, by);
        const honest = await c.calculateWitness(input, true);
        await c.checkConstraints(honest);
        const pos = morton(bx, by);
        const pixels = depthPixels(pos);
        await c.assertOut(honest, { bytes: rgbBytes(pos, frame), d: pixels.map(x => x[0]), c: pixels.map(x => x[1]) });
        // Check that padding is rejected even with hash-consistent siblings and root.
        await reject(c, honest, temporalInput(frame, bx, 48), 1);
        await reject(c, honest, temporalInput(frame, bx, by, true), 1);
        await reject(c, honest, { ...input, root: temporalInput(2-frame, bx, by).root }, 1);
    }
});

function evidenceRoot({ rgb_root, roots, salts, camera, imu, bluetooth, noise, meta }) {
    const components = [rgb_root, ...roots.map((r, i) => hash(tag(6, i+1), salts[i], r))];
    for (const [i, elems] of [camera, imu, bluetooth, noise, meta].entries()) {
        let h = hash(tag(7, i+4), salts[i+3], BigInt(elems.length));
        for (let j = 0; j < elems.length; j += 4) {
            h = hash(h, ...list(4, k => elems[j+k] ?? 0n));
        }
        components.push(h);
    }
    for (let i = 9; i < 16; i++) components.push(hash(tag(9, i), 0n));
    const below = list(4, i => hash(tag(8, 65536+i), ...components.slice(4*i, 4*i+4)));
    return hash(tag(8, 2*65536), ...below);
}

test('CAMERA substitution, signed range and IMU overflow fail constraint checks', async () => {
    const c = await compile('GalaxyCommitmentAudit');
    const camera = list(54, i => BigInt(i+1));
    camera[4] = P - (1n << 31n); // i32::MIN is accepted.
    camera[5] = (1n << 31n) - 1n;
    camera[17] = camera[35] = camera[53] = 2n;
    const imu = [0n, P-(1n << 31n), 1n, 19n, 0n, ...list(128, i => BigInt(i) << 96n)];
    const input = {
        rgb_root: 9n, roots: [10n, 11n, 12n], salts: list(8, i => BigInt(13+i)),
        camera, imu, bluetooth: list(24, i => BigInt(i)), noise: list(18, () => 0n), meta: list(9, () => 0n),
    };
    input.evidence_root = evidenceRoot(input);
    const honest = await c.calculateWitness(input, true);
    await c.checkConstraints(honest);
    const substituted = structuredClone(input);
    substituted.camera[0] += 1n;
    await reject(c, honest, substituted, 1); // Fixed public evidence root only.
    const wideCam = structuredClone(input);
    wideCam.camera[5]++;
    wideCam.evidence_root = evidenceRoot(wideCam);
    await reject(c, honest, wideCam, 1);
    const wideImu = structuredClone(input);
    wideImu.imu[5] += 1n << 128n;
    wideImu.evidence_root = evidenceRoot(wideImu);
    await reject(c, honest, wideImu, 1);
});

const tiles = [10, 11, 12, 13, 18, 19, 20, 21, 0, 1, 2, 3, 4, 5, 6, 7];
const samplingHash = (limbs, slot, tile) => hash(tag(11, slot), hash(tag(10, 0), ...limbs), BigInt(tile));
function expectedSampling(limbs, sampledTiles, overrideFirst) {
    const ctx = hash(tag(10, 0), ...limbs);
    const out = { tile: [], blk: [], blk_u0: [], blk_v0: [], tmp_ix: [], tmp_iy: [], hyp_idx: [] };
    for (const [s, t] of sampledTiles.entries()) {
        const h = s === 0 && overrideFirst !== undefined ? overrideFirst : samplingHash(limbs, s, t);
        out.tile.push(bits(6, BigInt(morton(t % 8, t >> 3))));
        for (let q = 0; q < 4; q++) {
            const bx = q % 2 * 4 + Number(h >> BigInt(4*q) & 3n);
            const by = (q >> 1) * 4 + Number(h >> BigInt(4*q+2) & 3n);
            out.blk.push(bits(6, BigInt(morton(bx, by))));
            out.blk_u0.push(32 * (t % 8) + 4*bx);
            out.blk_v0.push(32 * (t >> 3) + 4*by);
            if (q === 0 || q === 3) {
                out.tmp_ix.push(1n + (h >> BigInt(16+2*q) & 1n));
                out.tmp_iy.push(1n + (h >> BigInt(17+2*q) & 1n));
            }
        }
    }
    out.hyp_idx = list(8, j => {
        const h = hash(tag(12, j), ctx);
        const n = j < 4 ? 8 : 7;
        return list(3, i => h >> BigInt(n*i) & ((1n << BigInt(n))-1n));
    });
    return out;
}

test('sampling tile strata, ordering and seed limb range reject invalid recomputed traces', async () => {
    const c = await compile('GalaxySampling');
    const input = { limbs: [1n, 2n, 3n, 4n], sampled_tiles: tiles };
    const honest = await c.calculateWitness(input, true);
    await c.checkConstraints(honest);
    await c.assertOut(honest, expectedSampling(input.limbs, tiles));
    await reject(c, honest, { ...input, sampled_tiles: [...tiles.slice(0, 15), 48] }, 1);
    await reject(c, honest, { ...input, sampled_tiles: [9, ...tiles.slice(1)] }, 1);
    await reject(c, honest, { ...input, sampled_tiles: [...tiles.slice(0, 15), 10] }, 1);
    await reject(c, honest, { ...input, sampled_tiles: [11, 10, ...tiles.slice(2)] }, 1);
    await reject(c, honest, { ...input, limbs: [1n, 2n, (1n << 128n) + 3n, 4n] }, 1);

    // A different canonical ledger seed is a different public statement; it
    // cannot be paired with the original hash/picks. Audit that equality too.
    const changed = await c.calculateWitness({ ...input, limbs: [1n, 2n, 3n, 5n] }, true);
    await c.checkConstraints(changed);
    const modified = honest.slice();
    for (const [name, { varIdx }] of Object.entries(c.symbols)) {
        if (name === 'main.limbs[3]' || name.startsWith('main.limb_range[3].')) modified[varIdx] = changed[varIdx];
    }
    await checkRejected(c, honest, modified, 1); // Only Poseidon's context input binding.
});

test('alias h + r satisfies ordinary bit reconstruction but fails production AliasCheck', async () => {
    const c = await compile('GalaxySampling');
    const trace = await compile('AliasTrace');
    const limbs = [1n, 2n, 3n, 0n];
    let h;
    // Find a deterministic field hash for which h+r still fits in 254 bits.
    do { limbs[3]++; h = samplingHash(limbs, 0, tiles[0]); } while (h + P >= 1n << 254n);
    const aliased = h + P;
    const b = bits(254, aliased);
    const honest = await c.calculateWitness({ limbs, sampled_tiles: tiles }, true);
    await c.checkConstraints(honest);
    const canonicalTrace = await trace.calculateWitness({ bits: bits(254, h) }, true);
    await trace.checkConstraints(canonicalTrace);
    await assert.rejects(trace.calculateWitness({ bits: b }, true), /Assert Failed/i);
    const aliasTrace = await trace.recalculate({ bits: b });
    const modified = honest.slice();
    for (const [name, { varIdx }] of Object.entries(trace.symbols)) {
        if (!name.startsWith('main.alias.')) continue;
        const tail = name.slice('main.alias.'.length);
        modified[signal(c, `sample_bits[0].aliasCheck.${tail}`)] = aliasTrace[varIdx];
    }
    for (const [i, bit] of b.entries()) {
        modified[signal(c, `sample_bits[0].n2b.out[${i}]`)] = bit;
        modified[signal(c, `sample_bits[0].out[${i}]`)] = bit;
    }
    const expected = expectedSampling(limbs, tiles, aliased);
    function update(prefix, values) {
        if (Array.isArray(values)) values.forEach((x, i) => update(`${prefix}[${i}]`, x));
        else modified[signal(c, prefix)] = BigInt(values);
    }
    for (const [prefix, values] of Object.entries(expected)) update(prefix, values);
    await checkRejected(c, honest, modified, 1); // Only strict canonical alias check.
});
