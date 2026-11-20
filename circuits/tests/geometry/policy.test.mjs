import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, createReadStream, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { createInterface } from 'node:readline';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { renderPolicy } from '../../../scripts/circuit-gen-policy.mjs';

const require = createRequire(import.meta.url);
const { BaseTester } = require('circom_tester/common/tester.js');
const root = fileURLToPath(new URL('../../../', import.meta.url));
const harness = join(root, 'circuits/tests/geometry');
const fixtures = join(root, 'test-vectors/policy');
const prime = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;

const inputNames = (
    'blk_r blk_g blk_b blk_d blk_c blk_u0 blk_v0 tmp_ix tmp_iy hyp_idx ' +
    'pr_r pr_g pr_b pr_d pr_c pr_bx pr_by fx fy cx cy R T ts track imu_dt imu_w g0 a_peak'
).split(' ');
const statNames = (
    'V_all V_c n_hyp_ok_g n_hyp_ok_c max_all max_c j_star tiles_cov tiles_nonplanar tiles_nonplanar_c ' +
    'n_strong tiles_strong N_disc N_match N_usable N_occ N_dok N_inf N_pok'
).split(' ');
const flagNames = 'quality_ok geometry_ok rgb_depth_ok temporal_ok motion_ok'.split(' ');
const outputNames = [...statNames.map(s => `st_${s}`), ...flagNames];

