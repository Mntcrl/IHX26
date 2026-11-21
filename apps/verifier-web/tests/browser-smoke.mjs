// Run against a fixture-configured production build with the SHA-pinned synthetic photo.
// Requires Chromium and chromedriver; uses only real WebDriver and CDP input commands.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { replay } from './recording.mjs';

const [imagePath] = process.argv.slice(2).map(path => resolve(path));
assert.ok(imagePath, 'pass the synthetic photo produced by GALAXY_VECTOR_IMAGE_OUT');
const fixture = new URL('../../../test-vectors/verifier/dev-unattested/', import.meta.url);
const dist = new URL('../dist/', import.meta.url);
const [image, refs, exchanges] = await Promise.all([
  readFile(imagePath), readFile(new URL('refs.json', fixture), 'utf8').then(JSON.parse),
  readFile(new URL('recording.json', fixture), 'utf8').then(JSON.parse),
]);
assert.equal(createHash('sha256').update(image).digest('hex'), refs.image_sha256);

const tmp = await mkdtemp(join(tmpdir(), 'galaxy-browser-'));
const untagged = join(tmp, 'image-without-xmp.png');
let xmpOffset = 8;
while (xmpOffset < image.length && image.toString('ascii', xmpOffset + 4, xmpOffset + 8) !== 'iTXt') {
  xmpOffset += image.readUInt32BE(xmpOffset) + 12;
}
assert.ok(xmpOffset < image.length);
await writeFile(untagged, Buffer.concat([image.subarray(0, xmpOffset), image.subarray(xmpOffset + image.readUInt32BE(xmpOffset) + 12)]));

