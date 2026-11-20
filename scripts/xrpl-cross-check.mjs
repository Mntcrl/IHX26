#!/usr/bin/env node
import { writeFile } from 'node:fs/promises'
import { parseArgs } from 'node:util'
import { accountSetVectors, decode, transactionHash } from './xrpl-vectors/oracle.mjs'

const TESTNET = 'https://s.altnet.rippletest.net:51234/'
const SAMPLE_SIZE = 256
const MIN_TYPES = 12
const LEDGER_STRIDE = 719
const MAX_LEDGERS = 96
const PARALLEL_LEDGERS = 4
const ORACLE = { xrpl: '5.3.0', 'ripple-binary-codec': '2.11.0' }

async function rpc(method, params) {
  const description = `${method}${params.ledger_index ? ` ${params.ledger_index}` : ''}`
  try {
    const response = await fetch(TESTNET, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ method, params: [{ ...params, api_version: 2 }] }),
      redirect: 'error',
      signal: AbortSignal.timeout(20_000),
    })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    const { result } = await response.json()
    if (result?.status !== 'success') {
      throw new Error(result?.error_message ?? result?.error ?? 'unknown RPC error')
    }
    return result
  } catch (cause) {
    throw new Error(`Testnet ${description}: ${cause.message}`, { cause })
  }
}

async function captureLedger(ledger_index) {
  const result = await rpc('ledger', { ledger_index, transactions: true, expand: true, binary: true })
  if (result.validated !== true || result.ledger_index !== ledger_index || !Array.isArray(result.ledger?.transactions)) {
    throw new Error(`expected validated ledger ${ledger_index} with binary transactions`)
  }
  return result.ledger.transactions.map(({ hash, tx_blob, meta_blob }) => {
    if (typeof tx_blob !== 'string' || !/^(?:[0-9A-F]{2})+$/u.test(tx_blob) ||
        typeof meta_blob !== 'string' || !/^(?:[0-9A-F]{2})+$/u.test(meta_blob) ||
        hash !== transactionHash(tx_blob)) {
      throw new Error(`invalid binary pair for Testnet ledger ${ledger_index}, transaction ${hash}`)
    }
    try {
      // Decode the RPC's original binary, never its JSON or Rust's decoded output.
      const tx = decode(tx_blob)
      const meta = decode(meta_blob)
      if (!tx.TransactionType || !meta.TransactionResult || !Number.isInteger(meta.TransactionIndex)) {
        throw new Error('missing transaction type, result or index')
      }
      return { ledger_index, hash, tx_blob, meta_blob, tx, meta }
    } catch (cause) {
      throw new Error(`xrpl.js decode failed for Testnet ledger ${ledger_index}, transaction ${hash}: ${cause.message}`, { cause })
    }
  })
}

async function capture(startLedger) {
  const { info } = await rpc('server_info', {})
  if (info.network_id !== 1) throw new Error(`expected Testnet network ID 1; got ${info.network_id}`)
  const start = startLedger ?? info.validated_ledger?.seq
  if (!Number.isSafeInteger(start) || start < 1 || start > info.validated_ledger?.seq) {
    throw new Error('start ledger must be a positive validated ledger index')
  }
  const byType = new Map()
  let scanned = 0
  let count = 0
  for (; scanned < MAX_LEDGERS; scanned += PARALLEL_LEDGERS) {
    const indices = Array.from({ length: PARALLEL_LEDGERS }, (_, i) => start - (scanned + i) * LEDGER_STRIDE)
    const ledgers = await Promise.all(indices.map(captureLedger))
    for (const rows of ledgers) {
      for (const row of rows) {
        const type = row.tx.TransactionType
        if (!byType.has(type)) byType.set(type, [])
        byType.get(type).push(row)
        count += 1
      }
    }
    if (count >= SAMPLE_SIZE && byType.size >= MIN_TYPES) break
  }
  if (count < SAMPLE_SIZE || byType.size < MIN_TYPES) {
    throw new Error(`only ${count} transactions of ${byType.size} types after ${MAX_LEDGERS} Testnet ledgers`)
  }
  // Round-robin across types so a large Payment or OfferCreate ledger cannot dominate.
  const rows = []
  for (let i = 0; rows.length < SAMPLE_SIZE; i += 1) {
    for (const group of byType.values()) {
      if (i < group.length) rows.push(group[i])
      if (rows.length === SAMPLE_SIZE) break
    }
  }
  if (new Set(rows.map(({ hash }) => hash)).size !== rows.length) throw new Error('duplicate transaction hashes')
  return { network_id: 1, start_ledger: start, ledger_stride: LEDGER_STRIDE, oracle: ORACLE, rows }
}

function representatives(rows) {
  // Cheapest fixture per type, plus compact examples of interesting cross-type field shapes.
  const compact = [...rows].sort((a, b) => a.tx_blob.length + a.meta_blob.length - b.tx_blob.length - b.meta_blob.length)
  const byType = new Map()
  for (const row of compact) {
    if (!byType.has(row.tx.TransactionType)) byType.set(row.tx.TransactionType, row)
  }
  const shapeChecks = [
    ({ tx }) => tx.TransactionType === 'Payment' && typeof tx.Amount === 'string',
    ({ tx }) => tx.TransactionType === 'Payment' && typeof tx.Amount === 'object',
    ({ tx }) => tx.Paths !== undefined,
    ({ tx }) => tx.Memos !== undefined,
    ({ tx }) => tx.Signers !== undefined,
    ({ meta }) => meta.TransactionResult.startsWith('tec'),
  ]
  return [...new Set([...byType.values(), ...shapeChecks.map((check) => compact.find(check)).filter(Boolean)])]
    .sort((a, b) => a.tx.TransactionType.localeCompare(b.tx.TransactionType) || a.hash.localeCompare(b.hash))
}

const { values: options } = parseArgs({ options: {
  out: { type: 'string', default: '/tmp/galaxy-xrpl-testnet.json' },
  'start-ledger': { type: 'string' },
  fixtures: { type: 'boolean' },
} })
const start = options['start-ledger'] === undefined ? undefined : Number(options['start-ledger'])
const corpus = await capture(start)
await writeFile(options.out, `${JSON.stringify(corpus)}\n`)
const types = new Map()
for (const { tx } of corpus.rows) types.set(tx.TransactionType, (types.get(tx.TransactionType) ?? 0) + 1)
console.log(`${options.out}: ${corpus.rows.length} validated Testnet binary pairs; start ledger ${corpus.start_ledger}; ${types.size} types`)
console.log(Object.fromEntries([...types].sort()))
if (options.fixtures) {
  const selected = representatives(corpus.rows)
  const ledgerFixture = new URL('../test-vectors/xrpl/testnet.json', import.meta.url)
  const signingFixture = new URL('../test-vectors/xrpl/account-set.json', import.meta.url)
  await writeFile(ledgerFixture, `${JSON.stringify({ ...corpus, rows: selected }, null, 2)}\n`)
  await writeFile(signingFixture, `${JSON.stringify(accountSetVectors(), null, 2)}\n`)
  console.log(`permanent fixtures: ${selected.length} Testnet binary pairs and two offline AccountSet signatures`)
}
