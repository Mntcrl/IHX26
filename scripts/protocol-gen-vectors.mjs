import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(new URL('../circuits/package.json', import.meta.url));
const { buildPoseidon } = require('circomlibjs');
const poseidon = await buildPoseidon();
const H = (...xs) => poseidon.F.toObject(poseidon(xs));
const R = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const tag = (code, aux) => (0x47414c58n << 96n) + (1n << 80n) + (BigInt(code) << 64n) + BigInt(aux);
const hash = (...parts) => createHash('sha256').update(Buffer.concat(parts.map(p => Buffer.from(p)))).digest();
const hex = n => n.toString(16).padStart(64, '0');
const dir = new URL('../test-vectors/protocol/', import.meta.url);
mkdirSync(dir, { recursive: true });
const emit = (filename, description, section, cases) => writeFileSync(new URL(filename, dir), JSON.stringify({ description, section, cases }, (_, value) => typeof value === 'bigint' ? value.toString() : value, 2) + '\n');

emit('poseidon.json', 'Circomlibjs buildPoseidon reference, Poseidon input arities one through five', '§6',
  Array.from({ length: 5 }, (_, i) => {
    const n = i + 1;
    return [new Array(n).fill(0n), Array.from({ length: n }, (_, j) => BigInt(j + 1)), Array.from({ length: n }, (_, j) => R - BigInt(j + 1))];
  }).flat().map(inputs => ({ inputs, output: H(...inputs) })));

const canonical = x => x && typeof x === 'object'
  ? Array.isArray(x) ? '[' + x.map(canonical).join(',') + ']' : '{' + Object.keys(x).sort().map(k => JSON.stringify(k) + ':' + canonical(x[k])).join(',') + '}'
  : JSON.stringify(x);
const policy = JSON.parse(readFileSync(new URL('../policies/galaxy-pc-v1m.json', import.meta.url)));
const policyJson = canonical(policy);
emit('policy.json', 'Canonical bytes and domain-separated ID of the initial mobile policy', '§2, §3, §8.7',
  [{ canonical_hex: Buffer.from(policyJson).toString('hex'), id: hash('GALAXY/POLICY/v1\0', policyJson).toString('hex') }]);

emit('field.json', 'BN254 canonical scalar boundaries, raw hash limbs and auxiliary domain tags', '§2, §6', [{
  scalars: [0n, 1n, 1n << 128n, R - (1n << 127n), R - 1n, R, (1n << 256n) - 1n].map(n => ({ bytes: hex(n), decimal: n, canonical: n < R })),
  limbs: [{ bytes: '0123456789abcdef'.repeat(2) + 'fedcba9876543210'.repeat(2), hi: BigInt('0x' + '0123456789abcdef'.repeat(2)), lo: BigInt('0x' + 'fedcba9876543210'.repeat(2)) }],
  tags: Array.from({ length: 12 }, (_, i) => ({ code: i + 1, aux: i === 11 ? 2n ** 64n - 1n : BigInt(i) * 4397n, value: tag(i + 1, i === 11 ? 2n ** 64n - 1n : BigInt(i) * 4397n) })),
}]);

function be32(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n);
  return b;
}

function tileBlock(seed, counter) {
  return hash('GALAXY/SAMPLE-TILES/v1\0', seed, be32(counter));
}

function* tileWords(seed) {
  for (let c = 0; ; c++) {
    const bytes = tileBlock(seed, c);
    for (let offset = 0; offset < 32; offset += 4) yield bytes.readUInt32BE(offset);
  }
}

function select(pool, words) {
  for (let i = 0; i < 8; i++) {
    const n = pool.length - i;
    const limit = 2 ** 32 - (2 ** 32 % n);
    let x;
    do { x = words.next().value; } while (x >= limit);
    const j = i + x % n;
    [pool[i], pool[j]] = [pool[j], pool[i]];
  }
  return pool.slice(0, 8).sort((a, b) => a - b);
}

