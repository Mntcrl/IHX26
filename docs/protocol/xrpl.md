# XRPL layer (`galaxy-xrpl`)

Normative text: spec §10 (wire formats), §12 (XRPL conventions), §13 (LedgerSource), §14 (node needs), §22 (API). Decision record: [ADR 0050](../adr/0050-xrpl-client-and-submission.md). This document describes the implementation, its trust model and the Testnet measurements.

## Crate layout

| Module | Content | Targets |
|---|---|---|
| `codec` | Binary codec driven by the vendored `definitions.json` | all (wasm32, iOS, native) |
| `keys` | Ed25519 family seeds, account id, classic address, signing, tx hash | all |
| `transaction` | Galaxy AccountSet + one Memo, signing, strict Galaxy decode | all |
| `ledger` | `LedgerSource`, `RpcTransport`, sans-IO parsing of RPC results | all |
| `submission` | Reliable submission (`Submitter`, `TxStore`, `FileTxStore`), sans-IO helpers | all (`FileTxStore`: unix) |
| `registry` | Chunked bundle publishing and §10 retrieval | all |
| `mobile` | UniFFI-friendly, sans-IO signing/submission surface for iOS | all |
| `native` (feature `native`) | `RpcClient` (JSON-RPC over reqwest/rustls) and `Subscription` (WebSocket) | native |
| `dev_wallet` | Development wallets in `~/.config/galaxy/wallets/<name>.json` (0600), Testnet faucet | native |
| `bin/xrpl-tool` (feature `native`) | Command-line tool | native |

## Binary codec

- Field, type and code tables come from the unmodified `definitions.json` of `ripple-binary-codec` 2.11.0 (`crates/galaxy-xrpl/vendor/`, SHA-256 `dbd5c60a3b0fffa6c88a71414c5a0e8225192c90161886fe552d70f17e8cf2ff`, ISC licence alongside).
- Serialized types: STObject, STArray, Blob (VL), AccountID, Amount (XRP, IOU, MPT), Hash128/160/192/256, UInt8–UInt64 (UInt64 as hex or decimal string per definitions), Int32/Int64, Vector256, PathSet, Currency, Issue, XChainBridge, Number, plus the enumerated TransactionType / LedgerEntryType / TransactionResult names.
- Encode sorts fields canonically (type code, then field code). Decode is strict: no trailing bytes, bounded VL lengths and nesting depth, unknown field codes rejected.
- Cross-check against xrpl.js 5.3.0 / ripple-binary-codec 2.11.0 (exact pins in `scripts/xrpl-vectors/package.json`, oracle in `scripts/xrpl-vectors/oracle.mjs`, corpus generator `scripts/xrpl-cross-check.mjs`):
  - `test-vectors/xrpl/account-set.json`: Galaxy AccountSet+Memo vectors (seed → address, signed blob, hash) produced by xrpl.js; `tests/codec_crosscheck.rs::account_set_memo_signatures` reproduces them byte for byte.
  - `test-vectors/xrpl/testnet.json`: 15 representative real Testnet transactions (tx_blob + meta_blob + xrpl.js decode).
  - Full corpus: 256 real Testnet transactions with metadata (ledgers from 22480009, stride 719, 13 transaction types: Payment 91, OracleSet 91, OfferCreate 48, Batch 13, AccountSet 4, NFTokenAcceptOffer 2, TrustSet, TicketCreate, PaymentChannelClaim, NFTokenCreateOffer, MPTokenIssuanceCreate, CredentialAccept, AMMCreate). Our decode equals the xrpl.js decode field by field for tx and meta, re-encode reproduces the blobs, and recomputed hashes equal the ledger hashes. Run: `node scripts/xrpl-cross-check.mjs --out /tmp/galaxy-xrpl-testnet.json && XRPL_TESTNET_CORPUS=/tmp/galaxy-xrpl-testnet.json cargo test -p galaxy-xrpl --features native --test codec_crosscheck -- --ignored` (2026-11-20: `test captured_testnet_corpus ... ok`).

## Keys and signing

