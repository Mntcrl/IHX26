//! The canonical Galaxy AccountSet envelope and single-signer transaction signing.

use crate::codec::{decode, encode, encode_for_signing};
use crate::keys::{account_id, classic_address, decode_address, tx_hash, Wallet};
use crate::{AccountId, Error, Hash32};
use ed25519_dalek::{Signature, VerifyingKey};
use galaxy_protocol::wire::MemoKind;
use serde_json::{json, Value};

pub const MEMO_FORMAT: &str = "application/octet-stream";
pub const MAX_MEMOS_BYTES: usize = 1024;
pub const ASF_ACCOUNT_TXN_ID: u32 = 5;

pub(crate) fn requires_account_txn_id(kind: &str) -> bool {
    matches!(MemoKind::from_memo_type(kind),
        Some(MemoKind::Device | MemoKind::DeviceChunk | MemoKind::Capture | MemoKind::Proof | MemoKind::Revoke))
        || matches!(kind, "galaxy/test/device" | "galaxy/test/device-chunk")
}

pub(crate) fn valid_signing_predecessor(kind: &str, predecessor: Option<Hash32>) -> bool {
    if !requires_account_txn_id(kind) { return true; }
    match predecessor {
        None => false,
        Some(hash) if hash == [0; 32] => kind == MemoKind::DeviceChunk.as_str() || kind == "galaxy/test/device-chunk",
        Some(_) => true,
    }
}

#[derive(Clone, Copy)]
pub struct Memo<'a> {
    pub kind: &'a str,
    pub data: &'a [u8],
}

impl Memo<'_> {
    /// Serialized byte length including the Memos header and both end markers.
    pub fn serialized_len(&self) -> usize {
        let blob = |n: usize| n.saturating_add(if n <= 192 { 2 } else if n <= 12480 { 3 } else { 4 });
        (4 + blob(self.kind.len()) + blob(MEMO_FORMAT.len())).saturating_add(blob(self.data.len()))
    }

    pub fn to_json(&self) -> Result<Value, Error> {
        if self.kind.is_empty()
            || !self.kind.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
            || self.data.is_empty()
            || self.serialized_len() > MAX_MEMOS_BYTES
        {
            return Err(Error::Invalid("invalid Galaxy memo type or memo size"));
        }
        Ok(json!([{"Memo": {
            "MemoType": hex::encode_upper(self.kind),
            "MemoFormat": hex::encode_upper(MEMO_FORMAT),
            "MemoData": hex::encode_upper(self.data),
        }}]))
    }
}

pub struct AccountSet<'a> {
    pub account: AccountId,
    pub sequence: u32,
    pub fee_drops: u64,
    pub last_ledger_sequence: u32,
    pub network_id: u32,
    pub account_txn_id: Option<Hash32>,
    pub memo: Memo<'a>,
}

impl AccountSet<'_> {
    pub fn to_json(&self) -> Result<Value, Error> {
        if !valid_signing_predecessor(self.memo.kind, self.account_txn_id) {
            return Err(Error::Invalid("device record requires a valid AccountTxnID predecessor"));
        }
        let mut value = account_set_fields(&self.account, self.sequence, self.fee_drops, self.last_ledger_sequence, self.network_id)?;
        value["Memos"] = self.memo.to_json()?;
        if let Some(prior) = self.account_txn_id {
            value["AccountTxnID"] = json!(hex::encode_upper(prior));
        }
        Ok(value)
    }

    pub fn sign(&self, wallet: &Wallet) -> Result<SignedTransaction, Error> {
        sign_transaction(self.to_json()?, wallet)
    }
}

fn account_set_fields(account: &AccountId, sequence: u32, fee: u64, last: u32, network: u32) -> Result<Value, Error> {
    if sequence == 0 || fee == 0 || last == 0 {
        return Err(Error::Invalid("invalid AccountSet autofill fields"));
    }
    let mut value = json!({
        "TransactionType": "AccountSet", "Account": classic_address(account), "Sequence": sequence,
        "Fee": fee.to_string(), "LastLedgerSequence": last,
    });
    if network > 1024 {
        value["NetworkID"] = json!(network);
    }
    Ok(value)
}

/// Sign the one-time, non-Galaxy account setup transaction.
pub(crate) fn sign_account_preparation(wallet: &Wallet, sequence: u32, fee: u64, last: u32, network: u32) -> Result<SignedTransaction, Error> {
    let mut tx = account_set_fields(&wallet.account_id(), sequence, fee, last, network)?;
    tx["SetFlag"] = json!(ASF_ACCOUNT_TXN_ID);
    sign_transaction(tx, wallet)
}