const field = x => ((BigInt(x) % prime + prime) % prime);
function encode(value) {
    if (Array.isArray(value)) return value.map(encode);
    assert(Number.isSafeInteger(value), 'non-integral or unsafe circuit input');
    return field(value).toString();
}
function scratch(t) {
    const dir = mkdtempSync(join(tmpdir(), 'galaxy-circuit-geometry-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    return dir;
}

async function compile(dir, source, wanted, groupConstraints = false) {
    const circom = process.env.CIRCOM ?? join(root, '.tools/bin/circom');
    execFileSync(circom, [source, '--wasm', '--r1cs', '--sym', '--O1', '-o', dir], { cwd: root, stdio: 'inherit' });
    const name = basename(source, '.circom');
    const tester = new BaseTester(dir, name);
    const builder = require(join(dir, `${name}_js`, 'witness_calculator.js'));
    const calculator = await builder(readFileSync(join(dir, `${name}_js`, `${name}.wasm`)), { sanityCheck: true });
    await tester.loadConstraints();
    const indexes = new Map();
    const groupByWire = groupConstraints ? new Uint8Array(tester.nVars) : null;
    const groups = ['wiring', 'pixelValid', 'planarity', 'planarity.plane', 'planarity.support',
        'rgbDepth', 'pose', 'temporal', 'temporal.pair', 'quality', 'motion', 'motion.imu'];
    const symbolStream = createInterface({ input: createReadStream(join(dir, `${name}.sym`)), crlfDelay: Infinity });
    for await (const line of symbolStream) {
        const [, index, , label] = line.split(',');
        const wire = Number(index);
        if (!(wire > 0)) continue;
        const key = label.slice('main.'.length);
        if (wanted.has(key)) indexes.set(key, wire);
        if (groupByWire) {
            for (let g = groups.length - 1; g > 0; g--) {
                const prefix = groups[g];
                if (key.startsWith(`${prefix}.`) || key.startsWith(`${prefix}[`)) {
                    groupByWire[wire] = g;
                    break;
                }
            }
        }
    }
    if (groupByWire) {
        const counts = groups.map(() => 0);
        for (const constraint of tester.constraints) {
            let group = 0;
            let mixed = false;
            for (const lc of constraint) {
                for (const key in lc) {
                    const next = groupByWire[key];
                    if (next && group && next !== group) mixed = true;
                    if (next) group = next;
                }
            }
            counts[mixed ? 0 : group]++;
        }
        console.log(`geometry R1CS groups (O1): ${groups.map((g, i) => `${g}=${counts[i]}`).join(' ')} total=${tester.constraints.length}`);
    }
    for (const signal of wanted) assert(indexes.has(signal), `missing R1CS signal ${signal}`);
    return { calculator, tester, indexes };
}
function value(circuit, witness, signal) {
    return witness[circuit.indexes.get(signal)];
}
async function expectRejection(circuit, witness, updates) {
    const modified = witness.slice();
    for (const [signal, replacement] of Object.entries(updates)) {
        const index = circuit.indexes.get(signal);
        assert(index > 0 && witness[index] !== field(replacement), `modification did not change ${signal}`);
        modified[index] = field(replacement);
    }
    await assert.rejects(circuit.tester.checkConstraints(modified), /^Error: Constraint doesn't match$/);
}

const gadgetOutputs = new Set('absolute negative less inside q rem scaled max first picked both either count band0 band7 band8 absLe absGe'.split(' '));
function bandOutputs(x) {
    return { inside: Number(-16 <= x && x < 16), band0: Number(-1 <= x && x < 1), band7: Number(-128 <= x && x < 128),
        band8: 1, absLe: Number(Math.abs(x) <= 16), absGe: Number(Math.abs(x) >= 16), absolute: Math.abs(x),
        negative: Number(x < 0), scaled: Math.floor(x/8) };
}
test('bounded arithmetic, strict first maximum, and mux boundaries', async t => {
    const circuit = await compile(scratch(t), join(harness, 'gadgets.circom'), gadgetOutputs);
    const vectors = [
        [-256, 255, 1, 1, [0, 0], [4, 4, 4, 4]],
        [-255, -255, 255, 1, [1, 0], [1, 2, 2, 2]],
        [-17, -16, 8, 1, [0, 1], [0, 0, 255, 3]],
        [-16, -17, 8, 1, [1, 1], [5, 6, 2, 9]],
        [-15, 15, 8, 1, [1, 1], [0, 0, 1, 1]],
        [15, 16, 7, 1, [1, 0], [8, 1, 8, 1]],
        [16, 15, 7, 1, [0, 0], [9, 1, 4, 4]],
        [17, 17, 7, 1, [1, 1], [0, 1, 2, 3]],
        [255, -256, 1, 1, [1, 1], [1, 2, 3, 4]],
        [-256, -256, 0, 0, [0, 0], [0, 0, 0, 0]],
        [255, 255, 0, 0, [0, 1], [1, 0, 1, 1]],
    ];
    for (const [x, y, den, enabled, sel, values] of vectors) {
        const witness = await circuit.calculator.calculateWitness(Object.fromEntries(
            Object.entries({ x, y, den, enabled, sel, values }).map(([name, input]) => [name, encode(input)])), true);
        const q = enabled ? Math.floor(x / den) : 0;
        const max = Math.max(...values);
        const expected = { ...bandOutputs(x), less: Number(x < y), q, rem: enabled ? x - q*den : 0,
            max, first: values.indexOf(max), picked: values[sel[0]+2*sel[1]],
            both: enabled*sel[0]*sel[1], either: Number(Boolean(enabled || sel[0] || sel[1])), count: enabled+sel[0]+sel[1] };
        for (const [name, expectedValue] of Object.entries(expected)) assert.equal(value(circuit, witness, name), field(expectedValue), name);
        await circuit.tester.checkConstraints(witness);
    }
    await t.test('bands, absolute value and shifts over the whole signed 8-bit domain', async () => {
        for (let x = -256; x < 256; x++) {
            const witness = await circuit.calculator.calculateWitness({ x: encode(x), y: '0', den: '1', enabled: '1',
                sel: ['0','0'], values: ['0','0','0','0'] }, true);
            for (const [name, expectedValue] of Object.entries(bandOutputs(x))) {
                assert.equal(value(circuit, witness, name), field(expectedValue), `${name}(${x})`);
            }
            await circuit.tester.checkConstraints(witness);
        }
    });
    await t.test('modified band output is rejected by R1CS', async () => {
        const witness = await circuit.calculator.calculateWitness({ x: encode(-129), y: '0', den: '1', enabled: '1',
            sel: ['0','0'], values: ['0','0','0','0'] }, true);
        await expectRejection(circuit, witness, { band7: 1n });
        await expectRejection(circuit, witness, { absGe: 0n });
    });
    await t.test('integer identity with a remainder outside [0, den) is rejected by R1CS', async () => {
        const witness = await circuit.calculator.calculateWitness({ x: encode(-17), y: '0', den: '8', enabled: '1', sel: ['0','0'], values: ['0','0','0','0'] }, true);
        await circuit.tester.checkConstraints(witness);
        const q = value(circuit, witness, 'q') + 1n;
        const rem = value(circuit, witness, 'rem') - 8n;
        assert.equal(field(8n*q+rem), field(-17));
        await expectRejection(circuit, witness, { q, rem });
    });
});

test('generated constants are current and bind the protocol policy_id', () => {
    const files = renderPolicy(readFileSync(join(root, 'policies/galaxy-pc-v1m.json'), 'utf8'));
    for (const [name, data] of Object.entries(files)) {
        assert.equal(readFileSync(join(root, 'circuits/physical-consistency/generated', name), 'utf8'), data, name);
    }
    const vector = JSON.parse(readFileSync(join(root, 'test-vectors/protocol/policy.json'), 'utf8')).cases[0];
    assert.equal(JSON.parse(files['policy_v1m.manifest.json']).policy_id, vector.id);
});

function readView(path) {
    const view = JSON.parse(readFileSync(path, 'utf8'));
    assert.equal(view.version, 1, 'GeometryView version');
    assert.deepEqual(Object.keys(view.expected).sort(), [...statNames, ...flagNames, 'pass'].sort());
    return view;
}
async function compareView(circuit, view) {
    const witness = await circuit.calculator.calculateWitness(Object.fromEntries(inputNames.map(name => [name, encode(view[name])])), true);
    await circuit.tester.checkConstraints(witness);
    for (const name of statNames) assert.equal(value(circuit, witness, `st_${name}`), BigInt(view.expected[name]), name);
    for (const name of flagNames) assert.equal(value(circuit, witness, name), BigInt(view.expected[name]), name);
    assert.equal(flagNames.every(name => value(circuit, witness, name) === 1n), view.expected.pass, 'pass');
    return witness;
}

const negativeSignals = ['planarity.support[0].inlier[0]', 'motion.imu[0].integral[0][0]',
    'planarity.plane[0].fit[0].q', 'planarity.plane[0].fit[0].r', 'planarity.plane[0].den'];
const projectedInputs = Array.from({ length: 64 }, (_, i) => `pr_bx[${i}]`);
const projectionDivision = Array.from({ length: 64 }, (_, i) =>
    ['q', 'r'].map(s => `temporal.pair[${i}].quotient[0].${s}`).concat(`temporal.pair[${i}].guarded[0].safeDen`)).flat();

test('production policy agrees with every reference GeometryView', async t => {
    const names = readdirSync(fixtures, { withFileTypes: true })
        .filter(entry => entry.isFile() && entry.name.endsWith('.json')).map(entry => entry.name).sort();
    assert(names.length > 0, 'missing default reference fixtures');
    const circuit = await compile(scratch(t), join(harness, 'policy.circom'),
        new Set([...outputNames, ...negativeSignals, ...projectedInputs, ...projectionDivision]), true);
    assert(circuit.tester.constraints.length <= 450_000, 'policy constraint budget exceeded');
    let pass;
    let offscreen;
    for (const name of names) {
        await t.test(name, async () => {
            const view = readView(join(fixtures, name));
            const witness = await compareView(circuit, view);
            if (!pass && view.expected.pass) pass = { view, witness };
            if (!offscreen && view.pairs.some(pair => !pair.inb)) offscreen = { view, witness };
        });
    }
    assert(pass, 'at least one passing reference fixture required');
    assert(offscreen, 'at least one rejected projection required');
    const { view, witness } = pass;
    const inside = view.pairs.findIndex(pair => pair.inb);
    const outside = offscreen.view.pairs.findIndex(pair => !pair.inb);
    assert(inside >= 0 && outside >= 0);
    const [inlier, integral, fitQ, fitR, fitDen] = negativeSignals;
    // q + 1 and r - den keep num = q*den + r but leave r outside [0, den).
    const shiftedDivision = (q, r, den) => ({ [q]: value(circuit, witness, q) + 1n,
        [r]: value(circuit, witness, r) - value(circuit, witness, den) });
    const pairDivision = [`temporal.pair[${inside}].quotient[0].q`, `temporal.pair[${inside}].quotient[0].r`,
        `temporal.pair[${inside}].guarded[0].safeDen`];
    const cases = [
        ['boolean output', witness, { quality_ok: 1n - value(circuit, witness, 'quality_ok') }],
        ['inlier predicate', witness, { [inlier]: 1n - value(circuit, witness, inlier) }],
        ['in-bounds projected block', witness, { [`pr_bx[${inside}]`]: (view.pr_bx[inside]+1)%64 }],
        ['offscreen projected block', offscreen.witness, { [`pr_bx[${outside}]`]: 1 }],
        ['first maximizing hypothesis', witness, { st_j_star: (view.expected.j_star+1)%4 }],
        ['IMU integral term', witness, { [integral]: value(circuit, witness, integral)+1n }],
        ['plane coefficient quotient/remainder pair', witness, shiftedDivision(fitQ, fitR, fitDen)],
        ['projection quotient/remainder pair', witness, shiftedDivision(...pairDivision)],
    ];
    for (const [name, original, changes] of cases) {
        await t.test(`R1CS rejects modified ${name}`, () => expectRejection(circuit, original, changes));
    }
    // Release constraint objects before compiling the isolated coefficient profile.
    circuit.tester.constraints = null;
});

test('isolated test-only policy checks coefficient overflow against the reference', async t => {
    const coeff = join(fixtures, 'coefficient-overflow');
    const stage = scratch(t);
    const generated = join(stage, 'circuits/physical-consistency/generated');
    const stagedHarness = join(stage, 'circuits/tests/geometry');
    mkdirSync(generated, { recursive: true });
    mkdirSync(stagedHarness, { recursive: true });
    cpSync(join(root, 'circuits/components/geometry'), join(stage, 'circuits/components/geometry'), { recursive: true });
    symlinkSync(join(root, 'circuits/node_modules'), join(stage, 'circuits/node_modules'), 'dir');
    cpSync(join(harness, 'policy.circom'), join(stagedHarness, 'policy.circom'));
    const files = renderPolicy(readFileSync(join(coeff, 'policy-test-dmin1.json'), 'utf8'));
    for (const [name, data] of Object.entries(files)) writeFileSync(join(generated, name), data);
    const circuit = await compile(stage, join(stagedHarness, 'policy.circom'), new Set(outputNames));
    await compareView(circuit, readView(join(coeff, 'view.json')));
});