- Only ed25519 family seeds (`sEd…`, base58 ripple alphabet, version `01 E1 4B`). Private key = SHA-512Half(16-byte entropy); public key `ED ‖ pk`.
- Account id = RIPEMD-160(SHA-256(public key)); classic address = base58check with version `00`.
- Signing payload `STX\0 ‖ canonical tx without TxnSignature`; tx hash = SHA-512Half(`TXN\0 ‖ signed blob`).
- Seeds and private key material are held in `Zeroizing` buffers and redacted in `Debug`; nothing in the crate logs a seed. Development wallet files are created with mode 0600 in a 0700 directory and are never in the repository.

## Galaxy transactions

Galaxy AccountSets carry only common fields and exactly one Memo. The decoder
checks the Memo envelope and the Ed25519 master-key signature: the public key
must derive `Account`. Device-account publications also sign `AccountTxnID`:
the hash of the last successful transaction sent by that account, or zero
for the first device chunk after tracking is enabled. `AccountSet` with
`SetFlag: 5` and **no Memo** enables tracking; it is not a Galaxy record.
CIRCUIT records may omit `AccountTxnID`. A CAPTURE or DEVICE manifest without
a nonzero predecessor is invalid; a PROOF without one is not a candidate.
A valid REVOKE remains effective without the field.

MemoType namespaces: `galaxy/v1/*` for protocol records (spec §10); `galaxy/test/*` for every test and measurement transaction (`registry::Namespace::Test`). Readers of one namespace never consider records of the other.

## Trust model: what comes from the RPC

| Data | Source of truth |
|---|---|
| Transaction content (all fields, memo) | The blob must hash to the requested transaction ID. Galaxy records must also verify under the account's Ed25519 master key; fields in history and chunks cannot be substituted without invalidating the signature. |
| `validated`, `TransactionResult`, `TransactionIndex`, ledger index, ledger hash, close time, `complete_ledgers` | RPC-trusted (`proven_by: ledger-rpc`). A light client cannot check validator signatures or state proofs. |
| Network | `server_info.network_id` must equal the configured network before every operation, on each endpoint, over the same connection type used for the operation (HTTP preflight per request, WebSocket `server_info` before `subscribe`). Network id 0 (mainnet) is refused unless `allow_mainnet = true`. |

Remote endpoint (default, `s.altnet.rippletest.net`): a dishonest or compromised server can claim a transaction is validated or failed, hide transactions, or report wrong ledger metadata, but cannot alter transaction content. Local rippled (`trusted_local_rippled` in the network config, operator-run, loopback HTTP allowed): metadata and validation are as trustworthy as the operator's node. Reports state which endpoints were used and whether they are remote or local (`trust.rpc_trust`).

## LedgerSource and native client

`LedgerSource` (spec §13): `tx(hash)` (binary transaction and metadata, order,
result and validation status), `ledger_header(index)`, `account_root(account, index)`
(validated `AccountTxnID` via `account_info`), `account_tx(account, min, max, marker, limit)`
and `server_info()`. `LedgerTx::previous_account_txn_id` reads the RPC metadata
link for a transaction that did not sign one. `RpcLedger<T: RpcTransport>`
works with native, browser and recording transports. Native
`ledger_transactions` provides an indexed full validated ledger for node backfill.

`RpcClient` (feature `native`): HTTPS only except loopback rippled; no redirects
or proxies; connect timeout 5 s, request timeout 15 s; response caps 64 KiB
(preflight), 8 MiB (commands), 32 MiB (backfill). CLI and node verification use
`pinned_endpoints()` for whole-attempt failover; the browser uses an explicit
endpoint index per attempt. Reports label the endpoint used. Other RPC callers
may use per-request failover for reads; a `submit` is never retried on another
endpoint (an ambiguous write is resolved by the submission state machine).

