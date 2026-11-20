// Independent (Node.js) construction of the §3 identifiers and §10 wire formats.
// Writes test-vectors/protocol/{ids,wire}.json, consumed by crates/galaxy-protocol/tests/wire.rs.
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';

const dir = new URL('../test-vectors/protocol/', import.meta.url);
mkdirSync(dir, { recursive: true });
const emit = (filename, description, section, cases) =>
  writeFileSync(new URL(filename, dir), JSON.stringify({ description, section, cases }, null, 2) + '\n');

const sha = (...parts) => createHash('sha256').update(Buffer.concat(parts.map(p => Buffer.from(p)))).digest();
const u8 = n => Buffer.from([n]);
const u16 = n => { const b = Buffer.alloc(2); b.writeUInt16BE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
const u64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return b; };
const fill = (len, seed) => Buffer.from(Array.from({ length: len }, (_, i) => (seed * 31 + i * 7) & 0xff));
const h32 = seed => sha('wire vector', u32(seed));
const cat = (...parts) => Buffer.concat(parts.map(p => Buffer.from(p)));
const hex = b => Buffer.from(b).toString('hex');
const R = Buffer.from('30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001', 'hex');

// ---------- §10 CAPTURE ----------
const core = {
  network: 1, registration_tx: h32(1), preledger_hash: h32(2), nonce: h32(3), preledger_index: 22480444,
  claimed_time_ms: '1795204800123', width: 1024, height: 768, flags: 0, photo_id: h32(4),
  rgb_root: Buffer.concat([Buffer.alloc(1), h32(5).subarray(1)]), evidence_root: Buffer.concat([Buffer.alloc(1), h32(6).subarray(1)]),
  policy_id: h32(7), circuit_id: h32(8), account_id: fill(20, 9),
};
const coreBytes = c => cat(u8(1), u32(c.network), c.registration_tx, c.preledger_hash, c.nonce, u32(c.preledger_index),
  u64(c.claimed_time_ms), u16(c.width), u16(c.height), u16(c.flags), c.photo_id, c.rgb_root, c.evidence_root,
  c.policy_id, c.circuit_id, c.account_id);
const core299 = coreBytes(core);
if (core299.length !== 299) throw new Error('core length');
const fieldsJson = o => Object.fromEntries(Object.entries(o).map(([k, v]) => [k, Buffer.isBuffer(v) ? hex(v) : v]));

const devAssertion = cat(u8(0xf0), fill(64, 10));
const cborAssertion = cat(u8(0xa2), fill(599, 11));
const captureMemo = (c, a) => cat(coreBytes(c), u16(a.length), a);
const coreJpeg = { ...core, flags: 1, rgb_root: R.map((b, i) => i === 31 ? b - 1 : b) };

// ---------- §10 PROOF ----------
const proof = { network: 1, manifest_tx: h32(20), selection_ledger_index: 22480449, selection_ledger_hash: h32(21), circuit_id: h32(8), proof: fill(256, 22) };
const proofBytes = p => cat(u8(1), u32(p.network), p.manifest_tx, u32(p.selection_ledger_index), p.selection_ledger_hash, p.circuit_id, p.proof);

// ---------- §10 CHUNK ----------
const chunkBytes = c => cat(u8(1), c.bundle_id, u16(c.index), u16(c.total), u16(c.payload.length), c.payload);
const chunkA = { bundle_id: h32(30), index: 0, total: 2, payload: fill(800, 31) };
const chunkB = { bundle_id: h32(30), index: 1, total: 2, payload: fill(1, 32) };

// ---------- §10 DEVICE manifest + bundle ----------
const deviceManifest = {
  network: 1, bundle_id: h32(40), bundle_len: 1601, chunk_total: 3, first_chunk_ledger: 22480400, last_chunk_ledger: 22480402,
  kind: 1, env: 2, key_id: h32(41), account_id: fill(20, 9), reg_ledger_index: 22480390, reg_ledger_hash: h32(42),
};
const deviceManifestBytes = m => cat(u8(1), u32(m.network), m.bundle_id, u32(m.bundle_len), u16(m.chunk_total),
  u32(m.first_chunk_ledger), u32(m.last_chunk_ledger), u8(m.kind), u8(m.env), m.key_id, m.account_id,
  u32(m.reg_ledger_index), m.reg_ledger_hash);
