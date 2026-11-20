//! The canonical Galaxy AccountSet envelope and single-signer transaction signing.

use crate::codec::{decode, encode, encode_for_signing};
use crate::keys::{classic_address, decode_address, tx_hash, Wallet};
use crate::{AccountId, Error, Hash32};
use serde_json::{json, Value};

pub const MEMO_FORMAT: &str = "application/octet-stream";
pub const MAX_MEMOS_BYTES: usize = 1024;

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
    pub memo: Memo<'a>,
}

impl AccountSet<'_> {
    pub fn to_json(&self) -> Result<Value, Error> {
        if self.sequence == 0 || self.fee_drops == 0 || self.last_ledger_sequence == 0 {
            return Err(Error::Invalid("invalid AccountSet autofill fields"));
        }
        let mut value = json!({
            "TransactionType": "AccountSet",
            "Account": classic_address(&self.account),
            "Sequence": self.sequence,
            "Fee": self.fee_drops.to_string(),
            "LastLedgerSequence": self.last_ledger_sequence,
            "Memos": self.memo.to_json()?,
        });
        if self.network_id > 1024 {
            value["NetworkID"] = json!(self.network_id);
        }
        Ok(value)
    }

    pub fn sign(&self, wallet: &Wallet) -> Result<SignedTransaction, Error> {
        sign_transaction(self.to_json()?, wallet)
    }
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

#[derive(Debug)]
pub struct GalaxyMemo {
    pub account: AccountId,
    pub network_id: Option<u32>,
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
            | "NetworkID" | "SigningPubKey" | "TxnSignature" | "Flags" | "Memos"))
        || object.get("Flags").is_some_and(|flags| !matches!(flags.as_u64(), Some(0 | 0x8000_0000)))
        || !tx["SigningPubKey"].as_str().is_some_and(|s| s.len() == 66 && s.starts_with("ED"))
        || tx["TxnSignature"].as_str().is_none_or(|s| s.len() != 128)
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
    Ok(GalaxyMemo { account, network_id, kind, data })
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
                last_ledger_sequence: 200, network_id: 1, memo };
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
}