`Subscription` (WebSocket, `wss://` only, message/frame cap 4 MiB): `server_info` network check, then `subscribe` to `ledger` + `transactions`. The first event after every (re)connection is `Connected { ledger_index, ledger_hash }` (the validated tip); notifications received before the subscribe acknowledgement are buffered and delivered after it. Events: `LedgerClosed` (index, hash, close time, transaction count) and `Transaction` notices (hash, ledger, TransactionIndex, result, untrusted Galaxy memo hint; only validated transactions). A disconnect, an idle stream (45 s), a malformed message or a network mismatch returns `Err`; the next `next_event` reconnects after 1 s, rotating endpoints, and the caller must backfill from its last durable checkpoint to the new tip (no events are synthesized or replayed across a gap). Live check (2026-11-20, `native::tests::testnet_ws_tip_matches_http_backfill`): the subscription tip and the next `ledgerClosed` (ledger 22482048, 26 transactions) matched HTTP backfills (hash, close time, transaction count; all 26 transaction notices present with equal TransactionIndex and result).

## Reliable submission

`Submitter<R: RpcTransport, S: TxStore>`, constructed with `network_id`, `allow_mainnet`, `max_fee_drops`:

1. `prepare(purpose, wallet, memo, deadline)`: if the store holds a record for `purpose`, return it unchanged after checking that it binds the same account, memo and deadline (it never re-signs). Otherwise autofill from validated data — `server_info` (network check, validated ledger *V*), `fee` (`open_ledger_fee`, refused above `max_fee_drops`), `account_info` at ledger *V* (Sequence) — set `LastLedgerSequence = min(V + 20, deadline)`, sign, and persist `{purpose, hash, blob, network_id, first_possible_ledger = V + 1, last_ledger}` with an atomic compare-and-swap **before** any submit.
   For device-account publications, validated `account_info` also supplies
   the predecessor (`AccountTxnID`); it is signed and persisted with the blob.
2. `submit_saved(pending)`: submits exactly the persisted bytes (verified against the store, fee cap rechecked), refuses once the validated ledger reached `last_ledger`. Engine results are provisional.
3. `run(pending, delay)`: look up the hash; if not final and still open, submit; poll; every `REBROADCAST_POLLS` (8) pending polls rebroadcast the identical blob (same hash and Sequence, so at most one copy can apply). Returns `Validated(tx)` or `Expired`.
4. `state(pending)`: `Validated` requires the returned blob to equal the persisted blob, `validated = true`, a ledger inside `first_possible_ledger..=last_ledger` and a `tes`/`tec` result. `Expired` requires both a validated ledger > `last_ledger` and a `tx` lookup bounded to `min_ledger..max_ledger` that reports `searched_all: true`; a bare `txnNotFound` is never treated as expiry.
5. `replace_expired(...)`: the only way to sign a different transaction for a purpose, and only after a proven `Expired`. CAPTURE callers decide whether to replace and pass the capture deadline.

Result classes: `tesSUCCESS` → Success; `tec*` → ClaimedFee (validated failure, fee consumed); `tef*` Failure; `tem*` Malformed; `tel*` Local; `ter*` Retry; anything else Unknown. Success = validated ∧ `tesSUCCESS`.

`FileTxStore` (unix): one JSON file per purpose (file name SHA-256(purpose)), per-purpose lock file, temp file + `fsync` + rename + directory `fsync`, entries ≤ 16 KiB, mode 0600 in a 0700 directory. Distinct purposes of one wallet must be submitted serially (one live Sequence at a time); `registry::publish_bundle_in` does this.

The same rules are exposed sans-IO (`autofill_from_responses`, `tx_lookup_params`, `evaluate_tx_lookup`, `evaluate_submit`, `PendingTx::new/verify`) for transports that cannot run the async state machine.

## iOS signing surface (`mobile`)

Owned types only (`String`, `Vec<u8>`, integers, records, fieldless enums, `Option`), no network I/O in Rust; the app sends the request bodies with URLSession and passes response bodies back:

`generate_family_seed() → sEd…`; `wallet_info(seed) → {classic_address, account_id, public_key}`; `server_info_request()` / `validated_ledger_from_server_info(network, body)`; `fee_request()`, `account_info_request(address, V)` → `autofill(network, V, deadline, address, fee_body, account_info_body) → AutofillFields`; `sign_galaxy_account_set(seed, network, purpose, fields, memo_type, memo_data) → SignedRecord {purpose, hash_hex, blob, network_id, first_possible_ledger, last_ledger}` (persist before submitting); `submit_request(network, record, V)` / `evaluate_submit_response`; `tx_request(network, record, V)` / `evaluate_tx_response → Pending | Expired | Validated{ledger_index, transaction_index, transaction_result, success}`. `NetworkParams {network_id, allow_mainnet, max_fee_drops}` is checked at every step; errors are `MobileError::{InvalidInput, Network, Rpc}`. The UniFFI bindings themselves live in `galaxy-ffi`.

## Registry (spec §10)

Publishing (`publish_bundle_in(submitter, wallet, namespace, kind, bytes, delay)`):
before DEVICE chunks, enable tracking once with a journaled, validated,
memo-free AccountSet if needed. Then publish the ≤ 1 MiB bundle in chunks of
≤ 800 payload bytes, strictly serial, each validated with `tesSUCCESS` before
the next. The manifest commits the first and last chunk ledgers. Stable
`registry/<network>/<account>/<kind>/<bundle_id>/…` journal purposes allow
resuming without re-signing.

Retrieval (`fetch_bundle_in(source, manifest_tx, network, allow_mainnet, namespace)`): manifest tx by hash (hash recomputed) → validated ∧ tesSUCCESS → manifest decode, network and (DEVICE) account / registration ledger checks → `complete_ledgers` must cover `first..last` (otherwise `HistoryUnavailable`) → `account_tx(account, first, last)` with all pages (marker loop bounded, repeated marker rejected) → records naming this bundle id are validated and collected. Rejections (each covered by `tests/registry.rs`):

| Condition | Result |
|---|---|
| Unvalidated, failed, mis-signed, third-party, or wrong-namespace row | Ignored and counted; a missing required chunk still rejects retrieval |
| Correctly signed chunk for this bundle outside `first..last` or at/after the manifest position | `Invalid` |
| `bundle_id` / `total` mismatch, `index ≥ total` | `Invalid` |
| Two different payloads for the same index (conflicting duplicate) | `Invalid`; identical duplicates are ignored |
| Gap (missing index) or first/last chunk ledger differs from the manifest | `Invalid` |
| Collected bytes exceed `bundle_len`; `bundle_len` > 1 MiB; inconsistent chunk bounds | Rejected |
| `ids::bundle_id` (domain-separated SHA-256) ≠ the manifest's `bundle_id` | `Invalid` |
| Bundle content ≠ manifest (device key/env/account/registration ledger; circuit id, policy id, vk digest) | `Invalid` |
| Manifest not found or chunk ledgers outside `complete_ledgers` | `HistoryUnavailable` |
| Network mismatch / mainnet without opt-in | `Network` |

## Measurements

### Serialized sizes of v1 records (offline, `xrpl-tool measure`)

Legacy pre-hardening baseline (no AccountTxnID), signed AccountSet with one
Memo. A device record carrying AccountTxnID adds 33 B to the blob but does
not change the Memos field size.

| Record | MemoData (B) | Memos field (B) | Signed blob (B) |
|---|---:|---:|---:|
| CAPTURE, 600-byte assertion budget (299 + 2 + 600) | 901 | 953 | 1099 |
| CAPTURE, ~450-byte assertion (299 + 2 + 450) | 751 | 803 | 949 |
| CAPTURE, real App Attest assertion size 142 B (299 + 2 + 142) | 443 | 495 | 641 |
| CAPTURE, dev-unattested (299 + 2 + 65) | 366 | 418 | 564 |
| PROOF (361) | 361 | 411 | 557 |

Overhead: blob = MemoData + 196 B for `galaxy/v1/capture` (17-byte MemoType), + 194 B for `galaxy/v1/proof`; the measured `galaxy/test/measure` transactions (19-byte MemoType) carry + 199/200 B (e.g. 751 B MemoData → 951 B blob on Testnet vs 949 B offline with the 2-byte-shorter type).

