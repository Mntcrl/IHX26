//! XRPL reads over a platform-independent transport. Transaction hashes bind the
//! signed blobs; metadata, validation status, and history coverage are RPC claims.

use std::{collections::HashSet, future::Future};

use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{codec, keys, AccountId, Error, Hash32};

const RIPPLE_EPOCH: u64 = 946_684_800;
const MAX_BLOB_BYTES: usize = 1024 * 1024;
const API_VERSION: u32 = 2;

/// A ledger operation can move between native executor threads. Browser
/// transports may instead await a non-Send JavaScript promise.
#[cfg(not(target_arch = "wasm32"))]
pub trait LedgerFuture<T>: Future<Output = Result<T, Error>> + Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T, F: Future<Output = Result<T, Error>> + Send> LedgerFuture<T> for F {}

/// A ledger operation on the browser's single-threaded executor.
#[cfg(target_arch = "wasm32")]
pub trait LedgerFuture<T>: Future<Output = Result<T, Error>> {}
#[cfg(target_arch = "wasm32")]
impl<T, F: Future<Output = Result<T, Error>>> LedgerFuture<T> for F {}

/// `params` is the command's parameter object. Return the bare XRPL `result`,
/// including command errors like `txnNotFound`. Transport failures are `Err`.
#[cfg(target_arch = "wasm32")]
pub trait RpcTransport {
    fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, Error>>;
}

/// `params` is the command's parameter object. Return the bare XRPL `result`,
/// including command errors like `txnNotFound`. Transport failures are `Err`.
#[cfg(not(target_arch = "wasm32"))]
pub trait RpcTransport: Sync {
    fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, Error>> + Send;
}

