//! Sans-IO, UniFFI-friendly surface for the iOS app: owned argument and return
//! types only (`String`, `Vec<u8>`, integers, records, fieldless enums). Rust
//! never performs network I/O here; the app sends the JSON-RPC request bodies
//! built below over URLSession and passes the raw HTTP response bodies back.
//!
//! Flow: `server_info_request` → `validated_ledger_from_server_info`;
//! `fee_request` + `account_info_request` → `autofill`; `sign_galaxy_account_set`
//! → persist the returned [`SignedRecord`] durably (blob + hash + window) BEFORE
//! the first submit; `submit_request` → `evaluate_submit_response`;
//! `tx_request` → `evaluate_tx_response` until it is not `Pending`. After a
//! crash, resubmit the persisted record unchanged; never re-sign the same
//! purpose unless `evaluate_tx_response` reported `Expired` (for a CAPTURE the
//! app decides whether a new capture still fits its deadline).

#[cfg(any(test, not(target_arch = "wasm32")))]
use crate::keys::encode_family_seed;
use crate::keys::{decode_family_seed, Wallet};
use crate::ledger::{parse_server_info, take_http_result};
use crate::submission::{self, Autofill, PendingTx, ResultClass, TxState};
use crate::transaction::{decode_galaxy_object, AccountSet, Memo};
use crate::Error;
use serde_json::{json, Value};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum MobileError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("XRPL network mismatch, or mainnet not explicitly enabled")]
    Network,
    #[error("XRPL RPC: {message}")]
    Rpc { message: String },
}

impl From<Error> for MobileError {
    fn from(err: Error) -> Self {
        match err {
            Error::Network => Self::Network,
            Error::Rpc(message) => Self::Rpc { message },
            other => Self::InvalidInput { message: other.to_string() },
        }
    }
}

impl From<serde_json::Error> for MobileError {
    fn from(err: serde_json::Error) -> Self { Error::from(err).into() }
}

/// Network policy the app enforces on every step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkParams {
    pub network_id: u32,
    pub allow_mainnet: bool,
    pub max_fee_drops: u64,
}