### Testnet fee and latency run (2026-11-20, wallet `rPCCnEjNBPizcDL11LRzhGmzfEmiXxwhNR`, run ID `xrpl4a`)

All with MemoType `galaxy/test/measure`, fee from `fee.open_ledger_fee`, cap 1000 drops. Latency = first `submit` call to the poll that observed validation (poll interval 0.9 s; one `server_info` call for the validated head and one `tx` lookup with its own `server_info` preflight, three HTTPS requests per poll).

| # | Tx hash | Ledger | MemoData | Memos | Blob | Fee (drops) | Submit → validated (ms) |
|---:|---|---:|---:|---:|---:|---:|---:|
| 0 | `029369EB3A58D76CA0ACDEDF854E6E23D026C34B4C23E9108CA5FC5BA975D11A` | 22481957 | 1 | 54 | 200 | 10 | 5837 |
| 1 | `0623EFD064F6A84080A853B8AC0B6A7D8A7BB57A42F685415F06302A41906C2D` | 22481959 | 32 | 85 | 231 | 10 | 4293 |
| 2 | `A0D80082DF99F20C34C26A20BF83F10C9491327338B7062F2C6475A0CA3518C5` | 22481961 | 65 | 118 | 264 | 10 | 6445 |
| 3 | `07346148528BD8DA208A5C09FD507611ECF031AE54F5C4E6DFA40D76B8A31A24` | 22481964 | 191 | 244 | 390 | 10 | 9716 |
| 4 | `ED2ABAF90C07BD31542C9D553BF180E023FD2E223998DC210F516E94725DFD9B` | 22481967 | 192 | 245 | 391 | 10 | 4859 |
| 5 | `ED0685100D06658F1CE0059D52863776F8384E185ED1E4873315E92A9E66B652` | 22481969 | 193 | 247 | 393 | 10 | 6241 |
| 6 | `E247F90F8A03C4B7D9321A77E11FED178CD0B15694F7FC4E0B474EA51DAB1D04` | 22481972 | 361 | 415 | 561 | 10 | 10438 |
| 7 | `C6E34F8DD386DAEB01A540296A5ED5963EC30B6C875449791413F3CD58ADAFD7` | 22481975 | 751 | 805 | 951 | 10 | 6996 |
| 8 | `8F2A44E3419F7A695D7E4FC4D6F399D18B846708AD553990473DBCD6C016DC14` | 22481980 | 839 | 893 | 1039 | 10 | 9496 |
| 9 | `1F89157AA765AF1D738715B0F479D119000880EF115765939440D65CF8A4BC0C` | 22481983 | 901 | 955 | 1101 | 10 | 6644 |

Latency over the 10 transactions: min 4293 ms, median 6545 ms, mean 7097 ms, max 10438 ms. Fee: 10 drops (base fee) for every size up to 1101 B; the Testnet open-ledger fee was never above the base fee during the run. MemoData lengths 192/193 cross the VL one-byte/two-byte length boundary (Memos grows by 2 B instead of 1 B).

Repeat run `xrpl5-recovery-1795214301`: 10/10 validated at 10 drops each with the same byte counts. Submit-to-validated latency: min 4125 ms, median 6285 ms, mean 6491 ms, max 8965 ms. Duplicate resubmission returned `tefPAST_SEQ`; the expired test blob returned `tefMAX_LEDGER` after the complete search, and the failed Payment validated with `tecNO_DST_INSUF_XRP` (not success).

Testnet submission scenarios:

- **Interrupted after sending.** The earlier version of `measure --interrupt` prepared, persisted and submitted `552747B14E16B78C1D579FB6FA10288141778BDD3C6B22CE1FBB8F05D0330EE8` (engine `tesSUCCESS`, LastLedgerSequence 22481973) and exited without polling. The next `measure --live` reloaded the journal and found the original hash validated in ledger 22481955 with `tesSUCCESS`; nothing was re-signed.
- **Interrupted before sending.** The current `measure --interrupt` exited after persisting `CE7C2D4EEBEAE5DF5A853897856BEC0CCF9F4956CAFC9DFFD310B5C98433D086` (LastLedgerSequence 22483541), without calling `submit`. A second process (`measure --live` with the same run ID) reported `pending_at_restart: true`, broadcast the same persisted blob, and observed `tesSUCCESS` in ledger 22483524. The validated blob matched the journal byte for byte.
- **Duplicate submission.** Transaction #0 was submitted twice; the second submit of the same blob returned `tefPAST_SEQ` (the Sequence was already consumed by the first copy), and the transaction was validated once.
- **LastLedgerSequence expiry.** `D2265375A5826B5B2F9144454E878A8F4B76F17D3FD8DDEE31D38BAED95800A7` was signed and persisted with deadline = validated + 3 (LastLedgerSequence 22481986) and not submitted. `poll` reported `Expired` only after the validated ledger passed 22481986 and the bounded `tx` lookup returned `searched_all: true`. Submitting the old blob afterwards (deliberate bypass of the guard, test only) returned `tefMAX_LEDGER`.
- **tec result.** A 1-drop Payment to a fresh unfunded address, `1A7C578BA06F5B71D5DC4D26C7C839ADB3B649DFC7436135ECE1795F3A12F54A`, validated in ledger 22481991 with `tecNO_DST_INSUF_XRP`, classified `ClaimedFee`, `success = false`.

### Registry round trip on Testnet (2026-11-20, `tests/testnet_registry.rs`, `galaxy/test/*` MemoTypes)

`GALAXY_XRPL_TESTNET_WALLET=<name> cargo test -p galaxy-xrpl --features native --test testnet_registry -- --ignored --nocapture` → `1 passed` (176.98 s including negative cases).

- Test CIRCUIT bundle (random 2000-byte test payload in the VK field, not a usable verifying key), 2099 B → 3 chunks: `C6A47734807DBD92F6795DCF3ABE80DEE73CC4E11B7F69E35977AC4840CA1B22`, `C64105DBD2DCC832640F848BD6A46E1B9D370F3BF871189D9761D9C43FFAA90F`, `5F08AED37B2318E187DB0D5B1B8BB9E15B058E2D500297D7723E5F40924FD115`; manifest `9FE23AEDFFC66FE959EDCAE7AF07A3051F0D0A9537753851D106558E339AA747` (ledger 22483525). Publish time 39.36 s.
- Development-unattested DEVICE bundle (fresh ed25519 key, validated registration ledger 22483527, random nonce), 206 B → 1 chunk `BCEE231AAF73AD30C40124ADB85417F0E5D79CA7204E3F927C7589F39785B85D`; manifest `976090AF7D58C6D6A031FDE4729DE830FF78057C09610DC4094B1CD5349E4E6B` (ledger 22483533). Publish time 20.89 s.
- Both bundles were fetched from their manifest hashes and matched the original bytes. Neither was accepted under the `galaxy/v1/*` namespace.

The retriever rejected these validated `galaxy/test/*` manifests:

| Condition | Manifest tx hash |
|---|---|
| Conflicting duplicate | `BED5FFCA2F48EB415F3232AEF054C411A018F6627AD1F25588B9295D40A6EA34` |
| Chunk index ≥ total | `6538969EA402E41E9FD41A2954E36FD8C599AC3771E3E020EA301775E9D59B94` |
| Bundle digest mismatch | `A5666EA29B03735CDDA15022E2DE27E78A3F9B7814117C3593B222E2D4C33538` |
| Failed chunk transaction (`tecNO_DST_INSUF_XRP`) | `FF3C975CDEF0DFB95E92018AB11333F9950FAA3EFC6776B5867D503010459D09` |
| Missing chunk | `6E866396E3A29EC473416857A7718E23394664C41C43B292D175239E21E5762F` |
| Wrong bundle ID | `E6B632E4D66798F4A96359EEF5395DDAE7EB8AFE4D5CD02FC83A6F211C146BBD` |
| Oversize bundle | `76D71C31EC98A56A03B37D50AC4E7D3AF946BAF9C8CB52EE8A1CA060025D7A6B` |