pub trait LedgerSource {
    fn tx(&self, hash: &Hash32) -> impl LedgerFuture<Option<LedgerTx>>;
    fn ledger_header(&self, index: u32) -> impl LedgerFuture<LedgerHeader>;
    fn account_tx(
        &self,
        account: &AccountId,
        min: u32,
        max: u32,
        marker: Option<Value>,
        limit: u16,
    ) -> impl LedgerFuture<AccountTxPage>;
    fn server_info(&self) -> impl LedgerFuture<ServerInfo>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerTx {
    pub hash: Hash32,
    pub tx_blob: Vec<u8>,
    pub meta_blob: Vec<u8>,
    pub transaction_result: String,
    pub transaction_index: u32,
    pub ledger_index: u32,
    pub validated: bool,
    /// Unix seconds, if the endpoint supplied the transaction's ledger close time.
    pub close_time: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerHeader {
    pub ledger_index: u32,
    pub hash: Hash32,
    pub parent_hash: Hash32,
    /// Unix seconds.
    pub close_time: u64,
    pub validated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountTxPage {
    pub transactions: Vec<LedgerTx>,
    /// Opaque XRPL marker. Pass it unchanged with the same account and bounds.
    pub marker: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerInfo {
    pub network_id: u32,
    pub complete_ledgers: String,
    pub validated_ledger: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerBackfill {
    pub header: LedgerHeader,
    /// All transactions in the validated ledger, sorted by TransactionIndex.
    /// Includes failed transactions; callers must check TransactionResult.
    pub transactions: Vec<LedgerTx>,
}

/// Adapt a platform-specific RPC callback to the typed ledger reads. The
/// transport must enforce the caller's network policy before sending commands.
pub struct RpcLedger<T> {
    transport: T,
}

impl<T: RpcTransport> RpcLedger<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Fetch a whole validated ledger for checkpointed backfill. Do not advance
    /// the checkpoint without matching `header.parent_hash` to its predecessor.
    pub async fn ledger_transactions(&self, index: u32) -> Result<LedgerBackfill, Error> {
        ledger_transactions(&self.transport, index).await
    }
}

impl<T: RpcTransport> LedgerSource for RpcLedger<T> {
    async fn tx(&self, hash: &Hash32) -> Result<Option<LedgerTx>, Error> {
        tx(&self.transport, hash).await
    }

    async fn ledger_header(&self, index: u32) -> Result<LedgerHeader, Error> {
        ledger_header(&self.transport, index).await
    }

    async fn account_tx(
        &self,
        account: &AccountId,
        min: u32,
        max: u32,
        marker: Option<Value>,
        limit: u16,
    ) -> Result<AccountTxPage, Error> {
        account_tx(&self.transport, account, min, max, marker, limit).await
    }

    async fn server_info(&self) -> Result<ServerInfo, Error> {
        server_info(&self.transport).await
    }
}

pub(crate) async fn tx(rpc: &impl RpcTransport, hash: &Hash32) -> Result<Option<LedgerTx>, Error> {
    let result = rpc
        .request(
            "tx",
            json!({"api_version": API_VERSION, "transaction": hex::encode_upper(hash), "binary": true}),
        )
        .await?;
    if result.get("error").and_then(Value::as_str) == Some("txnNotFound") {
        return Ok(None);
    }
    let record = parse_tx(&result)?;
    if record.hash != *hash {
        return Err(Error::Invalid("transaction hash does not match request"));
    }
    Ok(Some(record))
}

pub(crate) async fn ledger_header(
    rpc: &impl RpcTransport,
    index: u32,
) -> Result<LedgerHeader, Error> {
    let result = rpc
        .request(
            "ledger",
            json!({"api_version": API_VERSION, "ledger_index": index, "transactions": false}),
        )
        .await?;
    parse_ledger_header(&result, index).map(|(header, _)| header)
}

pub(crate) async fn account_tx(
    rpc: &impl RpcTransport,
    account: &AccountId,
    min: u32,
    max: u32,
    marker: Option<Value>,
    limit: u16,
) -> Result<AccountTxPage, Error> {
    if min == 0 || max < min || limit == 0 {
        return Err(Error::Invalid("invalid account_tx range or limit"));
    }
    let address = keys::classic_address(account);
    let mut params = json!({
        "api_version": API_VERSION, "account": address, "ledger_index_min": min,
        "ledger_index_max": max, "binary": true, "forward": true, "limit": limit,
    });
    if let Some(marker) = marker.as_ref() {
        params["marker"] = marker.clone();
    }
    let result = rpc.request("account_tx", params).await?;
    check_result(&result)?;
    if result.get("account").and_then(Value::as_str) != Some(address.as_str())
        || u32_field(&result, "ledger_index_min")? != min
        || u32_field(&result, "ledger_index_max")? != max
        || !validated(&result)?
    {
        return Err(Error::Invalid("account_tx did not cover the requested account and range"));
    }
    let next = result.get("marker").filter(|value| !value.is_null());
    if next.is_some() && next == marker.as_ref() {
        return Err(Error::Invalid("account_tx marker did not advance"));
    }
    let entries = transaction_array(&result)?;
    let mut transactions = Vec::with_capacity(entries.len());
    for entry in entries {
        let ledger_index = u32_field(entry, "ledger_index")?;
        if !(min..=max).contains(&ledger_index) || !validated(entry)? {
            return Err(Error::Invalid("unexpected account_tx ledger or validation state"));
        }
        // rippled doesn't include the hash on binary account_tx entries; Clio can.
        transactions.push(parse_binary_tx(entry, ledger_index, true, close_time(entry)?, false)?);
    }
    Ok(AccountTxPage {
        transactions,
        marker: next.cloned(),
    })
}

pub(crate) async fn server_info(rpc: &impl RpcTransport) -> Result<ServerInfo, Error> {
    parse_server_info(&rpc.request("server_info", json!({"api_version": API_VERSION})).await?)
}

pub(crate) async fn ledger_transactions(
    rpc: &impl RpcTransport,
    index: u32,
) -> Result<LedgerBackfill, Error> {
    let result = rpc
        .request(
            "ledger",
            json!({"api_version": API_VERSION, "ledger_index": index, "transactions": true,
                   "expand": true, "binary": true}),
        )
        .await?;
    parse_ledger_backfill(&result, index)
}

/// Parse a `tx` result obtained with `binary: true`. This checks the returned
/// hash against the blob; callers issuing their own query must also compare it
/// against the requested hash.
pub fn parse_tx(result: &Value) -> Result<LedgerTx, Error> {
    check_result(result)?;
    parse_binary_tx(
        result,
        u32_field(result, "ledger_index")?,
        validated(result)?,
        close_time(result)?,
        true,
    )
}

fn parse_binary_tx(
    entry: &Value,
    ledger_index: u32,
    validated: bool,
    close_time: Option<u64>,
    require_hash: bool,
) -> Result<LedgerTx, Error> {
    let tx_blob = blob(entry, "tx_blob")?;
    let hash = keys::tx_hash(&tx_blob);
    if (require_hash || entry.get("hash").is_some()) && hash_field(entry, "hash")? != hash {
        return Err(Error::Invalid("transaction blob hash mismatch"));
    }
    // 'meta' is a hex string in API v1, 'meta_blob' in API v2.
    let meta_blob = blob(entry, if entry.get("meta_blob").is_some() { "meta_blob" } else { "meta" })?;
    let meta = codec::decode(&meta_blob)?;
    let transaction_result = string_field(&meta, "TransactionResult")?.to_owned();
    let transaction_index = u32_field(&meta, "TransactionIndex")?;
    Ok(LedgerTx {
        hash,
        tx_blob,
        meta_blob,
        transaction_result,
        transaction_index,
        ledger_index,
        validated,
        close_time,
    })
}

pub fn parse_server_info(result: &Value) -> Result<ServerInfo, Error> {
    let (network_id, complete_ledgers, validated_ledger) = server_info_fields(result)?;
    Ok(ServerInfo {
        network_id,
        complete_ledgers: complete_ledgers.to_owned(),
        validated_ledger,
    })
}

pub(crate) fn server_info_fields(result: &Value) -> Result<(u32, &str, u32), Error> {
    check_result(result)?;
    let info = &result["info"];
    let validated_ledger = u32_field(&info["validated_ledger"], "seq")?;
    if validated_ledger == 0 {
        return Err(Error::Invalid("server has no validated ledger"));
    }
    Ok((u32_field(info, "network_id")?, string_field(info, "complete_ledgers")?, validated_ledger))
}

fn parse_ledger_header(result: &Value, index: u32) -> Result<(LedgerHeader, Hash32), Error> {
    check_result(result)?;
    let ledger_index = u32_field(result, "ledger_index")?;
    let hash = hash_field(result, "ledger_hash")?;
    let validated = validated(result)?;
    let ledger = &result["ledger"];
    if ledger_index != index || ledger.get("closed").and_then(Value::as_bool) != Some(true) {
        return Err(Error::Invalid("unexpected ledger index or closed state"));
    }
    let (parent_hash, tx_root, close_time) = if let Some(data) = ledger.get("ledger_data") {
        // Binary ledger header: sequence, drops, parent/tx/state hashes,
        // parent close time, close time, resolution, flags.
        let mut bytes = [0u8; 118];
        hex::decode_to_slice(data.as_str().ok_or(Error::Invalid("ledger_data"))?, &mut bytes)
            .map_err(|_| Error::Invalid("ledger_data"))?;
        let mut digest = Sha512::new();
        digest.update(b"LWR\0");
        digest.update(bytes.as_slice());
        if digest.finalize()[..32] != hash || u32::from_be_bytes(bytes[..4].try_into().unwrap()) != index {
            return Err(Error::Invalid("ledger_data hash or index mismatch"));
        }
        (
            bytes[12..44].try_into().unwrap(),
            bytes[44..76].try_into().unwrap(),
            unix_time(u32::from_be_bytes(bytes[112..116].try_into().unwrap())),
        )
    } else {
        if hash_field(ledger, "ledger_hash")? != hash || u32_field(ledger, "ledger_index")? != index {
            return Err(Error::Invalid("ledger header hash or index mismatch"));
        }
        (
            hash_field(ledger, "parent_hash")?,
            hash_field(ledger, "transaction_hash")?,
            unix_time(u32_field(ledger, "close_time")?),
        )
    };
    Ok((LedgerHeader { ledger_index, hash, parent_hash, close_time, validated }, tx_root))
}

fn parse_ledger_backfill(result: &Value, index: u32) -> Result<LedgerBackfill, Error> {
    let (header, tx_root) = parse_ledger_header(result, index)?;
    if !header.validated {
        return Err(Error::Invalid("backfill ledger is not validated"));
    }
    let entries = transaction_array(&result["ledger"])?;
    if entries.is_empty() != (tx_root == [0; 32]) {
        return Err(Error::Invalid("backfill transactions contradict the ledger header"));
    }
    let mut transactions = Vec::with_capacity(entries.len());
    let mut hashes = HashSet::with_capacity(entries.len());
    for entry in entries {
        let record = parse_binary_tx(entry, index, true, Some(header.close_time), true)?;
        if !hashes.insert(record.hash) {
            return Err(Error::Invalid("duplicate transaction hash in backfill ledger"));
        }
        transactions.push(record);
    }
    transactions.sort_unstable_by_key(|tx| tx.transaction_index);
    if transactions.iter().enumerate().any(|(i, tx)| tx.transaction_index as usize != i) {
        return Err(Error::Invalid("missing or duplicate TransactionIndex in backfill ledger"));
    }
    Ok(LedgerBackfill { header, transactions })
}

pub(crate) fn check_result(result: &Value) -> Result<(), Error> {
    if !result.is_object() {
        return Err(Error::Invalid("RPC result must be an object"));
    }
    if let Some(error) = result.get("error") {
        return Err(Error::Rpc(error.as_str().filter(|s| !s.is_empty()).unwrap_or("malformed RPC error").to_owned()));
    }
    if result.get("status").is_some_and(|status| status.as_str() != Some("success")) {
        return Err(Error::Invalid("unexpected RPC result status"));
    }
    Ok(())
}

pub(crate) fn u32_field(value: &Value, field: &'static str) -> Result<u32, Error> {
    value.get(field).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or(Error::Invalid(field))
}

pub(crate) fn hash_field(value: &Value, field: &'static str) -> Result<Hash32, Error> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(string_field(value, field)?, &mut bytes).map_err(|_| Error::Invalid(field))?;
    Ok(bytes)
}

pub(crate) fn string_field<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, Error> {
    value.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or(Error::Invalid(field))
}

pub(crate) fn unix_time(ripple_time: u32) -> u64 {
    u64::from(ripple_time) + RIPPLE_EPOCH
}

fn close_time(value: &Value) -> Result<Option<u64>, Error> {
    let date = value.get("date").map(|_| u32_field(value, "date").map(unix_time)).transpose()?;
    let iso = value.get("close_time_iso").map(|_| {
        let iso = string_field(value, "close_time_iso")?;
        let timestamp = OffsetDateTime::parse(iso, &Rfc3339).map_err(|_| Error::Invalid("close_time_iso"))?;
        let seconds = u64::try_from(timestamp.unix_timestamp()).map_err(|_| Error::Invalid("close_time_iso"))?;
        if !(RIPPLE_EPOCH..=unix_time(u32::MAX)).contains(&seconds) || timestamp.nanosecond() != 0 {
            return Err(Error::Invalid("close_time_iso"));
        }
        Ok(seconds)
    }).transpose()?;
    if date.is_some() && iso.is_some() && date != iso {
        return Err(Error::Invalid("inconsistent transaction close times"));
    }
    Ok(date.or(iso))
}

fn validated(value: &Value) -> Result<bool, Error> {
    match value.get("validated") {
        Some(value) => value.as_bool().ok_or(Error::Invalid("validated")),
        None => Ok(false),
    }
}

fn blob(value: &Value, field: &'static str) -> Result<Vec<u8>, Error> {
    let text = string_field(value, field)?;
    if text.len() > 2 * MAX_BLOB_BYTES {
        return Err(Error::Invalid("oversize XRPL binary blob"));
    }
    hex::decode(text).map_err(|_| Error::Invalid(field))
}

fn transaction_array(value: &Value) -> Result<&[Value], Error> {
    value.get("transactions").and_then(Value::as_array).map(Vec::as_slice).ok_or(Error::Invalid("transactions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEDGER_INDEX: u32 = 22_480_008;
    const LEDGER_HASH: &str = "1AB277F155A8869777C6CD18B53A9366B4C20BC854D25722984049595F103BD0";
    const PARENT_HASH: &str = "BC8CD5C6FCF17FE7FC24551D0304B5B4F246E2830D1373E390BD56A458130199";
    const LEDGER_DATA: &str = concat!(
        "015704880163455AB931BD7B",
        "BC8CD5C6FCF17FE7FC24551D0304B5B4F246E2830D1373E390BD56A458130199",
        "C8A32775A46B0DDDBCBBEB77D0427589410553DD323671BEEE220E1C841D2897",
        "5391F27B40431AFD959ED106C27C939E270CECE068BB85BB7D2E3F539324F4FE",
        "329359EA329359EB0A00",
    );

    fn binary_entry(tx_hex: &str, index: u32, code: u8) -> Value {
        json!({
            "hash": hex::encode_upper(keys::tx_hash(&hex::decode(tx_hex).unwrap())),
            "tx_blob": tx_hex,
            "meta_blob": format!("201C{index:08X}F8F10310{code:02X}"),
        })
    }

    #[test]
    fn binary_tx_binds_hash_and_decodes_rpc_metadata_and_iso() {
        let mut result = binary_entry("120003", 7, 0);
        result["validated"] = json!(true);
        result["ledger_index"] = json!(LEDGER_INDEX);
        result["close_time_iso"] = json!("2026-11-20T19:20:11Z");
        let tx = parse_tx(&result).unwrap();
        assert!(tx.validated);
        assert_eq!(tx.hash, keys::tx_hash(&[0x12, 0, 3]));
        assert_eq!(tx.transaction_index, 7);
        assert_eq!(tx.transaction_result, "tesSUCCESS");
        assert_eq!(tx.close_time, Some(unix_time(848_517_611)));

        result["date"] = json!(848_517_610);
        assert!(parse_tx(&result).is_err());
        result["date"] = json!(848_517_611);
        assert!(parse_tx(&result).is_ok());
        result["tx_blob"] = json!("120000");
        assert!(parse_tx(&result).is_err());
    }

    #[test]
    fn testnet_binary_header_and_backfill_in_transaction_order() {
        // Testnet header from the binary, expanded `ledger` command. Synthetic
        // transaction entries exercise ordering and result decoding below.
        let mut result = json!({
            "ledger_index": LEDGER_INDEX, "ledger_hash": LEDGER_HASH, "validated": true,
            "ledger": {"closed": true, "ledger_data": LEDGER_DATA,
                       "transactions": [binary_entry("120000", 1, 100), binary_entry("120003", 0, 0)]},
        });
        let backfill = parse_ledger_backfill(&result, LEDGER_INDEX).unwrap();
        assert_eq!(hex::encode_upper(backfill.header.parent_hash), PARENT_HASH);
        assert_eq!(backfill.header.close_time, unix_time(848_517_611));
        assert_eq!(backfill.transactions[0].transaction_result, "tesSUCCESS");
        assert_eq!(backfill.transactions[1].transaction_result, "tecCLAIM");
        assert!(backfill.transactions.iter().all(|tx| tx.close_time == Some(backfill.header.close_time)));

        let hash = result["ledger_hash"].take();
        result["ledger_hash"] = json!("0".repeat(64));
        assert!(parse_ledger_backfill(&result, LEDGER_INDEX).is_err());
        result["ledger_hash"] = hash;
        result["validated"] = json!(false);
        assert!(parse_ledger_backfill(&result, LEDGER_INDEX).is_err());
        result["validated"] = json!(true);
        result["ledger"]["transactions"] = json!([]);
        assert!(parse_ledger_backfill(&result, LEDGER_INDEX).is_err());
        result["ledger"]["transactions"] = json!([binary_entry("120003", 1, 0)]);
        assert!(parse_ledger_backfill(&result, LEDGER_INDEX).is_err());
    }

    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    mod async_reads {
        use super::*;

        struct Reply<F>(F);

        impl<F: Fn(&str, Value) -> Value + Sync> RpcTransport for Reply<F> {
            async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
                Ok((self.0)(method, params))
            }
        }

        #[tokio::test]
        async fn account_tx_binary_entries_use_derived_hash_and_opaque_markers() {
            let account = [19; 20];
            let address = keys::classic_address(&account);
            let cursor = json!({"ledger": 11, "seq": 6});
            let next = json!({"ledger": 12, "seq": 7});
            let source = RpcLedger::new(Reply(|method: &str, params: Value| {
                assert_eq!(method, "account_tx");
                assert_eq!(params["account"], address);
                assert_eq!(params["ledger_index_min"], 10);
                assert_eq!(params["ledger_index_max"], 20);
                assert_eq!(params["marker"], cursor);
                assert_eq!(params["binary"], true);
                assert_eq!(params["limit"], 1);
                let mut entry = binary_entry("120003", 6, 0);
                entry.as_object_mut().unwrap().remove("hash");
                entry["ledger_index"] = json!(11);
                entry["validated"] = json!(true);
                json!({"account": address, "validated": true, "ledger_index_min": 10,
                       "ledger_index_max": 20, "marker": next, "transactions": [entry]})
            }));
            let page = source.account_tx(&account, 10, 20, Some(cursor.clone()), 1).await.unwrap();
            assert_eq!(page.marker, Some(next));
            assert_eq!(page.transactions[0].hash, keys::tx_hash(&[0x12, 0, 3]));
            assert_eq!(page.transactions[0].close_time, None);
        }

        #[tokio::test]
        async fn short_account_history_is_not_an_empty_success() {
            let account = [19; 20];
            let source = RpcLedger::new(Reply(|_: &str, _: Value| {
                json!({"account": keys::classic_address(&account), "validated": true,
                       "ledger_index_min": 11, "ledger_index_max": 20, "transactions": []})
            }));
            assert!(source.account_tx(&account, 10, 20, None, 1).await.is_err());
        }

        #[tokio::test]
        async fn only_txn_not_found_maps_to_none_and_requested_hash_must_match() {
            let missing = RpcLedger::new(Reply(|_: &str, _: Value| json!({"error": "txnNotFound"})));
            assert!(missing.tx(&[0; 32]).await.unwrap().is_none());
            let unavailable = RpcLedger::new(Reply(|_: &str, _: Value| json!({"error": "notSynced"})));
            assert!(unavailable.tx(&[0; 32]).await.is_err());
            let mismatch = RpcLedger::new(Reply(|_: &str, _: Value| {
                let mut entry = binary_entry("120003", 0, 0);
                entry["ledger_index"] = json!(10);
                entry["validated"] = json!(true);
                entry
            }));
            assert!(mismatch.tx(&[0; 32]).await.is_err());
        }
    }
}