/// Validate a persisted account preparation transaction and return the signer.
pub(crate) fn decode_account_preparation(tx: &Value) -> Result<AccountId, Error> {
    let invalid = || Error::Invalid("invalid AccountTxnID preparation transaction");
    let object = tx.as_object().ok_or_else(invalid)?;
    if tx["TransactionType"] != "AccountSet" || tx["SetFlag"].as_u64() != Some(u64::from(ASF_ACCOUNT_TXN_ID))
        || object.keys().any(|key| !matches!(key.as_str(),
            "TransactionType" | "Account" | "Sequence" | "Fee" | "LastLedgerSequence"
            | "NetworkID" | "SigningPubKey" | "TxnSignature" | "SetFlag"))
    {
        return Err(invalid());
    }
    let account = decode_address(tx["Account"].as_str().ok_or_else(invalid)?)?;
    verify_key_signature(tx, &account)?;
    Ok(account)
}

/// The predecessor committed by a decoded XRPL transaction or AccountRoot.
pub fn account_txn_id(object: &Value) -> Result<Option<Hash32>, Error> {
    object.get("AccountTxnID").map(|_| crate::ledger::hash_field(object, "AccountTxnID")).transpose()
}

#[derive(Debug)]
pub struct SignedTransaction {
    pub blob: Vec<u8>,
    pub hash: Hash32,
}

/// Local single-signing only. For Galaxy records use [`AccountSet::sign`], and
/// enforce network and fee policy before broadcasting any raw signed transaction.
pub fn sign_transaction(mut tx: Value, wallet: &Wallet) -> Result<SignedTransaction, Error> {
    let object = tx.as_object_mut().ok_or(Error::Invalid("expected a transaction object"))?;
    let address = wallet.classic_address();
    if object.contains_key("TxnSignature") || object.contains_key("Signers")
        || object.get("Account").and_then(Value::as_str) != Some(address.as_str())
    {
        return Err(Error::Invalid("expected an unsigned transaction bound to the signer"));
    }
    if object.contains_key("SigningPubKey") {
        return Err(Error::Invalid("signer public key is supplied by the wallet"));
    }
    object.insert("SigningPubKey".into(), json!(hex::encode_upper(wallet.public_key())));
    let signature = wallet.sign(&encode_for_signing(&tx)?);
    tx["TxnSignature"] = json!(hex::encode_upper(signature));
    let blob = encode(&tx)?;
    let hash = tx_hash(&blob);
    Ok(SignedTransaction { blob, hash })
}

/// Authenticate an already decoded transaction against the account's Ed25519 master key.
pub fn verify_master_signature(tx: &Value, account: &AccountId) -> Result<(), Error> {
    let address = tx["Account"].as_str().ok_or(Error::Invalid("missing Account"))?;
    if decode_address(address)? != *account {
        return Err(Error::Invalid("unexpected sender account"));
    }
    verify_key_signature(tx, account)
}

fn verify_key_signature(tx: &Value, account: &AccountId) -> Result<(), Error> {
    let invalid = || Error::Invalid("invalid XRPL Ed25519 master-key signature");
    if tx.get("Signers").is_some() { return Err(invalid()); }
    let mut key = [0; 33];
    hex::decode_to_slice(tx["SigningPubKey"].as_str().ok_or_else(invalid)?, &mut key)
        .map_err(|_| invalid())?;
    if key[0] != 0xed || account_id(&key) != *account {
        return Err(invalid());
    }
    let mut signature = [0; 64];
    hex::decode_to_slice(tx["TxnSignature"].as_str().ok_or_else(invalid)?, &mut signature)
        .map_err(|_| invalid())?;
    let key = VerifyingKey::from_bytes(key[1..].try_into().expect("Ed25519 key length"))
        .map_err(|_| invalid())?;
    key.verify_strict(&encode_for_signing(tx)?, &Signature::from_bytes(&signature))
        .map_err(|_| invalid())
}

#[derive(Debug)]
pub struct GalaxyMemo {
    pub account: AccountId,
    pub network_id: Option<u32>,
    pub account_txn_id: Option<Hash32>,
    pub kind: String,
    pub data: Vec<u8>,
}

/// Extract Galaxy's one-memo AccountSet envelope; consensus validation and
/// payload/network binding are separate, mandatory checks by the caller.
pub fn decode_galaxy_memo(blob: &[u8]) -> Result<GalaxyMemo, Error> {
    let tx = decode(blob)?;
    decode_galaxy_object(&tx)
}