An honest validated `account_tx` cannot return an unvalidated row or an AccountSet from a different account. The test exercises those two checks by altering the first CIRCUIT chunk row of an otherwise live Testnet response; both were rejected without submitting an altered row.

Explorer: `https://testnet.xrpl.org/transactions/<hash>`.

### Testnet account-history tests (2026-11-21)

Fresh faucet accounts A `rN8crTvZHvpFpv6g3ZDUsyrKPQDHHFkTPB` and B
`rnJ23Q2mVqjTtvNbZRhDNHDKVk1aZJHmmM`; memos under `galaxy/test/deposit-auth/*`.
The AccountRoot column matches `ModifiedNode.LedgerIndex`, **not**
`FinalFields.Account` (thread-only nodes lack `FinalFields`).

| Transaction | Hash | Ledger | TransactionResult | A's AccountRoot ModifiedNode | Listed by A's `account_tx` |
|---|---|---:|---|---|---|
| A: AccountSet `SetFlag: 9` + Memo | `A08DF8F02C840C3D96DF97846BFDEF60B043C37B26A2D18C29E86443E925BF15` | 22486054 | `tesSUCCESS` | yes | yes |
| B: Payment to A | `E91B2E954F25028BC39EC720F398942087666E4F48503ED1F4F5C7483FCA0BCF` | 22486056 | `tecNO_PERMISSION` | no | no |
| B: TrustSet with A as issuer | `4095843B4A470FE02C51F96F5D30E365A30F3574BCF9F5DE6CA132BB8420439E` | 22486058 | `tesSUCCESS` | yes (thread only) | yes |
| B: EscrowCreate to A | `FD5B8E6D3C5AE66F3AC31D40A4A6B94468B200DC7B787A48808528868C40A732` | 22486060 | `tesSUCCESS` | yes (thread only) | yes |
| B: SetRegularKey naming A | `47E382E3FFC165BC2EB3282CE7FFFC70E83C90FA6BF099E019B612C6641C64E7` | 22486062 | `tesSUCCESS` | no | yes |

At validated ledger 22486062, A's flags were `0x01000000` (`lsfDepositAuth`)
and `PreviousTxnID` walked from B's EscrowCreate to B's TrustSet, A's AccountSet,
and finally the faucet Payment `D14412E50CCBA8F1BDEA030AC629A151B573FC47C977D6099C54139F77C4C448`
(created A in ledger 22486052). This disproves owner-only `PreviousTxnID`
threading even with DepositAuth enabled.

Separate fresh faucet accounts A `raasnzjwGpGRXgHmuMPqnHYPWr9uERg9Fn` and B
`rEG7YM9F2rZcwvqTrmo5xVYsq4CDNXGeF`; memos under
`galaxy/test/account-txn-id/*` except the memo-free preparation transaction.

| Transaction | Hash | Ledger | Result | A's `AccountTxnID` after |
|---|---|---:|---|---|
| A: AccountSet `SetFlag: 5` | `2AAA0576BF841B1E6BF21EA368A080815024A7D910D89ECEBB8670BB62B69EF2` | 22486124 | `tesSUCCESS` | all-zero sentinel |
| A: first chained AccountSet | `F7EBDC587AA2C34E2C904479852996BBAA4EA69A017EFF10926E041EDDED9FBA` | 22486126 | `tesSUCCESS` | first AccountSet hash |
| A: second chained AccountSet | `81E36C9EC7CB4A402C00F73A5734719AD76A4784CB9C7A2B6025A8AD7BA146B5` | 22486128 | `tesSUCCESS` | second AccountSet hash |
| A: AccountSet with wrong prior | `C3031E1476C9516975B2AE5BB7732027FB26EC6951269755EC6844D810FD602A` | — | submit `tefWRONG_PRIOR` | unchanged |
| B: Payment to A | `4C529A59C0D0D4B172DD528E09142BBD070A35548FCA6BEDCA686EA8E146D1D6` | 22486130 | `tesSUCCESS` | unchanged |
| B: TrustSet with A as issuer | `198E852A7555FDBA11DC37B307551DBE7EF1AAF64500AB3784B63E48B22FCD53` | 22486132 | `tesSUCCESS` | unchanged |
| B: EscrowCreate to A | `A6477BDE4D6BC5DEEE14391C56A0AC2A009FDF5DD0EF54BC1EC1B9D872AF170A` | 22486134 | `tesSUCCESS` | unchanged |
| B: SetRegularKey naming A | `78657BF798211B4C862F34245D41196D3CE75E7A650E93951D6078DA5DDA0001` | 22486136 | `tesSUCCESS` | unchanged |
| A: 1-drop Payment to an unfunded destination | `048976BCCE8E8F2E64BE663D181C7D018FD94699C7F4144F94E53B362C300952` | 22486138 | `tecNO_DST_INSUF_XRP` | unchanged |

