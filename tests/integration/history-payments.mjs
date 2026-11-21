// Populate account_tx with ordinary, third-party transactions on Testnet only.
import assert from 'node:assert/strict';
import { readFile, stat } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const { Wallet, isValidClassicAddress } = createRequire(
  new URL('../../scripts/xrpl-vectors/package.json', import.meta.url),
)('xrpl');

const [name, destination, anchor] = process.argv.slice(2);
assert.match(name, /^[A-Za-z0-9_-]{1,64}$/, 'expected a local Testnet wallet name');
assert.ok(isValidClassicAddress(destination), 'expected a Testnet registration address');
assert.match(anchor, /^[1-9][0-9]{0,9}$/, 'expected a validated PROOF ledger');
const proofLedger = Number(anchor);
const walletFile = join(homedir(), '.config', 'galaxy', 'wallets', `${name}.json`);
assert.equal((await stat(walletFile)).mode & 0o777, 0o600, 'wallet must be private');
let wallet;
try {
  wallet = Wallet.fromSeed(JSON.parse(await readFile(walletFile, 'utf8')).seed);
} catch {
  throw Error('could not load local Testnet wallet');
}
assert.notEqual(wallet.classicAddress, destination, 'payments must be third-party transactions');

const endpoint = 'https://s.altnet.rippletest.net:51234/';
async function request(method, params) {
  const response = await fetch(endpoint, {
    method: 'POST', redirect: 'error', signal: AbortSignal.timeout(20_000),
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ method, params: [{ api_version: 2, ...params }] }),
  });
  assert.ok(response.ok, `${method}: HTTP ${response.status}`);
  const { result } = await response.json();
  assert.ok(result, `${method}: missing result`);
  return result;
}
const server = await request('server_info', {});
assert.equal(server.info?.network_id, 1, 'not XRPL Testnet');

async function accountAt(account, ledgerIndex) {
  const result = await request('account_info', { account, ledger_index: ledgerIndex });
  assert.ok(result.validated && result.ledger_index === ledgerIndex, 'account_info must be validated');
  assert.equal(result.account_data.Account, account);
  return result.account_data;
}
async function head() {
  const result = await request('ledger', { ledger_index: 'validated' });
  assert.ok(result.validated && Number.isSafeInteger(result.ledger_index), 'missing validated ledger');
  return result.ledger_index;
}
const now = await head();
assert.ok(proofLedger <= now, 'PROOF ledger must already be validated');
const before = (await accountAt(destination, proofLedger)).AccountTxnID;
assert.match(before, /^(?!0{64}$)[A-F0-9]{64}$/, 'destination must be a prepared registration account');
assert.equal((await accountAt(destination, now)).AccountTxnID, before, 'registration account advanced since the PROOF');

// One more than a full 200-row verifier page. Read back validated payments
// before beginning so restarting the experiment never repeats an earlier batch.
const count = 201;
const confirmed = [];
if (now > proofLedger) {
  let marker;
  const markers = new Set();
  do {
    const result = await request('account_tx', {
      account: wallet.classicAddress, ledger_index_min: proofLedger + 1,
      ledger_index_max: now, forward: true, limit: 200, marker,
    });
    assert.ok(result.validated && result.ledger_index_min === proofLedger + 1 && result.ledger_index_max === now,
      'sender history is incomplete');
    for (const row of result.transactions) {
      const tx = row.tx_json;
      if (row.validated && row.meta.TransactionResult === 'tesSUCCESS'
          && tx.TransactionType === 'Payment' && tx.Account === wallet.classicAddress
          && tx.Destination === destination && tx.DeliverMax === '1') {
        confirmed.push({ hash: row.hash, ledger: row.ledger_index, fee_drops: Number(tx.Fee) });
      }
    }
    marker = result.marker;
    if (marker != null) {
      const key = JSON.stringify(marker);
      assert.ok(!markers.has(key), 'sender history pagination did not advance');
      markers.add(key);
    }
  } while (marker != null);
}
assert.ok(confirmed.length <= count, 'too many existing payments from the test wallet');
for (const payment of confirmed) console.log(JSON.stringify(payment));

const fee = await request('fee', {});
const feeDrops = Number(fee.drops?.open_ledger_fee);
assert.ok(Number.isSafeInteger(feeDrops) && feeDrops >= 1 && feeDrops <= 1000, 'Testnet fee outside limit');
let ledger = now;
let sequence = (await accountAt(wallet.classicAddress, ledger)).Sequence;
for (let i = confirmed.length; i < count; i++) {
  const signed = wallet.sign({
    TransactionType: 'Payment', Account: wallet.classicAddress, Destination: destination,
    Amount: '1', Fee: String(feeDrops), Sequence: sequence, LastLedgerSequence: ledger + 20,
  });
  const submitted = await request('submit', { tx_blob: signed.tx_blob });
  assert.ok(['tesSUCCESS', 'terQUEUED'].includes(submitted.engine_result),
    `payment ${i + 1}: ${submitted.engine_result ?? submitted.error}`);
  let result;
  for (let j = 0; j < 40; j++) {
    result = await request('tx', { transaction: signed.hash });
    if (result.validated) break;
    assert.ok(!result.error || result.error === 'txnNotFound', 'tx lookup failed');
    await delay(700);
  }
  assert.ok(result.validated, 'payment was not validated');
  assert.equal(result.hash, signed.hash);
  assert.equal(result.meta.TransactionResult, 'tesSUCCESS');
  sequence += 1;
  ledger = result.ledger_index;
  console.log(JSON.stringify({ hash: signed.hash, ledger, fee_drops: feeDrops }));
}
assert.equal((await accountAt(destination, ledger)).AccountTxnID, before,
  'registration AccountTxnID changed during third-party payments');