const devManifest = { ...deviceManifest, kind: 0xf0, env: 0, bundle_len: 158, chunk_total: 1, last_chunk_ledger: 22480400 };

const deviceBundle = {
  kind: 1, env: 1, app_id: 'PPS57BF3X3.utcq.XRPL-Galaxy', key_id: h32(41), network: 1, reg_ledger_index: 22480390,
  reg_ledger_hash: h32(42), reg_nonce: h32(43), account_id: fill(20, 9), attestation: fill(1200, 44),
};
const deviceBundleBytes = b => cat('GXDB', u8(1), u8(b.kind), u8(b.env), u16(Buffer.byteLength(b.app_id)), b.app_id,
  b.key_id, u32(b.network), u32(b.reg_ledger_index), b.reg_ledger_hash, b.reg_nonce, b.account_id, u32(b.attestation.length), b.attestation);
const devBundle = { ...deviceBundle, kind: 0xf0, env: 0, app_id: '', attestation: fill(32, 45) };

// ---------- §10 CIRCUIT manifest + bundle ----------
const circuitManifest = {
  network: 1, bundle_id: h32(50), bundle_len: 5000, chunk_total: 7, first_chunk_ledger: 22480500, last_chunk_ledger: 22480510,
  circuit_id: h32(8), policy_id: h32(7), vk_digest: h32(51),
};
const circuitManifestBytes = m => cat(u8(1), u32(m.network), m.bundle_id, u32(m.bundle_len), u16(m.chunk_total),
  u32(m.first_chunk_ledger), u32(m.last_chunk_ledger), m.circuit_id, m.policy_id, m.vk_digest);
// Bytewise key order: "n_public" < "name" because '_' (0x5F) < 'a' (0x61).
const circuitBundle = { descriptor_json: Buffer.from('{"n_public":3,"name":"tiny"}'), vk_bytes: fill(710, 52), policy_json: Buffer.from('{"name":"galaxy-pc-v1m","version":1}') };
const circuitBundleBytes = b => cat('GXCB', u8(1), u32(b.descriptor_json.length), b.descriptor_json, u32(b.vk_bytes.length), b.vk_bytes, u32(b.policy_json.length), b.policy_json);

// ---------- §10 REVOKE / NODE ----------
const revoke = { network: 1, registration_tx: h32(60), reason: 2, effective_ledger: 0 };
const revokeBytes = r => cat(u8(1), u32(r.network), r.registration_tx, u8(r.reason), u32(r.effective_ledger));
const node = { network: 1, node_pubkey: h32(70), expires_at: '1795204800', url: 'https://galaxy-node.example.org/', sig: fill(64, 71) };
const nodePayload = n => cat(u8(1), u32(n.network), n.node_pubkey, u64(n.expires_at), u16(n.url.length), n.url);
const nodeBytes = n => cat(nodePayload(n), n.sig);

