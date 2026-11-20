import { createHash } from 'node:crypto'
import { decode, encode } from 'ripple-binary-codec'
import { decodeAccountID, Wallet } from 'xrpl'

export { decode }

export function transactionHash(blob) {
  return createHash('sha512')
    .update('TXN\0')
    .update(blob, 'hex')
    .digest('hex')
    .slice(0, 64)
    .toUpperCase()
}

export function accountSetVectors() {
  // Public Ed25519 fixture: https://github.com/XRPLF/xrpl.js/blob/xrpl%405.3.0/packages/ripple-address-codec/test/xrp-codec.test.ts
  // Use only for offline vectors. Never fund or submit from this account.
  const entropy = [76, 58, 29, 33, 63, 189, 251, 20, 199, 194, 141, 96, 148, 105, 179, 65]
  const wallet = Wallet.fromEntropy(entropy, { algorithm: 'ed25519' })
  const hex = (bytes) => Buffer.from(bytes).toString('hex').toUpperCase()
  const ascii = (text) => hex(Buffer.from(text, 'ascii'))
  const memoType = 'galaxy/test/codec'
  const cases = [
    { network_id: 1, sequence: 1, memo_data: '000102FF' },
    {
      network_id: 1025,
      sequence: 2,
      // Exercise the two-byte variable-length prefix and data that is not UTF-8.
      memo_data: hex(Buffer.from(Array.from({ length: 193 }, (_, i) => 255 - i))),
    },
  ].map(({ network_id, sequence, memo_data }) => {
    const unsigned = {
      TransactionType: 'AccountSet',
      Account: wallet.classicAddress,
      Sequence: sequence,
      Fee: '12',
      LastLedgerSequence: 50,
      ...(network_id <= 1024 ? {} : { NetworkID: network_id }),
      Memos: [{ Memo: {
        MemoType: ascii(memoType),
        MemoFormat: ascii('application/octet-stream'),
        MemoData: memo_data,
      } }],
    }
    const { tx_blob, hash } = wallet.sign(unsigned)
    if (hash !== transactionHash(tx_blob) || !wallet.verifyTransaction(tx_blob)) {
      throw new Error('xrpl.js did not produce a valid signed vector')
    }
    return { network_id, memo_data, unsigned, unsigned_blob: encode(unsigned), tx_blob, hash, tx: decode(tx_blob) }
  })
  return {
    entropy,
    // Family seed of the public fixture entropy above (not a secret).
    seed: wallet.seed,
    address: wallet.classicAddress,
    account_id: hex(decodeAccountID(wallet.classicAddress)),
    public_key: wallet.publicKey,
    memo_type: memoType,
    cases,
  }
}