const nodeRequests = [];
const rpcMethods = [];
const unexpected = [];
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (url.pathname === '/xrpl/' && request.method === 'POST') {
      const body = await new Response(request).json();
      rpcMethods.push(body.method);
      const result = replay(exchanges, body.method, body.params[0]);
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ id: body.id, result }));
    } else if (url.pathname.startsWith('/node/v1/photos/') && request.method === 'GET') {
      nodeRequests.push(url);
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({
        protocol: 'galaxy-node/v1', network: 'xrpl-testnet', network_id: 1, software_version: '0.1.0', photo_id: refs.photo_id,
        indexed_from: 1000, indexed_through: 1409, indexed_ranges: [{ from: 1000, through: 1409 }],
        incomplete: false, next_cursor: null,
        captures: [{ capture_id: refs.capture_id, manifest_tx: refs.manifest_tx, proof_tx: refs.proof_tx, ledger_index: 1012 }],
      }));
    } else if (request.method === 'GET' && (url.pathname === '/verifier/' || /^\/verifier\/assets\/[\w.-]+$/.test(url.pathname))) {
      const path = url.pathname.slice('/verifier/'.length) || 'index.html';
      const types = { html: 'text/html', js: 'text/javascript', css: 'text/css', wasm: 'application/wasm' };
      response.writeHead(200, { 'content-type': types[path.split('.').pop()] }).end(await readFile(new URL(path, dist)));
    } else {
      unexpected.push(url.pathname);
      response.writeHead(404).end();
    }
  } catch (error) {
    unexpected.push(String(error));
    response.writeHead(500).end();
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const driver = spawn(process.env.CHROMEDRIVER ?? 'chromedriver', ['--port=0'], { stdio: ['ignore', 'pipe', 'inherit'] });
let control;
let session;
try {
  const { promise: started, resolve, reject } = Promise.withResolvers();
  const timeout = setTimeout(() => reject(new Error('ChromeDriver did not start')), 10_000);
  driver.on('error', reject);
  driver.on('exit', code => reject(new Error(`ChromeDriver exited: ${code}`)));
  driver.stdout.setEncoding('utf8');
  driver.stdout.on('data', text => {
    const match = /started successfully on port (\d+)/i.exec(text);
    if (match) resolve(`http://127.0.0.1:${match[1]}`);
  });
  control = await started.finally(() => clearTimeout(timeout));

  const command = async (path, data, method = data === undefined ? 'GET' : 'POST') => {
    const result = await fetch(`${control}${path}`, {
      method, headers: { 'content-type': 'application/json' }, body: JSON.stringify(data), signal: AbortSignal.timeout(30_000),
    });
    const { value } = await result.json();
    if (!result.ok) throw new Error(`${path}: ${value.message}`);
    return value;
  };
  const launched = await command('/session', { capabilities: { alwaysMatch: {
    browserName: 'chrome', 'goog:loggingPrefs': { browser: 'ALL' },
    'goog:chromeOptions': { binary: process.env.CHROMIUM ?? '/usr/bin/chromium', args: [
      '--headless', '--no-sandbox', '--disable-dev-shm-usage', '--disable-background-networking', '--window-size=900,950',
    ] },
  } } });
  session = `/session/${launched.sessionId}`;
  const wd = (path, data) => command(session + path, data);
  const element = async selector => (await wd('/element', { using: 'css selector', value: selector }))['element-6066-11e4-a52e-4f735466cecf'];
  const click = async selector => wd(`/element/${await element(selector)}/click`, {});
  const enabled = async selector => wd(`/element/${await element(selector)}/enabled`);
  const type = async (selector, text, clear = true) => {
    const id = await element(selector);
    if (clear) await wd(`/element/${id}/clear`, {});
    return wd(`/element/${id}/value`, { text });
  };
  const script = (code, ...args) => wd('/execute/sync', { script: code, args });
  const cdp = (cmd, params) => wd('/goog/cdp/execute', { cmd, params });
  const readReport = async () => {
    for (let attempt = 0; attempt < 200; attempt++) {
      const value = await script('return document.querySelector("#report pre")?.textContent ?? null');
      if (value) return JSON.parse(value);
      const status = await script('return document.querySelector("#status").textContent');
      if (/could not|no verdict/i.test(status)) throw new Error(status);
      await delay(100);
    }
    throw new Error('No report appeared in Chromium');
  };

  await wd('/url', { url: `${origin}/verifier/` });
  await click('#settings-panel summary');
  await type('#rpc-endpoints', `${origin}/xrpl/`);
  await type('#node-endpoints', `${origin}/node`);
  await click('#settings-form button');
  await click('input[value=lookup]');
  assert.equal(await script('return document.querySelector("#lookup-notice").textContent'), 'Photo ID lookup is disabled. It can be enabled in settings.');
  assert.equal(await enabled('#verify'), false);
  await click('input[value=direct]');
  const { x, y } = await script('const r = document.querySelector("#drop-zone").getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2}');
  const drag = { x, y, data: { items: [], files: [imagePath], dragOperationsMask: 1 } };
  for (const type of ['dragEnter', 'dragOver', 'drop']) await cdp('Input.dispatchDragEvent', { type, ...drag });
  assert.match(await script('return document.querySelector("#status").textContent'), /Selected locally/);
  await click('#verify');
  const direct = await readReport();
  assert.equal(direct.state, 'UNSUPPORTED');
  assert.equal(direct.image.photo_id, refs.photo_id);
  assert.equal(direct.capture.manifest_tx, refs.manifest_tx);
  assert.ok(direct.image.xmp);
  assert.equal(nodeRequests.length, 0);
  const checkLabels = await script('return [...document.querySelectorAll("#report [data-basis]")].map(x => x.textContent)');
  assert.ok(checkLabels.includes('Proven by circuit when passed') && checkLabels.includes('Committed only · not proven'));
  assert.ok(await script('return document.querySelector("#report a")?.href.includes(arguments[0])', refs.manifest_tx));
  console.log('Chromium direct (drag/drop): UNSUPPORTED, photo ID + XMP matched, explorer link and separate circuit/committed labels, zero node requests');

  await type('#photo', untagged, false);
  await click('input[value=manual]');
  await type('#manifest', refs.manifest_tx);
  await click('#verify');
  const manual = await readReport();
  assert.equal(manual.state, 'UNSUPPORTED');
  assert.equal(manual.mode, 'manual');
  assert.equal(manual.image.xmp, null);
  assert.equal(manual.capture.manifest_tx, refs.manifest_tx);
  assert.equal(nodeRequests.length, 0);
  console.log('Chromium manual (file picker, no XMP): UNSUPPORTED, manifest matched, zero node requests');

  await click('#settings-panel summary');
  await click('#discovery');
  await click('#settings-form button');
  await type('#photo', untagged, false);
  await click('input[value=lookup]');
  assert.equal(await enabled('#verify'), false);
  assert.equal(nodeRequests.length, 0);
  assert.match(await script('return document.querySelector("#lookup-notice").textContent'), /can correlate/);
  await click('#consent');
  await click('#verify');
  const lookup = await readReport();
  assert.equal(lookup.state, 'UNSUPPORTED');
  assert.equal(lookup.mode, 'lookup');
  assert.equal(lookup.candidates[0].manifest_tx, refs.manifest_tx);
  assert.equal(nodeRequests.length, 1);
  assert.equal(nodeRequests[0].pathname, `/node/v1/photos/${refs.photo_id}`);
  assert.equal(nodeRequests[0].searchParams.get('network_id'), '1');
  assert.equal(await enabled('#verify'), false);
  assert.ok(rpcMethods.includes('tx') && rpcMethods.includes('ledger') && rpcMethods.includes('account_tx'));
  assert.deepEqual(unexpected.filter(path => path !== '/favicon.ico'), []);
  const logs = await wd('/log', { type: 'browser' });
  assert.ok(logs.every(({ message }) => !/Content.Security.Policy|Uncaught/i.test(message)), JSON.stringify(logs));
  await cdp('Emulation.setDeviceMetricsOverride', { width: 390, height: 850, deviceScaleFactor: 1, mobile: false });
  const layout = await script(`return {
    width: document.documentElement.clientWidth, document: document.documentElement.scrollWidth,
    overflowing: [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth
      || (e.clientWidth && e.scrollWidth > e.clientWidth))
      .slice(0, 20).map(e => [e.tagName, e.id, e.className, e.clientWidth, e.scrollWidth, Math.round(e.getBoundingClientRect().right)]),
  }`);
  assert.ok(layout.document <= layout.width, `report should fit a 390px viewport: ${JSON.stringify(layout)}`);
  const blocked = await script('return fetch(arguments[0]).then(() => false, () => true)', `${origin}/not-allowlisted`);
  assert.ok(blocked && !unexpected.includes('/not-allowlisted'), 'CSP should block unconfigured same-origin connections');
  console.log('Chromium layout and CSP: report fits a 390px viewport; unconfigured same-origin fetch blocked');
  console.log(`Chromium discovery: explicit consent gate, one photo-ID GET, local candidate verification; ${rpcMethods.length} recorded RPC calls across all modes; no CSP or uncaught browser errors`);
} finally {
  if (control && session) await fetch(`${control}${session}`, { method: 'DELETE', signal: AbortSignal.timeout(5_000) }).catch(() => {});
  driver.kill();
  server.close();
  await rm(tmp, { recursive: true, force: true });
}