impl NetworkParams {
    fn check(&self) -> Result<(), MobileError> {
        if self.network_id == 0 && !self.allow_mainnet { return Err(MobileError::Network); }
        if self.max_fee_drops == 0 { return Err(Error::Invalid("zero max_fee_drops").into()); }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletInfo {
    pub classic_address: String,
    /// 20-byte AccountID.
    pub account_id: Vec<u8>,
    /// 33-byte XRPL ed25519 public key (0xED prefix).
    pub public_key: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutofillFields {
    pub sequence: u32,
    pub fee_drops: u64,
    pub last_ledger_sequence: u32,
    pub first_possible_ledger: u32,
}

/// The journal record the app must persist before submitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRecord {
    pub purpose: String,
    /// 64 uppercase hex characters: SHA-512Half("TXN\0" ‖ blob).
    pub hash_hex: String,
    pub blob: Vec<u8>,
    pub network_id: u32,
    pub first_possible_ledger: u32,
    pub last_ledger: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// Provisional engine result; never final.
    pub engine_result: String,
    pub class: ResultClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxStatus {
    /// Not yet in a validated ledger and the window is still open.
    Pending,
    /// The server searched the whole window and the validated ledger passed
    /// `last_ledger`: the transaction can never apply.
    Expired,
    /// Final. `success` = validated ∧ `tesSUCCESS`; otherwise a `tec*` result
    /// that consumed the fee.
    Validated { ledger_index: u32, transaction_index: u32, transaction_result: String, success: bool },
}

/// A new random family seed (`sEd…`). The app stores it in the Keychain.
#[cfg(not(target_arch = "wasm32"))]
pub fn generate_family_seed() -> Result<String, MobileError> {
    let mut entropy = Zeroizing::new([0u8; 16]);
    getrandom::fill(&mut entropy[..]).map_err(|_| Error::Invalid("operating system RNG unavailable"))?;
    Ok(encode_family_seed(&entropy).to_string())
}

pub fn wallet_info(seed: String) -> Result<WalletInfo, MobileError> {
    let wallet = wallet_from(seed)?;
    Ok(WalletInfo { classic_address: wallet.classic_address(), account_id: wallet.account_id().to_vec(),
        public_key: wallet.public_key().to_vec() })
}

fn wallet_from(seed: String) -> Result<Wallet, MobileError> {
    let seed = Zeroizing::new(seed);
    let entropy = decode_family_seed(&seed)?;
    Ok(Wallet::from_entropy(&entropy))
}

fn rpc_body(method: &str, params: Value) -> String {
    let mut params = params;
    params["api_version"] = json!(2);
    json!({"method": method, "params": [params]}).to_string()
}

/// Parses an HTTP response body and fails on an RPC-level error.
fn rpc_result(body: &str) -> Result<Value, MobileError> {
    let result = take_http_result(serde_json::from_str(body)?)?;
    if let Some(code) = result["error"].as_str() { return Err(MobileError::Rpc { message: code.to_owned() }); }
    Ok(result)
}

pub fn server_info_request() -> String { rpc_body("server_info", json!({})) }

pub fn fee_request() -> String { rpc_body("fee", json!({})) }

pub fn account_info_request(classic_address: String, validated_ledger: u32) -> String {
    rpc_body("account_info", submission::account_info_params(&classic_address, validated_ledger))
}

/// The latest validated ledger index; fails closed unless the server's
/// `network_id` equals the configured network.
pub fn validated_ledger_from_server_info(network: NetworkParams, body: String) -> Result<u32, MobileError> {
    network.check()?;
    let info = parse_server_info(&rpc_result(&body)?)?;
    if info.network_id != network.network_id { return Err(MobileError::Network); }
    Ok(info.validated_ledger)
}

/// Fee (open-ledger fee, capped), Sequence (validated account state) and
/// `LastLedgerSequence = min(validated + 20, deadline)`.
pub fn autofill(
    network: NetworkParams, validated_ledger: u32, deadline: Option<u32>, classic_address: String,
    fee_body: String, account_info_body: String,
) -> Result<AutofillFields, MobileError> {
    network.check()?;
    let fee = rpc_result(&fee_body)?;
    let account = rpc_result(&account_info_body)?;
    let a = submission::autofill_from_responses(validated_ledger, deadline, network.max_fee_drops, &classic_address, &fee, &account)?;
    Ok(AutofillFields { sequence: a.sequence, fee_drops: a.fee_drops, last_ledger_sequence: a.last_ledger_sequence,
        first_possible_ledger: a.first_possible_ledger })
}

/// Signs a Galaxy AccountSet carrying exactly one Memo (MemoType = `memo_type`
/// ASCII, MemoFormat `application/octet-stream`, MemoData = `memo_data`).
pub fn sign_galaxy_account_set(
    seed: String, network: NetworkParams, purpose: String, fields: AutofillFields, memo_type: String, memo_data: Vec<u8>,
) -> Result<SignedRecord, MobileError> {
    network.check()?;
    if fields.fee_drops > network.max_fee_drops { return Err(Error::Invalid("fee exceeds configured cap").into()); }
    let autofill = Autofill { sequence: fields.sequence, fee_drops: fields.fee_drops,
        last_ledger_sequence: fields.last_ledger_sequence, first_possible_ledger: fields.first_possible_ledger };
    let wallet = wallet_from(seed)?;
    let signed = AccountSet { account: wallet.account_id(), sequence: fields.sequence, fee_drops: fields.fee_drops,
        last_ledger_sequence: fields.last_ledger_sequence, network_id: network.network_id,
        memo: Memo { kind: &memo_type, data: &memo_data } }.sign(&wallet)?;
    Ok(record_of(PendingTx::new(&purpose, signed, network.network_id, &autofill)?))
}

fn record_of(p: PendingTx) -> SignedRecord {
    SignedRecord { purpose: p.purpose, hash_hex: hex::encode_upper(p.hash), blob: p.blob, network_id: p.network_id,
        first_possible_ledger: p.first_possible_ledger, last_ledger: p.last_ledger }
}

/// Re-validates a persisted record (hash, window, fee cap, network) before use.
fn pending_of(network: &NetworkParams, record: SignedRecord) -> Result<PendingTx, MobileError> {
    network.check()?;
    let mut hash = [0u8; 32];
    if record.hash_hex.len() != 64 || hex::decode_to_slice(&record.hash_hex, &mut hash).is_err() {
        return Err(Error::Invalid("invalid record hash").into());
    }
    let pending = PendingTx { purpose: record.purpose, hash, blob: record.blob, network_id: record.network_id,
        first_possible_ledger: record.first_possible_ledger, last_ledger: record.last_ledger };
    let decoded = pending.check_fee(network.network_id, network.max_fee_drops)?;
    decode_galaxy_object(&decoded)?;
    Ok(pending)
}

/// `submit` body for exactly the persisted blob; refused once the validated
/// ledger reached `last_ledger` (the transaction can no longer apply).
pub fn submit_request(network: NetworkParams, record: SignedRecord, validated_ledger: u32) -> Result<String, MobileError> {
    let pending = pending_of(&network, record)?;
    if validated_ledger >= pending.last_ledger {
        return Err(Error::Invalid("refusing to submit against an already closed last ledger").into());
    }
    Ok(rpc_body("submit", json!({"tx_blob": hex::encode_upper(&pending.blob)})))
}

pub fn evaluate_submit_response(network: NetworkParams, record: SignedRecord, body: String) -> Result<SubmitOutcome, MobileError> {
    let pending = pending_of(&network, record)?;
    let result = take_http_result(serde_json::from_str(&body)?)?;
    let outcome = submission::evaluate_submit(&pending, &result)?;
    Ok(SubmitOutcome { engine_result: outcome.code, class: outcome.class })
}

/// `tx` lookup body; pass the same `validated_ledger` to [`evaluate_tx_response`].
pub fn tx_request(network: NetworkParams, record: SignedRecord, validated_ledger: u32) -> Result<String, MobileError> {
    let pending = pending_of(&network, record)?;
    Ok(rpc_body("tx", submission::tx_lookup_params(&pending, validated_ledger)))
}

/// The tx hash is recomputed from the returned blob and must equal the record;
/// `validated`, ledger index and result are trusted from the RPC.
pub fn evaluate_tx_response(network: NetworkParams, record: SignedRecord, validated_ledger: u32, body: String) -> Result<TxStatus, MobileError> {
    let pending = pending_of(&network, record)?;
    let result = take_http_result(serde_json::from_str(&body)?)?;
    Ok(match submission::evaluate_tx_lookup(&pending, validated_ledger, &result)? {
        TxState::Pending => TxStatus::Pending,
        TxState::Expired => TxStatus::Expired,
        TxState::Validated(tx) => TxStatus::Validated {
            ledger_index: tx.ledger_index, transaction_index: tx.transaction_index,
            success: tx.validated && tx.transaction_result == "tesSUCCESS", transaction_result: tx.transaction_result,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec;

    const NET: NetworkParams = NetworkParams { network_id: 1, allow_mainnet: false, max_fee_drops: 1000 };

    fn ok(result: Value) -> String { json!({"result": result}).to_string() }

    #[test]
    fn sans_io_round_trip_matches_submitter_rules() {
        let seed = encode_family_seed(&[7; 16]).to_string();
        let info = wallet_info(seed.clone()).unwrap();
        assert_eq!(info.public_key[0], 0xED);
        let server = ok(json!({"status":"success","info":{"network_id":1,"complete_ledgers":"1-500","validated_ledger":{"seq":400}}}));
        assert_eq!(validated_ledger_from_server_info(NET, server.clone()).unwrap(), 400);
        assert!(matches!(validated_ledger_from_server_info(NetworkParams { network_id: 2, ..NET }, server), Err(MobileError::Network)));
        assert!(matches!(validated_ledger_from_server_info(NetworkParams { network_id: 0, ..NET }, String::new()), Err(MobileError::Network)));

        let fee = ok(json!({"drops":{"open_ledger_fee":"12"}}));
        let account = ok(json!({"validated":true,"ledger_index":400,"account_data":{"Account":info.classic_address,"Sequence":9}}));
        let fields = autofill(NET, 400, Some(410), info.classic_address.clone(), fee.clone(), account.clone()).unwrap();
        assert_eq!(fields, AutofillFields { sequence: 9, fee_drops: 12, last_ledger_sequence: 410, first_possible_ledger: 401 });
        assert!(autofill(NetworkParams { max_fee_drops: 11, ..NET }, 400, None, info.classic_address.clone(), fee, account).is_err());

        let record = sign_galaxy_account_set(seed.clone(), NET, "capture/1".into(), fields, "galaxy/test/mobile".into(), vec![5; 32]).unwrap();
        assert_eq!(record.hash_hex, hex::encode_upper(crate::keys::tx_hash(&record.blob)));
        let decoded = codec::decode(&record.blob).unwrap();
        assert_eq!(decoded["TransactionType"], "AccountSet");
        assert_eq!((decoded["Sequence"].as_u64(), decoded["LastLedgerSequence"].as_u64()), (Some(9), Some(410)));
        assert!(decoded.get("NetworkID").is_none());
        let too_expensive = AutofillFields { fee_drops: 1001, ..fields };
        assert!(sign_galaxy_account_set(seed, NET, "p".into(), too_expensive, "galaxy/test/mobile".into(), vec![1]).is_err());

        let body: Value = serde_json::from_str(&submit_request(NET, record.clone(), 400).unwrap()).unwrap();
        assert_eq!(body["params"][0]["tx_blob"], hex::encode_upper(&record.blob));
        assert!(submit_request(NET, record.clone(), 410).is_err());
        let mut tampered = record.clone();
        tampered.blob[10] ^= 1;
        assert!(submit_request(NET, tampered, 400).is_err());
        let mut tampered = record.clone();
        tampered.purpose.clear();
        assert!(submit_request(NET, tampered, 400).is_err());
        assert!(matches!(submit_request(NetworkParams { network_id: 0, ..NET }, record.clone(), 400), Err(MobileError::Network)));
        assert!(submit_request(NetworkParams { max_fee_drops: 11, ..NET }, record.clone(), 400).is_err());
        let outcome = evaluate_submit_response(NET, record.clone(),
            ok(json!({"engine_result":"terQUEUED","tx_blob":hex::encode_upper(&record.blob)}))).unwrap();
        assert_eq!(outcome.class, ResultClass::Retry);
        assert!(evaluate_submit_response(NET, record.clone(), ok(json!({"engine_result":"tesSUCCESS","tx_blob":"00"}))).is_err());

        let lookup: Value = serde_json::from_str(&tx_request(NET, record.clone(), 411).unwrap()).unwrap();
        assert_eq!((lookup["params"][0]["min_ledger"].as_u64(), lookup["params"][0]["max_ledger"].as_u64()), (Some(401), Some(410)));
        let not_found = |searched: bool| ok(json!({"error":"txnNotFound","searched_all":searched}));
        assert_eq!(evaluate_tx_response(NET, record.clone(), 405, not_found(false)).unwrap(), TxStatus::Pending);
        assert!(evaluate_tx_response(NET, record.clone(), 411, not_found(false)).is_err());
        assert_eq!(evaluate_tx_response(NET, record.clone(), 411, not_found(true)).unwrap(), TxStatus::Expired);
        // TransactionIndex 3, empty AffectedNodes, TransactionResult tesSUCCESS.
        let mut validated = json!({"tx_blob":hex::encode_upper(&record.blob),"meta_blob":"201C00000003F8F1031000",
            "validated":true,"ledger_index":403,"hash":record.hash_hex,"close_time_iso":"2026-11-20T20:00:00Z"});
        assert_eq!(evaluate_tx_response(NET, record.clone(), 405, ok(validated.clone())).unwrap(),
            TxStatus::Validated { ledger_index: 403, transaction_index: 3, transaction_result: "tesSUCCESS".into(), success: true });
        validated["meta_blob"] = json!("201C00000003F8F103107D"); // tecNO_DST_INSUF_XRP
        assert_eq!(evaluate_tx_response(NET, record.clone(), 405, ok(validated.clone())).unwrap(),
            TxStatus::Validated { ledger_index: 403, transaction_index: 3, transaction_result: "tecNO_DST_INSUF_XRP".into(), success: false });
        validated["hash"] = json!("00".repeat(32));
        assert!(evaluate_tx_response(NET, record, 405, ok(validated)).is_err());
    }
}