The failed own Payment advanced `PreviousTxnID`, not `AccountTxnID`. The
first successful transaction after setup explicitly signed the all-zero value;
the second signed the first transaction's hash. The verifier walks this
separate own-success chain, not `PreviousTxnID`.

The hardened registry test passed in 149.63 s. In its round trip, wallet
`rH8rVRgV8tuygJQ3v2h2uPyBwsngi2xpmP` first validated the memo-free SetFlag 5
`38C9311729CCBFB7318D8C3371B989932D52C2C345D2F3C0095D0E8298A7D2CE` (22489326),
then the zero-predecessor device chunk
`092A7A4A98001D9C89DBAA16512086CEEBDD6B690948492260F1187FA8C8DA68` (22489328),
then the linked `galaxy/test/device` manifest
`3A5DA53149EF3EA4E31ADF9E2791EA2D57E4EBF1457D1EC1FB33D23124BC37F3` (22489330).
The AccountRoot at 22489330 named that manifest as its `AccountTxnID`.

## xrpl-tool

`cargo run -p galaxy-xrpl --features native --bin xrpl-tool -- [--config deployments/networks/xrpl-testnet.toml] [--wallet <name>] <command>`

| Command | Effect |
|---|---|
| `wallet new` | New ed25519 family seed in `~/.config/galaxy/wallets/<name>.json` (0600); prints the address only |
| `wallet show-address` / `wallet balance` | Address / validated balance in drops |
| `wallet fund` | Testnet faucet request (refused on other networks) |
| `tx inspect <hash>` | Hash-checked transaction + metadata, decoded |
| `publish-bundle <device\|circuit> <file> --namespace <v1\|test>` | Chunk txs then manifest tx (resumable journal in `~/.config/galaxy/tx-store/<wallet>/`) |
| `fetch-bundle <manifest-tx> [-o file] [--namespace v1\|test]` | §10 retrieval |
| `measure` | Offline sizes of v1 records |
| `measure --interrupt --run-id <id>` then `measure --live --run-id <id>` | Persist a signed transaction without submitting; the next process loads and broadcasts that blob, then runs the latency/fee experiments. Testnet and `galaxy/test/*` only. |

## Tests

- Offline: `cargo test -p galaxy-xrpl --features native` (codec vectors, keys, Galaxy decode, submission state machine with crash recovery and rebroadcast, registry rejection rules, HTTP client caps/redirects/endpoint checks, WebSocket preflight/buffering/reconnect against local servers, mobile surface).
- Network (`--ignored`): `codec_crosscheck::captured_testnet_corpus` (needs `XRPL_TESTNET_CORPUS`), `native::tests::testnet_ws_tip_matches_http_backfill`, `testnet_registry` (needs `GALAXY_XRPL_TESTNET_WALLET`).
- Portability: `cargo clippy --offline -p galaxy-xrpl --all-targets --all-features -- -D warnings` and Rust builds for `wasm32-unknown-unknown` and `aarch64-apple-ios` passed. The iOS build verifies Rust cross-compilation, not Swift linkage or a device run.