const valid = (type, memo_type, fields, bytes, extra = {}) => ({ type, memo_type, fields: fieldsJson(fields), hex: hex(bytes), ...extra });
const validCases = [
  valid('CaptureMemo', 'galaxy/v1/capture', { core: fieldsJson(core), assertion: devAssertion }, captureMemo(core, devAssertion),
    { core_hex: hex(core299), description: 'dev-unattested assertion (0xF0 ‖ sig64), png-v1' }),
  valid('CaptureMemo', 'galaxy/v1/capture', { core: fieldsJson(coreJpeg), assertion: cborAssertion }, captureMemo(coreJpeg, cborAssertion),
    { core_hex: hex(coreBytes(coreJpeg)), description: 'maximum assertion_len 600, jpeg-v1, rgb_root = r - 1' }),
  valid('ProofMemo', 'galaxy/v1/proof', proof, proofBytes(proof), { description: '361-byte PROOF memo' }),
  valid('ChunkMemo', 'galaxy/v1/device-chunk', chunkA, chunkBytes(chunkA), { description: 'maximum payload 800' }),
  valid('ChunkMemo', 'galaxy/v1/circuit-chunk', chunkB, chunkBytes(chunkB), { description: 'minimum payload 1, last index' }),
  valid('DeviceManifest', 'galaxy/v1/device', deviceManifest, deviceManifestBytes(deviceManifest), { description: 'App Attest production' }),
  valid('DeviceManifest', 'galaxy/v1/device', devManifest, deviceManifestBytes(devManifest), { description: 'dev-unattested, env n/a' }),
  valid('DeviceBundle', null, deviceBundle, deviceBundleBytes(deviceBundle), { bundle_id: hex(sha('GALAXY/BUNDLE/v1\0', deviceBundleBytes(deviceBundle))), description: 'App Attest development bundle' }),
  valid('DeviceBundle', null, devBundle, deviceBundleBytes(devBundle), { bundle_id: hex(sha('GALAXY/BUNDLE/v1\0', deviceBundleBytes(devBundle))), description: 'dev-unattested bundle (empty app id, 32-byte public key)' }),
  valid('CircuitManifest', 'galaxy/v1/circuit', circuitManifest, circuitManifestBytes(circuitManifest), { description: 'CIRCUIT manifest' }),
  valid('CircuitBundle', null, circuitBundle, circuitBundleBytes(circuitBundle), { bundle_id: hex(sha('GALAXY/BUNDLE/v1\0', circuitBundleBytes(circuitBundle))), description: 'GXCB with canonical JSON documents' }),
  valid('RevokeMemo', 'galaxy/v1/revoke', revoke, revokeBytes(revoke), { description: 'effective_ledger 0 = revoke tx ledger' }),
  valid('NodeAnnouncement', 'galaxy/v1/node', node, nodeBytes(node), { signing_message: hex(cat('GALAXY/NODE/v1\0', nodePayload(node))), description: 'NODE announcement' }),
];