const center = tile => tile % 8 >= 2 && tile % 8 <= 5 && Math.floor(tile / 8) >= 1 && Math.floor(tile / 8) <= 4;
const tilesInRaster = Array.from({length: 48}, (_, i) => i);
const bitsLE = x => x.toString(2).padStart(254, '0').split('').reverse().join('');
const limb = bytes => BigInt('0x' + bytes.toString('hex'));
const limbs = bytes => [limb(bytes.subarray(0, 16)), limb(bytes.subarray(16))];
const sampling = Array.from({length: 24}, (_, i) => {
  const network_id = [1, 2, 0, 0xffffffff][i % 4];
  const capture = i === 0 ? Buffer.alloc(32) : hash('test vector capture', be32(i));
  const ledger = i === 1 ? Buffer.alloc(32, 0xff) : hash('test vector ledger', be32(i));
  const lsel = i === 2 ? 0xffffffff : 22222222 + i;
  const seed = hash('GALAXY/SAMPLE/v1\0', be32(network_id), capture, be32(lsel), ledger);
  const words = tileWords(seed);
  const tiles = [...select(tilesInRaster.filter(center), words), ...select(tilesInRaster.filter(t => !center(t)), words)];
  const ctx = H(tag(10, 0), ...limbs(capture), ...limbs(seed));
  const slots = tiles.map((tile, s) => {
    const h = H(tag(11, s), ctx, BigInt(tile));
    const bits = bitsLE(h);
    const at = i => Number(bits[i]);
    const blocks = Array.from({length: 4}, (_, q) => [4*(q % 2) + at(4*q) + 2*at(4*q+1), 4*Math.floor(q/2) + at(4*q+2) + 2*at(4*q+3)]);
    const temporal = Array.from({length: 4}, (_, q) => [1 + at(16+2*q), 1 + at(17+2*q)]);
    return {h, bits, blocks, temporal};
  });
  const hypotheses = Array.from({length: 8}, (_, j) => {
    const h = H(tag(12, j), ctx);
    const bits = bitsLE(h);
    const width = j < 4 ? 8 : 7;
    const indices = Array.from({length: 3}, (_, m) => parseInt(bits.slice(m*width, (m+1)*width).split('').reverse().join(''), 2));
    return {h, bits, indices};
  });
  return {network_id, capture_id: capture.toString('hex'), lsel, lsel_hash: ledger.toString('hex'), seed: seed.toString('hex'), block0: tileBlock(seed, 0).toString('hex'), tiles, ctx, slots, hypotheses};
});
emit('sampling.json', 'Independent SHA-256 and circomlibJs cross-check of stratification, context, canonical bits, tile and hypothesis picks', '§3, §7', sampling);
const sample = sampling[3];
emit('public.json', 'Exact order of the 22 Galaxy v1 public inputs', '§9, §22', [{
  rgb_root: R - 123n, evidence_root: 456n, capture_id: sample.capture_id, seed: sample.seed, tiles: sample.tiles,
  inputs: [R - 123n, 456n, ...limbs(Buffer.from(sample.capture_id, 'hex')), ...limbs(Buffer.from(sample.seed, 'hex')), ...sample.tiles.map(BigInt)],
}]);

const morton = (x, y) => [0, 1, 2].reduce((s, k) => s | (((x >> k) & 1) << (2*k)) | (((y >> k) & 1) << (2*k+1)), 0);
const packDepth = (d, c) => d.reduce((n, e, i) => (n << 18n) + BigInt(4*e + c[i]), 0n);
function block(x0, y0, frame) {
  const r = [], d = [], c = [];
  for (let y = y0; y < y0+4; y++) for (let x = x0; x < x0+4; x++) {
    r.push((3*x + y + 13*frame) % 256, (x + 5*y + 31*frame) % 256, (7*x + 11*y + 97*frame) % 256);
    d.push((191*x + 103*y + 1007*frame) % 65536);
    c.push((2*x + y + frame) % 4);
  }
  return [limb(Buffer.from(r.slice(0, 24))), limb(Buffer.from(r.slice(24))), packDepth(d.slice(0, 8), c.slice(0, 8)), packDepth(d.slice(8), c.slice(8))];
}

