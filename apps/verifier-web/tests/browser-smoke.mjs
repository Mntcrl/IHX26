// Run the recorded smoke, or use --live with private fixture paths and a screenshot directory.
// Requires Chromium and ChromeDriver; no browser dependencies.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { replay } from './recording.mjs';

const live = process.argv[2] === '--live';
const args = process.argv.slice(live ? 3 : 2).map(path => resolve(path));
assert.equal(args.length, live ? 5 : 1,
  'Pass the recorded PNG or --live <certified> <stripped> <ordinary> <changed> <screenshot directory>');
const [imagePath] = args;
const app = new URL('../', import.meta.url);
const fixture = new URL('../../../test-vectors/verifier/dev-unattested/', import.meta.url);
const tmp = await mkdtemp(join(tmpdir(), 'photon-browser-'));
const dist = live ? new URL('../dist/', import.meta.url) : new URL('dist/', `file://${tmp}/`);
const untagged = join(tmp, 'image-without-xmp.png');
let refs, exchanges;
if (!live) {
  const image = await readFile(imagePath);
  refs = JSON.parse(await readFile(new URL('refs.json', fixture), 'utf8'));
  exchanges = JSON.parse(await readFile(new URL('recording.json', fixture), 'utf8'));
  assert.equal(createHash('sha256').update(image).digest('hex'), refs.image_sha256);
  let xmpOffset = 8;
  while (xmpOffset < image.length && image.toString('ascii', xmpOffset + 4, xmpOffset + 8) !== 'iTXt') {
    xmpOffset += image.readUInt32BE(xmpOffset) + 12;
  }
  assert.ok(xmpOffset < image.length);
  await writeFile(untagged, Buffer.concat([image.subarray(0, xmpOffset), image.subarray(xmpOffset + image.readUInt32BE(xmpOffset) + 12)]));
}

