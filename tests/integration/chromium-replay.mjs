// Verify private real-device exports in the actual browser UI using public RPC recordings.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { readFile, realpath } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { replay } from '../../apps/verifier-web/tests/recording.mjs';

assert.equal(process.argv.length, 5, 'usage: chromium-replay.mjs PRIVATE_SESSION_DIR VITE_DIST GALAXY_CLI');
const session = await realpath(process.argv[2]);
assert.ok(session.startsWith(`${await realpath('fixtures/real')}/`), 'expected ignored private session');
const [dist, cli] = process.argv.slice(3).map(path => resolve(path));
const config = join(session, 'network.toml');
const cases = new Map(await Promise.all(['png-v1', 'jpeg-v1'].map(async profile => {
  const work = (await readFile(join(session, `${profile}.attempt`), 'utf8')).trim();
  assert.match(work, /^attempt-[0-9]+$/);
  const file = join(session, work, profile === 'png-v1' ? 'development.png' : 'development.jpg');
  const record = join(session, `${profile}.recording.json`);
  return [profile, { file, record, exchanges: JSON.parse(await readFile(record, 'utf8')) }];
})));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm' };
const rpcCounts = new Map();
const issues = [];
const server = createServer(async (request, response) => {
  try {
    const { pathname } = new URL(request.url, 'http://127.0.0.1');
    const profile = /^\/xrpl\/(png-v1|jpeg-v1)\/$/.exec(pathname)?.[1];
    if (request.method === 'POST' && profile) {
      const body = await new Response(request).json();
      rpcCounts.set(profile, (rpcCounts.get(profile) ?? 0) + 1);
      const result = replay(cases.get(profile).exchanges, body.method, body.params[0]);
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ id: body.id, result }));
    } else if (request.method === 'GET' && (pathname === '/' || /^\/assets\/[\w.-]+$/.test(pathname))) {
      const file = pathname === '/' ? 'index.html' : pathname.slice(1);
      response.writeHead(200, { 'content-type': types[extname(file)] }).end(await readFile(join(dist, file)));
    } else {
      if (pathname !== '/favicon.ico') issues.push(`${request.method} unexpected request`);
      response.writeHead(404).end();
    }
  } catch {
    issues.push('replay or static asset response failed');
    response.writeHead(500).end();
  }
});
await new Promise(ready => server.listen(0, '127.0.0.1', ready));
const origin = `http://127.0.0.1:${server.address().port}`;
const driver = spawn(process.env.CHROMEDRIVER ?? 'chromedriver', ['--port=0'], { stdio: ['ignore', 'pipe', 'ignore'] });
let control, browser;
const request = async (path, data, method = data === undefined ? 'GET' : 'POST') => {
  const response = await fetch(`${control}${path}`, {
    method, headers: { 'content-type': 'application/json' },
    body: JSON.stringify(data), signal: AbortSignal.timeout(45_000),
  });
  const { value } = await response.json();
  if (!response.ok) throw Error(`Chromium WebDriver request ${path} failed: ${value.error}`);
  return value;
};
try {
  control = await new Promise((ready, reject) => {
    const deadline = setTimeout(() => reject(Error('Chromium WebDriver did not start')), 10_000);
    driver.once('error', reject);
    driver.once('exit', code => reject(Error(`Chromium WebDriver exited: ${code}`)));
    driver.stdout.setEncoding('utf8').on('data', text => {
      const match = /started successfully on port (\d+)/i.exec(text);
      if (match) {
        clearTimeout(deadline);
        ready(`http://127.0.0.1:${match[1]}`);
      }
    });
  });
  const opened = await request('/session', { capabilities: { alwaysMatch: {
    browserName: 'chrome', 'goog:chromeOptions': {
      binary: process.env.CHROMIUM ?? '/usr/bin/chromium',
      args: ['--headless', '--no-sandbox', '--disable-dev-shm-usage', '--disable-background-networking'],
    },
  } } });
  browser = `/session/${opened.sessionId}`;
  const wd = (path, data) => request(browser + path, data);
  const el = async value => (await wd('/element', { using: 'css selector', value }))['element-6066-11e4-a52e-4f735466cecf'];
  const click = async selector => wd(`/element/${await el(selector)}/click`, {});
  const readReport = async () => {
    for (let i = 0; i < 250; i++) {
      const text = await wd('/execute/sync', { script: 'return document.querySelector("#report pre")?.textContent ?? null', args: [] });
      if (text) return JSON.parse(text);
      await delay(40);
    }
    throw Error('Chromium verifier produced no report');
  };
  for (const [profile, { file, record }] of cases) {
    const native = spawnSync(cli, ['--network-config', config, '--recording', record, '--json', 'verify', '--no-discovery', file], { encoding: 'utf8' });
    assert.equal(native.status, 60);
    const expected = JSON.parse(native.stdout);
    await wd('/url', { url: `${origin}/` });
    await click('#settings-panel summary');
    const rpc = await el('#rpc-endpoints');
    await wd(`/element/${rpc}/clear`, {});
    await wd(`/element/${rpc}/value`, { text: `${origin}/xrpl/${profile}/` });
    await click('#settings-form button');
    await wd(`/element/${await el('#photo')}/value`, { text: file });
    const start = performance.now();
    await click('#verify');
    const actual = await readReport();
    const elapsed = Math.round(performance.now() - start);
    assert.ok(rpcCounts.get(profile) > 0 && issues.length === 0, 'browser must use only expected replay requests');
    for (const id of [
      'capture.photo_binding', 'capture.rgb_binding', 'registration.txn_chain',
      'capture.txn_chain', 'history.complete', 'proof.groth16',
    ]) {
      assert.equal(actual.checks.find(check => check.id === id)?.status, 'pass', id);
    }
    assert.equal(actual.state, 'UNSUPPORTED');
    // The transport provenance is intentionally different; compare every other field.
    assert.deepEqual(actual.trust.rpc_endpoints, [`${origin}/xrpl/${profile}/`]);
    actual.trust.rpc_endpoints = expected.trust.rpc_endpoints;
    assert.deepEqual(actual, expected);
    console.log(`development: ${profile} Chromium ${elapsed} ms, ${rpcCounts.get(profile)} RPCs, CLI report parity`);
  }
} finally {
  if (control && browser) await request(browser, undefined, 'DELETE').catch(() => {});
  driver.kill();
  server.close();
}
