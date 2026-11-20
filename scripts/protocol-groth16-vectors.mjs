// Encodes the snarkjs proof/VK in test-vectors/protocol/groth16/ into §9 bytes (independently
// of the Rust codec) and writes vectors.json with positive and negative cases.
// Run via scripts/protocol-groth16-vectors.sh.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const dir = process.argv[2];
const load = name => JSON.parse(readFileSync(join(dir, name), 'utf8'));
const vk = load('verification_key.json');
const proof = load('proof.json');
const pub = load('public.json');

const Q = 21888242871839275222246405745257275088696311157297823662689037894645226208583n;
const R = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const mod = (a, m = Q) => ((a % m) + m) % m;
const pow = (b, e, m = Q) => { let r = 1n; b = mod(b, m); for (; e > 0n; e >>= 1n, b = b * b % m) if (e & 1n) r = r * b % m; return r; };
const inv = a => pow(a, Q - 2n);
const hex32 = n => n.toString(16).padStart(64, '0');

// Fq2 = Fq[u]/(u^2 + 1), elements [c0, c1].
const f2 = {
  add: ([a, b], [c, d]) => [mod(a + c), mod(b + d)],
  mul: ([a, b], [c, d]) => [mod(a * c - b * d), mod(a * d + b * c)],
  inv: ([a, b]) => { const t = inv(mod(a * a + b * b)); return [mod(a * t), mod(-b * t)]; },
  eq: ([a, b], [c, d]) => a === c && b === d,
};
const sqrtFq = a => { const r = pow(a, (Q + 1n) / 4n); return r * r % Q === mod(a) ? r : null; };
const sqrtF2 = ([a0, a1]) => {
  const n = sqrtFq(mod(a0 * a0 + a1 * a1));
  if (n === null) return null;
  const half = inv(2n);
  for (const t of [mod((a0 + n) * half), mod((a0 - n) * half)]) {
    const x0 = sqrtFq(t);
    if (x0 === null || x0 === 0n) continue;
    const x = [x0, mod(a1 * inv(2n * x0))];
    if (f2.eq(f2.mul(x, x), [a0, a1])) return x;
  }
  return null;
};
// BN254 twist: y^2 = x^3 + 3 / (9 + u).
const B2 = f2.mul([3n, 0n], f2.inv([9n, 1n]));
const onTwist = (x, y) => f2.eq(f2.mul(y, y), f2.add(f2.mul(f2.mul(x, x), x), B2));

const g1 = p => [BigInt(p[0]), BigInt(p[1])];
const g2 = p => [[BigInt(p[0][0]), BigInt(p[0][1])], [BigInt(p[1][0]), BigInt(p[1][1])]];
const encG1 = ([x, y]) => hex32(x) + hex32(y);
const encG2 = ([[x0, x1], [y0, y1]]) => hex32(x0) + hex32(x1) + hex32(y0) + hex32(y1);

for (const p of [vk.vk_beta_2, vk.vk_gamma_2, vk.vk_delta_2, proof.pi_b]) {
  if (!onTwist(...g2(p))) throw new Error('snarkjs G2 point is not on the twist with [c0, c1] order');
}

const A = g1(proof.pi_a), Bp = g2(proof.pi_b), C = g1(proof.pi_c);
const proofHex = A2 => encG1(A2.a) + encG2(A2.b) + encG1(A2.c);
const validProof = proofHex({ a: A, b: Bp, c: C });
const encVk = v => {
  const n = v.ic.length - 1;
  return '0101' + n.toString(16).padStart(8, '0') + encG1(v.alpha) + encG2(v.beta) + encG2(v.gamma) + encG2(v.delta) + v.ic.map(encG1).join('');
};
const vkParts = { alpha: g1(vk.vk_alpha_1), beta: g2(vk.vk_beta_2), gamma: g2(vk.vk_gamma_2), delta: g2(vk.vk_delta_2), ic: vk.IC.map(g1) };
const validVk = encVk(vkParts);
if (vk.nPublic !== pub.length || vk.IC.length !== pub.length + 1) throw new Error('nPublic mismatch');

// A point on the twist outside the order-r subgroup: the G2 cofactor is ~2^254, so a point
// found by incrementing x is in the subgroup with negligible probability; the Rust decoder
// must report NotInSubgroup (not NotOnCurve) for it.
let wrongSubgroup = null;
for (let x0 = 1n; wrongSubgroup === null; x0++) {
  const x = [x0, 1n];
  const y = sqrtF2(f2.add(f2.mul(f2.mul(x, x), x), B2));
  if (y !== null && onTwist(x, y)) wrongSubgroup = [x, y];
}