const nodeRequests = [];
const rpcMethods = [];
const unexpected = [];
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (!live && url.pathname === '/xrpl/' && request.method === 'POST') {
      const body = await new Response(request).json();
      rpcMethods.push(body.method);
      const result = replay(exchanges, body.method, body.params[0]);
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ id: body.id, result }));
    } else if (!live && url.pathname.startsWith('/node/v1/photos/') && request.method === 'GET') {
      nodeRequests.push(url);
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({
        protocol: 'galaxy-node/v1', network: 'xrpl-testnet', network_id: 1, software_version: '0.1.0', photo_id: refs.photo_id,
        indexed_from: 1000, indexed_through: 1409, indexed_ranges: [{ from: 1000, through: 1409 }],
        incomplete: false, next_cursor: null,
        captures: [{ capture_id: refs.capture_id, manifest_tx: refs.manifest_tx, proof_tx: refs.proof_tx, ledger_index: 1012 }],
      }));
    } else if (request.method === 'GET' && (url.pathname === '/verifier/' || /^\/verifier\/assets\/[\w.-]+$/.test(url.pathname))) {
      const path = url.pathname.slice('/verifier/'.length) || 'index.html';
      const types = { html: 'text/html', js: 'text/javascript', css: 'text/css', png: 'image/png', webp: 'image/webp', wasm: 'application/wasm' };
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
  if (!live) {
    const network = join(tmp, 'network.toml');
    await writeFile(network, (await readFile(new URL('network.toml', fixture), 'utf8'))
      .replace(/^json_rpc = .*$/m, `json_rpc = "${origin}/xrpl/"`)
      .replace(/^websocket = .*$/m, `websocket = "${origin.replace('http:', 'ws:')}/xrpl/"`)
      .replace(/^# trusted_local_rippled = .*$/m, `trusted_local_rippled = "${origin}/xrpl/"`));
    const build = spawnSync('./node_modules/.bin/vite', ['build', '--emptyOutDir', '--outDir', fileURLToPath(dist)], {
      cwd: fileURLToPath(app), stdio: 'inherit',
      env: { ...process.env, GALAXY_WEB_NETWORK_CONFIG: network, PHOTON_WEB_LOOKUP_NODE: `${origin}/node` },
    });
    assert.equal(build.status, 0, 'could not build recorded verifier');
  }
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
    browserName: 'chrome', 'goog:loggingPrefs': { browser: 'ALL', performance: 'ALL' },
    'goog:chromeOptions': { binary: process.env.CHROMIUM ?? '/usr/bin/chromium', args: [
      '--headless', '--no-sandbox', '--disable-dev-shm-usage', '--disable-background-networking', '--window-size=900,950',
    ] },
  } } });
  session = `/session/${launched.sessionId}`;
  const wd = (path, data) => command(session + path, data);
  const element = async selector => (await wd('/element', { using: 'css selector', value: selector }))['element-6066-11e4-a52e-4f735466cecf'];
  const click = async selector => wd(`/element/${await element(selector)}/click`, {});
  const enabled = async selector => wd(`/element/${await element(selector)}/enabled`);
  const choose = async path => wd(`/element/${await element('#photo')}/value`, { text: path });
  const script = (code, ...args) => wd('/execute/sync', { script: code, args });
  const cdp = (cmd, params) => wd('/goog/cdp/execute', { cmd, params });
  await cdp('Page.addScriptToEvaluateOnNewDocument', { source: `
    Element.prototype.requestPointerLock = () => Promise.reject(new Error('Pointer lock disabled in smoke tests'));
    Element.prototype.setPointerCapture = Element.prototype.releasePointerCapture = () => {};
  ` });
  const pauseForPaint = () => script('return new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const dropZoneAboveFold = () => script(`const r = document.querySelector('#drop-zone').getBoundingClientRect();
    return r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth && r.height > 0`);
  const readReport = async () => {
    for (let attempt = 0; attempt < (live ? 1200 : 200); attempt++) {
      const value = await script('return document.querySelector("#report pre")?.textContent ?? null');
      if (value) return JSON.parse(value);
      const status = await script('return document.querySelector("#status").textContent');
      if (/could not|no verdict/i.test(status)) throw new Error(status);
      await delay(100);
    }
    throw new Error('No report appeared in Chromium');
  };

  const inspect = async (kind, name) => {
    for (let attempt = 0; attempt < 200; attempt++) {
      const loaded = await script(`const card = document.querySelector('#photo-card');
        return (!card.hidden && document.querySelector('#file-name').textContent === arguments[0])
          ? [document.querySelector('#detection').dataset.kind, document.querySelector('#photo-id').textContent] : null`, name.split('/').at(-1));
      if (loaded) {
        assert.equal(loaded[0], kind);
        assert.ok(/^[a-f0-9]{64}$/.test(loaded[1]), 'expected a locally displayed photo ID');
        return loaded[1];
      }
      await delay(35);
    }
    throw new Error('No local inspection appeared in Chromium');
  };
  const verify = async () => {
    await click('#verify');
    return readReport();
  };
  const remoteRequests = async () => (await wd('/log', { type: 'performance' })).flatMap(entry => {
    const { message: { method, params } } = JSON.parse(entry.message);
    const url = method === 'Network.requestWillBeSent' ? params.request.url
      : method === 'Network.webSocketCreated' ? params.url : null;
    return url && /^(https|wss):/.test(url) ? [new URL(url)] : [];
  });

  await wd('/url', { url: `${origin}/verifier/` });
  assert.ok(await script('return document.querySelector(".wordmark").decode().then(() => true)'), 'official wordmark loads');
  assert.equal(await enabled('#verify'), false);
  assert.ok(await dropZoneAboveFold(), 'desktop drop target is on the opening screen');
  assert.equal(await script('return document.querySelector("#sampling").dataset.scAct'), 'pin');
  for (const expected of ['a[href="#method"]', '#photo', '#advanced summary']) {
    await cdp('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await cdp('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    assert.ok(await script('return document.activeElement.matches(arguments[0])', expected), `tab order should reach ${expected}`);
  }
  const { x, y } = await script('const r = document.querySelector("#drop-zone").getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2}');
  const drag = { x, y, data: { items: [], files: [imagePath], dragOperationsMask: 1 } };
  const start = performance.now();
  for (const type of ['dragEnter', 'dragOver', 'drop']) await cdp('Input.dispatchDragEvent', { type, ...drag });
  const id = await inspect('embedded', imagePath);
  const inspected = performance.now() - start;
  assert.equal((await remoteRequests()).length, 0, 'inspection must not contact external services');
  assert.equal(rpcMethods.length + nodeRequests.length, 0, 'inspection must not contact recorded services');
  await cdp('Browser.grantPermissions', { origin, permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'] });
  await click('#copy');
  assert.ok(await script('return navigator.clipboard.readText().then(value => value === arguments[0])', id), 'photo ID copied');

  const screenshot = async name => {
    await delay(460); // Let the report entry animation finish before capture.
    const { data } = await cdp('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(args[4], `${name}.png`), Buffer.from(data, 'base64'));
  };
  if (live) await screenshot('embedded');
  const verifyStart = performance.now();
  const direct = await verify();
  console.log(`Chromium direct: ${direct.state}; local inspection ${(inspected / 1000).toFixed(3)} s, verification ${((performance.now() - verifyStart) / 1000).toFixed(3)} s`);
  assert.equal(direct.state, live ? 'VERIFIED' : 'UNSUPPORTED');
  assert.equal(direct.mode, 'direct');
  assert.ok(direct.image.photo_id === id && direct.image.xmp);
  assert.deepEqual(direct.trust.lookup_endpoints, []);
  assert.equal(nodeRequests.length, 0);
  assert.ok(!(await remoteRequests()).some(url => url.pathname.startsWith('/v1/photos/')), 'direct verification must not contact a lookup node');
  const groupLabels = await script('return [...document.querySelectorAll(".check-group summary")].map(x => x.firstChild.textContent)');
  assert.deepEqual(groupLabels, ['Image', 'Capture', 'Device attestation', 'Ledger', 'Proof', 'Policy']);
  const checkLabels = await script('return [...document.querySelectorAll("#report [data-basis]")].map(x => x.textContent)');
  assert.ok(checkLabels.includes('Proven by circuit when passed') && checkLabels.includes('Committed only · not proven'));
  assert.ok(await script('return [...document.querySelectorAll("#report a")].some(a => a.href.includes(arguments[0]))', direct.capture.manifest_tx));
  if (live) {
    assert.ok(await script('return document.querySelector(".headline").textContent.includes("Physical consistency verified under Galaxy policy galaxy-pc-v1m")'));
    assert.ok(await script('return document.querySelector(".metadata").textContent.includes("Development setup (Testnet)")'));
    await screenshot('verified');
    const mismatches = [];
    for (const [label, path, kind, state] of [
      ['stripped', args[1], 'regular', /^VERIFIED$/],
      ['ordinary', args[2], 'regular', /^NOT_FOUND$/],
      ['changed', args[3], 'warning', /^INVALID$/],
    ]) {
      const before = performance.now();
      await choose(path);
      const nextId = await inspect(kind, path);
      const local = performance.now() - before;
      if (label === 'stripped') assert.ok(nextId === id, 'stripping XMP changes no pixels or photo ID');
      if (label === 'changed') assert.ok(nextId !== id, 'changing one pixel changes the photo ID');
      assert.equal((await remoteRequests()).length, 0, 'inspection must not contact external services');
      if (kind === 'regular') {
        assert.ok(await script('return !document.querySelector("#lookup-options").hidden && document.querySelector("#discovery").checked'));
        assert.match(await script('return document.querySelector("#lookup-notice").textContent'), /^Photo ID is sent to .+ to find its capture\.$/);
      }
      const started = performance.now();
      const result = await verify();
      console.log(`Chromium ${label}: ${result.state}; local inspection ${(local / 1000).toFixed(3)} s, verification ${((performance.now() - started) / 1000).toFixed(3)} s`);
      assert.equal(result.mode, kind === 'regular' ? 'lookup' : 'direct');
      if (label === 'ordinary') assert.match(await script('return document.querySelector(".coverage").textContent'), /index coverage/i);
      if (label === 'changed') {
        assert.ok(result.checks.some(check => check.id === 'capture.photo_binding' && check.status === 'fail'));
      }
      await screenshot(label);
      if (!state.test(result.state)) mismatches.push(`${label}: expected ${state}, got ${result.state}`);
      await remoteRequests();
    }
    assert.deepEqual(mismatches, []);
  } else {
    assert.ok(id === refs.photo_id);
    await choose(untagged);
    await inspect('regular', untagged);
    await click('#advanced summary');
    await click('#manual');
    await wd(`/element/${await element('#manifest')}/value`, { text: refs.manifest_tx });
    const manual = await verify();
    assert.equal(manual.state, 'UNSUPPORTED');
    assert.equal(manual.mode, 'manual');
    assert.equal(manual.image.xmp, null);
    assert.equal(manual.capture.manifest_tx, refs.manifest_tx);
    assert.equal(nodeRequests.length, 0);
    await click('#manual');
    await click('#discovery');
    assert.equal((await verify()).state, 'NOT_FOUND');
    assert.equal(nodeRequests.length, 0);
    await click('#discovery');
    assert.match(await script('return document.querySelector("#lookup-notice").textContent'), /Photo ID is sent to 127\.0\.0\.1:/);
    const lookup = await verify();
    assert.equal(lookup.state, 'UNSUPPORTED');
    assert.equal(lookup.mode, 'lookup');
    assert.equal(lookup.candidates[0].manifest_tx, refs.manifest_tx);
    assert.equal(nodeRequests.length, 1);
    assert.equal(nodeRequests[0].pathname, `/node/v1/photos/${refs.photo_id}`);
    assert.equal(nodeRequests[0].searchParams.get('network_id'), '1');
    assert.ok(rpcMethods.includes('tx') && rpcMethods.includes('ledger') && rpcMethods.includes('account_tx'));
    console.log(`Chromium recorded: direct/manual, lookup opt-out, one default photo-ID GET; ${rpcMethods.length} recorded RPC calls`);
  }
  assert.deepEqual(unexpected.filter(path => path !== '/favicon.ico'), []);
  const logs = await wd('/log', { type: 'browser' });
  assert.ok(logs.every(({ message }) => !/Content.Security.Policy|Uncaught/i.test(message)), 'no CSP or uncaught browser errors');
  await cdp('Emulation.setDeviceMetricsOverride', { width: 390, height: 850, deviceScaleFactor: 1, mobile: false });
  await pauseForPaint();
  const layout = await script(`return {
    width: document.documentElement.clientWidth, document: document.documentElement.scrollWidth,
    overflowing: [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth
      || (e.clientWidth && e.scrollWidth > e.clientWidth))
      .slice(0, 20).map(e => [e.tagName, e.id, e.className, e.clientWidth, e.scrollWidth, Math.round(e.getBoundingClientRect().right)]),
  }`);
  assert.ok(layout.document <= layout.width, `report should fit a 390px viewport: ${JSON.stringify(layout)}`);
  const blocked = await script('return fetch(arguments[0]).then(() => false, () => true)', `${origin}/not-allowlisted`);
  assert.ok(blocked && !unexpected.includes('/not-allowlisted'), 'CSP should block unconfigured same-origin connections');
  if (live) await screenshot('mobile');
  await cdp('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  await wd('/url', { url: `${origin}/verifier/` });
  await pauseForPaint();
  assert.ok(await dropZoneAboveFold(), '390 px drop target is on the opening screen');
  const reduced = await script(`const stage = document.querySelector('#sampling');
    const tile = document.querySelector('#sample-tiles use');
    return { act: stage.dataset.scAct, final: getComputedStyle(tile).getPropertyValue('--sample-lift').trim() }`);
  assert.deepEqual(reduced, { act: 'flow', final: '1' }, 'reduced motion opens the complete diagram without pinning');
  await click('.return-to-input');
  assert.ok(await script('return document.activeElement.id === "photo"'), 'the method closes by returning focus to the photo input');
  console.log('Chromium: desktop and 390 px first-screen input, keyboard order, reduced motion, and CSP');
} finally {
  if (control && session) await fetch(`${control}${session}`, { method: 'DELETE', signal: AbortSignal.timeout(5_000) }).catch(() => {});
  driver.kill();
  server.close();
  await rm(tmp, { recursive: true, force: true });
}