const cornerBlocks = [[0,0,0], [7,7,0], [40,0,7], [47,7,7], [39,3,5], [8,1,2], [24,4,5], [46,5,7]];
function generateTree(kind, frame) {
  const node = (level, i, children) => H(tag(4, (kind << 24) | (frame << 20) | (level << 16) | i), ...children);
  const leaf = (slot, all) => H(tag(kind, (frame << 12) | slot), ...(kind === 1 ? all.slice(0,2) : kind === 2 ? all.slice(2) : all));
  const levels = Array.from({length: 7}, (_, l) => new Array(4096 >> (2*l)));
  for (let ty = 0; ty < 8; ty++) for (let tx = 0; tx < 8; tx++) {
    const pos = morton(tx, ty);
    if (ty >= 6) {
      levels[3][pos] = H(tag(5, (kind << 16) | (frame << 8) | pos), 0n);
      continue;
    }
    for (let by = 0; by < 8; by++) for (let bx = 0; bx < 8; bx++) {
      const i = pos*64 + morton(bx, by);
      levels[0][i] = leaf(i, block(tx*32 + bx*4, ty*32 + by*4, frame));
    }
  }
  for (let l = 1; l <= 6; l++) {
    for (let i = 0; i < levels[l].length; i++) {
      if (l === 3 && levels[l][i] !== undefined) continue;
      const children = levels[l-1].slice(i*4, i*4+4);
      if (children[0] !== undefined) levels[l][i] = node(l, i, children);
    }
  }
  const openings = cornerBlocks.map(([tile, bx, by]) => {
    const tx = tile % 8, ty = Math.floor(tile / 8);
    const slot = morton(tx, ty)*64 + morton(bx, by);
    const packed = block(tx*32 + bx*4, ty*32 + by*4, frame);
    const siblings = Array.from({length: 6}, (_, l) => {
      const i = slot >> (2*l);
      return levels[l].slice(i & ~3, (i & ~3) + 4).filter((_, j) => j !== (i & 3));
    });
    return {tile, bx, by, slot, packed, leaf: leaf(slot, packed), siblings};
  });
  return {kind, frame, root: levels[6][0], openings};
}
const trees = [[1, 1], [2, 1], [3, 0], [3, 2]].map(([k, f]) => generateTree(k, f));
emit('trees.json', 'Synthetic 256×192 frame in each tree mode; packed leaves and openings (including all corners and padding neighbors)', '§6', trees);

const salts = Array.from({length: 8}, (_, i) => i === 0 ? R - 1n : 572n + BigInt(i));
const elements = [54, 133, 31, 7, 0].map((n, k) => Array.from({length: n}, (_, i) => (R - BigInt(i) * 17n + BigInt(k)) % R));
const components = [
  trees[0].root,
  ...trees.slice(1).map((tree, i) => H(tag(6, i+1), salts[i], tree.root)),
  ...elements.map((values, i) => {
    let h = H(tag(7, i+4), salts[i+3], BigInt(values.length));
    for (let j = 0; j < values.length; j += 4) h = H(h, ...[values[j], values[j+1] ?? 0n, values[j+2] ?? 0n, values[j+3] ?? 0n]);
    return h;
  }),
  ...Array.from({length: 7}, (_, i) => H(tag(9, i+9), 0n)),
];
const root = H(tag(8, 2 << 16), ...Array.from({length: 4}, (_, i) => H(tag(8, (1 << 16) | i), ...components.slice(4*i, 4*i+4))));
emit('commitments.json', 'Synthetic frame and scalar commitments, padding and evidence root', '§6', [{tree_roots: trees.map(t => t.root), salts, elements, components, root}]);