/// Extract and validate the envelope from an already-decoded binary object.
pub fn decode_galaxy_object(tx: &Value) -> Result<GalaxyMemo, Error> {
    let invalid = || Error::Invalid("invalid Galaxy AccountSet envelope");
    let object = tx.as_object().ok_or_else(invalid)?;
    if tx["TransactionType"] != "AccountSet"
        || object.keys().any(|key| !matches!(key.as_str(),
            "TransactionType" | "Account" | "Sequence" | "Fee" | "LastLedgerSequence"
            | "NetworkID" | "SigningPubKey" | "TxnSignature" | "Flags" | "Memos" | "AccountTxnID"))
        || object.get("Flags").is_some_and(|flags| !matches!(flags.as_u64(), Some(0 | 0x8000_0000)))
    {
        return Err(invalid());
    }
    let account = decode_address(tx["Account"].as_str().ok_or_else(invalid)?)?;
    let network_id = object.get("NetworkID").map(|field| {
        field.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(invalid)
    }).transpose()?;
    let memos = tx["Memos"].as_array().ok_or_else(invalid)?;
    let [entry] = memos.as_slice() else { return Err(invalid()); };
    let memo = entry["Memo"].as_object().ok_or_else(invalid)?;
    if memo.len() != 3 || entry.as_object().ok_or_else(invalid)?.len() != 1 {
        return Err(invalid());
    }
    let bytes = |key: &str| hex::decode(memo.get(key).and_then(Value::as_str).ok_or_else(invalid)?)
        .map_err(|_| invalid());
    if bytes("MemoFormat")? != MEMO_FORMAT.as_bytes() {
        return Err(invalid());
    }
    let kind = String::from_utf8(bytes("MemoType")?).map_err(|_| invalid())?;
    let data = bytes("MemoData")?;
    if !kind.starts_with("galaxy/") || data.is_empty()
        || (Memo { kind: &kind, data: &data }).serialized_len() > MAX_MEMOS_BYTES
    {
        return Err(invalid());
    }
    verify_key_signature(tx, &account)?;
    Ok(GalaxyMemo { account, network_id, account_txn_id: account_txn_id(tx)?, kind, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_memos_size_and_no_accountset_side_effects() {
        let wallet = Wallet::from_entropy(&[1; 16]);
        for (kind, len) in [("galaxy/v1/capture", 901), ("galaxy/v1/capture", 751), ("galaxy/v1/proof", 361)] {
            let data = vec![0;len];
            let memo = Memo { kind, data: &data };
            let tx = AccountSet { account: wallet.account_id(), sequence: 1, fee_drops: 10,
                last_ledger_sequence: 200, network_id: 1, account_txn_id: Some([3; 32]), memo };
            let signed = tx.sign(&wallet).unwrap();
            let mut decoded = decode(&signed.blob).unwrap();
            assert_eq!(decode_galaxy_object(&decoded).unwrap().data, data);
            decoded.as_object_mut().unwrap().remove("Memos");
            assert_eq!(signed.blob.len() - encode(&decoded).unwrap().len(), memo.serialized_len());
            assert!(decoded.get("NetworkID").is_none());
            decoded["SetFlag"] = json!(1);
            assert!(decode_galaxy_object(&decoded).is_err());
        }
    }

    #[test]
    fn signature_authenticates_the_predecessor_and_the_master_account() {
        let wallet = Wallet::from_entropy(&[1; 16]);
        let regular_key = Wallet::from_entropy(&[2; 16]);
        let tx = AccountSet {
            account: wallet.account_id(), sequence: 1, fee_drops: 10, last_ledger_sequence: 200,
            network_id: 1, account_txn_id: Some([3; 32]), memo: Memo { kind: MemoKind::Capture.as_str(), data: &[1] },
        };
        let signed = tx.sign(&wallet).unwrap();
        let mut object = decode(&signed.blob).unwrap();
        assert_eq!(decode_galaxy_object(&object).unwrap().account_txn_id, Some([3; 32]));
        object["AccountTxnID"] = json!(hex::encode_upper([4; 32]));
        assert!(decode_galaxy_object(&object).is_err(), "tampering breaks the signature");
        object["SigningPubKey"] = json!(hex::encode_upper(regular_key.public_key()));
        object["TxnSignature"] = json!(hex::encode_upper(regular_key.sign(&encode_for_signing(&object).unwrap())));
        assert!(decode_galaxy_object(&object).is_err(), "a valid signature by another key is not a master signature");
        assert!(AccountSet { account_txn_id: Some([0; 32]), ..tx }.sign(&wallet).is_err());
    }
}