const flip = ([[x0, x1], [y0, y1]]) => [[x1, x0], [y1, y0]];
const zero64 = '0'.repeat(128), zero128 = '0'.repeat(256);
const pubStr = pub.map(String);
const cases = [
  { name: 'valid', description: 'snarkjs proof, independently encoded', proof_hex: validProof, vk_hex: validVk, public: pubStr, expect: 'verify:true' },
  { name: 'flipped_c0_c1', description: 'B with c0/c1 swapped in both coordinates', proof_hex: proofHex({ a: A, b: flip(Bp), c: C }), vk_hex: validVk, public: pubStr, expect: 'error:NotOnCurve' },
  { name: 'off_curve_a', description: 'A.y + 1', proof_hex: proofHex({ a: [A[0], mod(A[1] + 1n)], b: Bp, c: C }), vk_hex: validVk, public: pubStr, expect: 'error:NotOnCurve' },
  { name: 'g2_wrong_subgroup', description: 'B on the twist but outside the order-r subgroup', proof_hex: proofHex({ a: A, b: wrongSubgroup, c: C }), vk_hex: validVk, public: pubStr, expect: 'error:NotInSubgroup' },
  { name: 'infinity_a', description: 'A encoded as all-zero bytes', proof_hex: zero64 + validProof.slice(128), vk_hex: validVk, public: pubStr, expect: 'error:Infinity' },
  { name: 'infinity_b', description: 'B encoded as all-zero bytes', proof_hex: validProof.slice(0, 128) + zero128 + validProof.slice(384), vk_hex: validVk, public: pubStr, expect: 'error:Infinity' },
  { name: 'coordinate_plus_q', description: 'A.x + q (same residue, non-canonical)', proof_hex: proofHex({ a: [A[0] + Q, A[1]], b: Bp, c: C }), vk_hex: validVk, public: pubStr, expect: 'error:NonCanonical' },
  { name: 'coordinate_eq_q', description: 'C.y = q', proof_hex: proofHex({ a: A, b: Bp, c: [C[0], Q] }), vk_hex: validVk, public: pubStr, expect: 'error:NonCanonical' },
  { name: 'b_coordinate_ge_q', description: 'B.x.c1 + q', proof_hex: proofHex({ a: A, b: [[Bp[0][0], Bp[0][1] + Q], Bp[1]], c: C }), vk_hex: validVk, public: pubStr, expect: 'error:NonCanonical' },
  { name: 'swapped_a_c', description: 'valid points, A and C exchanged', proof_hex: proofHex({ a: C, b: Bp, c: A }), vk_hex: validVk, public: pubStr, expect: 'verify:false' },
  { name: 'wrong_public_input', description: 'public[0] + 1', proof_hex: validProof, vk_hex: validVk, public: [String((BigInt(pub[0]) + 1n) % R), ...pubStr.slice(1)], expect: 'verify:false' },
  { name: 'public_input_ge_r', description: 'public[0] + r (non-canonical scalar)', proof_hex: validProof, vk_hex: validVk, public: [String(BigInt(pub[0]) + R), ...pubStr.slice(1)], expect: 'error:Scalar' },
  { name: 'too_few_public_inputs', description: 'n_public - 1 inputs', proof_hex: validProof, vk_hex: validVk, public: pubStr.slice(0, -1), expect: 'error:PublicInputs' },
  { name: 'too_many_public_inputs', description: 'n_public + 1 inputs', proof_hex: validProof, vk_hex: validVk, public: [...pubStr, '0'], expect: 'error:PublicInputs' },
  { name: 'proof_truncated', description: '255-byte proof', proof_hex: validProof.slice(0, -2), vk_hex: validVk, public: pubStr, expect: 'error:Length' },
  { name: 'proof_extended', description: '257-byte proof', proof_hex: validProof + '00', vk_hex: validVk, public: pubStr, expect: 'error:Length' },
  { name: 'vk_version', description: 'VK version byte 2', proof_hex: validProof, vk_hex: '02' + validVk.slice(2), public: pubStr, expect: 'error:VkHeader' },
  { name: 'vk_curve', description: 'VK curve byte 2', proof_hex: validProof, vk_hex: '0102' + validVk.slice(4), public: pubStr, expect: 'error:VkHeader' },
  { name: 'vk_n_public_mismatch', description: 'VK header n_public + 1 without an extra IC point', proof_hex: validProof, vk_hex: '0101' + (pub.length + 1).toString(16).padStart(8, '0') + validVk.slice(12), public: pubStr, expect: 'error:Length' },
  { name: 'vk_trailing', description: 'VK with one trailing byte', proof_hex: validProof, vk_hex: validVk + '00', public: pubStr, expect: 'error:Length' },
  { name: 'vk_delta_wrong_subgroup', description: 'delta_g2 outside the order-r subgroup', proof_hex: validProof, vk_hex: encVk({ ...vkParts, delta: wrongSubgroup }), public: pubStr, expect: 'error:NotInSubgroup' },
  { name: 'vk_gamma_flipped', description: 'gamma_g2 with c0/c1 swapped', proof_hex: validProof, vk_hex: encVk({ ...vkParts, gamma: flip(vkParts.gamma) }), public: pubStr, expect: 'error:NotOnCurve' },
];

writeFileSync(join(dir, 'vectors.json'), JSON.stringify({
  description: 'Groth16 BN254 proof and VK byte encodings (§9) cross-checked against a real snarkjs proof of tiny.circom (development single-party setup), with negative cases. expect: verify:true|verify:false|error:<Groth16Error variant> (Scalar = public input rejected by field::fr_from_dec).',
  section: '§9, §16',
  files: { circuit: 'tiny.circom', verification_key: 'verification_key.json', proof: 'proof.json', public: 'public.json' },
  n_public: pub.length,
  cases,
}, null, 2) + '\n');