const set = (buf, at, bytes) => { const b = Buffer.from(buf); Buffer.from(bytes).copy(b, at); return b; };
const capture = captureMemo(core, devAssertion);
const reject = (type, description, bytes) => ({ type, description, hex: hex(bytes) });
const dm = deviceManifestBytes(deviceManifest);
const db = deviceBundleBytes(deviceBundle);
const rejectCases = [
  reject('CaptureMemo', 'version 2', set(capture, 0, [2])),
  reject('CaptureMemo', 'network id 3', set(capture, 1, u32(3))),
  reject('CaptureMemo', 'reserved flag bit 4', captureMemo({ ...core, flags: 0x10 }, devAssertion)),
  reject('CaptureMemo', 'reserved flag bit 15', captureMemo({ ...core, flags: 0x8000 }, devAssertion)),
  reject('CaptureMemo', 'unknown profile code 2', captureMemo({ ...core, flags: 2 }, devAssertion)),
  reject('CaptureMemo', 'rgb_root = r', captureMemo({ ...core, rgb_root: R }, devAssertion)),
  reject('CaptureMemo', 'evidence_root = 2^256 - 1', captureMemo({ ...core, evidence_root: Buffer.alloc(32, 0xff) }, devAssertion)),
  reject('CaptureMemo', 'assertion_len 601', captureMemo(core, cat(u8(0xa2), fill(600, 1)))),
  reject('CaptureMemo', 'assertion_len 0', captureMemo(core, Buffer.alloc(0))),
  reject('CaptureMemo', 'dev-unattested assertion of 64 bytes', captureMemo(core, devAssertion.subarray(0, 64))),
  reject('CaptureMemo', 'core only (no assertion length)', core299),
  reject('CaptureMemo', 'assertion shorter than declared', capture.subarray(0, capture.length - 1)),
  reject('CaptureMemo', 'trailing byte', cat(capture, u8(0))),
  reject('CaptureCore', 'trailing byte after core', cat(core299, u8(0))),
  reject('CaptureCore', '298 bytes', core299.subarray(0, 298)),
  reject('ProofMemo', '360 bytes', proofBytes(proof).subarray(0, 360)),
  reject('ProofMemo', '362 bytes', cat(proofBytes(proof), u8(0))),
  reject('ProofMemo', 'version 0', set(proofBytes(proof), 0, [0])),
  reject('ChunkMemo', 'index = total', chunkBytes({ ...chunkB, index: 2 })),
  reject('ChunkMemo', 'total 0', chunkBytes({ ...chunkB, index: 0, total: 0 })),
  reject('ChunkMemo', 'payload_len 0', chunkBytes({ ...chunkB, payload: Buffer.alloc(0) })),
  reject('ChunkMemo', 'payload 801', chunkBytes({ ...chunkB, payload: fill(801, 3) })),
  reject('ChunkMemo', 'declared payload longer than data', chunkBytes(chunkA).subarray(0, 838)),
  reject('ChunkMemo', 'trailing byte', cat(chunkBytes(chunkB), u8(0))),
  reject('DeviceManifest', 'kind 2', deviceManifestBytes({ ...deviceManifest, kind: 2 })),
  reject('DeviceManifest', 'env 3', deviceManifestBytes({ ...deviceManifest, env: 3 })),
  reject('DeviceManifest', 'App Attest with env n/a', deviceManifestBytes({ ...deviceManifest, env: 0 })),
  reject('DeviceManifest', 'dev-unattested with env development', deviceManifestBytes({ ...devManifest, env: 1 })),
  reject('DeviceManifest', 'bundle_len 0', deviceManifestBytes({ ...deviceManifest, bundle_len: 0 })),
  reject('DeviceManifest', 'bundle_len 1 MiB + 1', deviceManifestBytes({ ...deviceManifest, bundle_len: 1048577, chunk_total: 1400 })),
  reject('DeviceManifest', 'chunk_total 0', deviceManifestBytes({ ...deviceManifest, chunk_total: 0 })),
  reject('DeviceManifest', 'chunk_total > bundle_len', deviceManifestBytes({ ...deviceManifest, bundle_len: 2, chunk_total: 3 })),
  reject('DeviceManifest', 'bundle_len > chunk_total * 800', deviceManifestBytes({ ...deviceManifest, bundle_len: 2401 })),
  reject('DeviceManifest', 'first_chunk_ledger > last_chunk_ledger', deviceManifestBytes({ ...deviceManifest, first_chunk_ledger: 22480403 })),
  reject('DeviceManifest', 'trailing byte', cat(dm, u8(0))),
  reject('DeviceManifest', 'truncated', dm.subarray(0, dm.length - 1)),
  reject('DeviceBundle', 'magic GXDC', set(db, 0, 'GXDC')),
  reject('DeviceBundle', 'version 2', set(db, 4, [2])),
  reject('DeviceBundle', 'app id without dot', deviceBundleBytes({ ...deviceBundle, app_id: 'PPS57BF3X3' })),
  reject('DeviceBundle', 'app id with space', deviceBundleBytes({ ...deviceBundle, app_id: 'PPS57BF3X3.utcq XRPL' })),
  reject('DeviceBundle', 'App Attest with empty app id', deviceBundleBytes({ ...deviceBundle, app_id: '' })),
  reject('DeviceBundle', 'dev-unattested public key of 31 bytes', deviceBundleBytes({ ...devBundle, attestation: fill(31, 4) })),
  reject('DeviceBundle', 'att_len 0', deviceBundleBytes({ ...deviceBundle, attestation: Buffer.alloc(0) })),
  reject('DeviceBundle', 'network 7', deviceBundleBytes({ ...deviceBundle, network: 7 })),
  reject('DeviceBundle', 'trailing byte', cat(db, u8(0))),
  reject('CircuitManifest', 'truncated', circuitManifestBytes(circuitManifest).subarray(0, 146)),
  reject('CircuitManifest', 'bundle_len > chunk_total * 800', circuitManifestBytes({ ...circuitManifest, bundle_len: 5601 })),
  reject('CircuitBundle', 'descriptor with whitespace', circuitBundleBytes({ ...circuitBundle, descriptor_json: Buffer.from('{"name": "tiny"}') })),
  reject('CircuitBundle', 'descriptor keys not in bytewise order', circuitBundleBytes({ ...circuitBundle, descriptor_json: Buffer.from('{"name":"tiny","n_public":3}') })),
  reject('CircuitBundle', 'policy is an array', circuitBundleBytes({ ...circuitBundle, policy_json: Buffer.from('[1]') })),
  reject('CircuitBundle', 'empty vk bytes', circuitBundleBytes({ ...circuitBundle, vk_bytes: Buffer.alloc(0) })),
  reject('CircuitBundle', 'trailing byte', cat(circuitBundleBytes(circuitBundle), u8(0))),
  reject('RevokeMemo', 'truncated', revokeBytes(revoke).subarray(0, 41)),
  reject('RevokeMemo', 'trailing byte', cat(revokeBytes(revoke), u8(0))),
  reject('NodeAnnouncement', 'empty url', nodeBytes({ ...node, url: '' })),
  reject('NodeAnnouncement', 'url with space', nodeBytes({ ...node, url: 'https://a b/' })),
  reject('NodeAnnouncement', 'signature truncated', nodeBytes(node).subarray(0, nodeBytes(node).length - 1)),
];
emit('wire.json', 'Galaxy v1 memo and bundle encodings built field by field in Node.js; valid cases round-trip exactly, reject cases must fail strict decoding', '§2, §10',
  { valid: validCases, reject: rejectCases });

