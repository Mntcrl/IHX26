import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import init, { inspect_image, verify } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { replay } from './recording.mjs';

const [cli, imagePath] = process.argv.slice(2);
const root = fileURLToPath(new URL('../../../', import.meta.url));
const fixture = 'test-vectors/verifier/dev-unattested';
const recordingPath = `${fixture}/recording.json`;
const networkPath = `${fixture}/network.toml`;
const [image, recording, network, refs, wasm] = await Promise.all([
  readFile(imagePath), readFile(`${root}${recordingPath}`, 'utf8').then(JSON.parse),
  readFile(`${root}${networkPath}`, 'utf8'), readFile(`${root}${fixture}/refs.json`, 'utf8').then(JSON.parse),
  readFile(new URL('../../../bindings/wasm/pkg/galaxy_wasm_bg.wasm', import.meta.url)),
]);
assert.equal(createHash('sha256').update(image).digest('hex'), refs.image_sha256, 'the synthetic image must match the committed vector');
await init({ module_or_path: wasm });
assert.equal(inspect_image(image).photo_id, refs.photo_id);

function request(method, json) {
  return JSON.stringify(replay(recording, method, JSON.parse(json)));
}
const ledger = {
  tx: async params => request('tx', params),
  ledger_header: async params => request('ledger', params),
  account_tx: async params => request('account_tx', params),
  server_info: async params => request('server_info', params),
};
const options = { network_config: network, rpc_endpoints: [`recording:${recordingPath}`] };

for (const [mode, cliArgs] of [
  [{ mode: 'direct' }, []],
  [{ mode: 'manual', manifest_tx: refs.manifest_tx }, ['--manifest-tx', refs.manifest_tx]],
]) {
  const native = spawnSync(cli, [
    '--json', '--recording', recordingPath, '--network-config', networkPath, 'verify', imagePath, ...cliArgs,
  ], { cwd: root, encoding: 'utf8' });
  assert.equal(native.status, refs.expected_exit_code, native.error?.message ?? native.stderr);
  const cliReport = JSON.parse(native.stdout);
  const wasmReport = JSON.parse(await verify(image, { ...options, ...mode }, ledger));
  assert.equal(wasmReport.state, refs.expected_state);
  assert.equal(wasmReport.capture.manifest_tx, refs.manifest_tx);
  assert.equal(wasmReport.image.photo_id, refs.photo_id);
  assert.equal(wasmReport.checks.find(({ id }) => id === 'capture.rgb_binding').status, 'pass');
  // No elapsed-time fields exist in galaxy-report/v1. Compare even ledger and claimed times.
  assert.deepEqual(wasmReport, cliReport);
  console.log(`CLI/WASM ${mode.mode} parity: ${wasmReport.state}, full report JSON identical`);
}

const nodes = ['https://index.example'];
const lookup = { ...options, mode: 'lookup', nodes, node_lookup_consent: true };
const captures = [{ capture_id: refs.capture_id, manifest_tx: refs.manifest_tx, proof_tx: refs.proof_tx, ledger_index: 1012 }];
const page = (captures, next_cursor) => JSON.stringify({
  protocol: 'galaxy-node/v1', network: 'xrpl-testnet', network_id: 1, software_version: '0.1.0',
  photo_id: refs.photo_id, indexed_from: 1000, indexed_through: 1409,
  indexed_ranges: [{ from: 1000, through: 1409 }], incomplete: false, captures, next_cursor,
});
const cursor = Buffer.from('page:two').toString('base64url');
let cursors = [];
const node = {
  async lookup(endpoint, photoId, networkId, limit, after) {
    assert.equal(endpoint, nodes[0]);
    assert.equal(photoId, refs.photo_id);
    assert.equal(networkId, 1);
    assert.ok(limit > 0 && limit <= 100);
    cursors.push(after);
    return after === undefined ? page(captures, cursor) : page([], null);
  },
};
await assert.rejects(verify(image, { ...lookup, node_lookup_consent: false }, ledger, node), /requires consent/);
assert.deepEqual(cursors, []);
const found = JSON.parse(await verify(image, lookup, ledger, node));
assert.deepEqual(cursors, [undefined, cursor]);
assert.equal(found.state, refs.expected_state);
assert.equal(found.candidates[0].manifest_tx, refs.manifest_tx);
assert.deepEqual(found.trust.lookup_endpoints, nodes);
assert.ok(found.trust.ledger_ranges.some(({ purpose, min, max }) => purpose === 'lookup index (untrusted)' && min === 1000 && max === 1409));
const repeated = JSON.parse(await verify(image, lookup, ledger, { lookup: async () => page(captures, cursor) }));
assert.equal(repeated.state, 'UNAVAILABLE');
assert.deepEqual(repeated.candidates, []);
console.log('WASM node lookup: consent guard, multi-page coverage, candidate verification, repeated cursor rejected');