// ---------- §3 identifiers ----------
const rgb = fill(2 * 3 * 3, 80);
const policyJson = Buffer.from('{"name":"galaxy-pc-v1m","version":1}');
const descriptorJson = Buffer.from('{"n_public":3,"name":"tiny"}');
const regArgs = { network_id: 1, reg_ledger_index: 22480390, reg_ledger_hash: h32(42), reg_nonce: h32(43), key_id: h32(41), account_id: fill(20, 9) };
const devPk = h32(90);
const vkBytes = fill(710, 52);
const nodeDescriptor = Buffer.from('{"network":"xrpl-testnet","url":"https://galaxy-node.example.org/"}');
emit('ids.json', 'Domain-separated SHA-256 identifiers and attestation messages computed in Node.js', '§3, §11', [{
  photo_id: { w: 3, h: 2, rgb: hex(rgb), id: hex(sha('GALAXY/PHOTO/v1\0', u32(3), u32(2), rgb)) },
  capture_id: { core: hex(core299), id: hex(sha('GALAXY/CAPTURE/v1\0', core299)) },
  assertion_client_data: { core: hex(core299), client_data: hex(cat('GALAXY/ASSERT/v1\0', core299)), hash: hex(sha('GALAXY/ASSERT/v1\0', core299)) },
  registration_client_data_hash: { ...fieldsJson(regArgs), hash: hex(sha('GALAXY/DEVICE-REG/v1\0', u32(regArgs.network_id), u32(regArgs.reg_ledger_index), regArgs.reg_ledger_hash, regArgs.reg_nonce, regArgs.key_id, regArgs.account_id)) },
  dev_assertion_message: { core: hex(core299), message: hex(cat('GALAXY/DEV-ASSERT/v1\0', core299)) },
  dev_key_id: { public_key: hex(devPk), id: hex(sha('GALAXY/DEV-KEY/v1\0', devPk)) },
  bundle_id: { bundle: hex(db), id: hex(sha('GALAXY/BUNDLE/v1\0', db)) },
  policy_id: { json: policyJson.toString(), id: hex(sha('GALAXY/POLICY/v1\0', policyJson)) },
  circuit_id: { json: descriptorJson.toString(), id: hex(sha('GALAXY/CIRCUIT/v1\0', descriptorJson)) },
  vk_digest: { vk: hex(vkBytes), digest: hex(sha('GALAXY/VK/v1\0', vkBytes)) },
  node_descriptor_signature_message: { json: nodeDescriptor.toString(), message: hex(cat('GALAXY/NODE/v1\0', nodeDescriptor)) },
}]);
