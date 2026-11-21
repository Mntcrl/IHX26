use galaxy_xrpl::codec;
use serde_json::{json, Value};

use super::*;

const MAX_BLOB: usize = 4 * MIB;

fn initialize_definitions() {
    codec::decode(&[0x12, 0x00, 0x03]).unwrap(); // TransactionType = AccountSet.
}

fn decode(bytes: &[u8]) -> Result<Value, galaxy_xrpl::Error> {
    // JSON output keys, node allocations and hex strings are larger than XRPL wire fields.
    bounded("XRPL codec", 16 * KIB + 256 * bytes.len().min(MAX_BLOB), || codec::decode(bytes))
}

#[test]
fn xrpl_arbitrary_bytes_and_size_cap() {
    initialize_definitions();
    property(384, noise(8 * KIB), |bytes| {
        let _ = decode(&bytes);
        Ok(())
    });
    let oversized = vec![0; MAX_BLOB + 1];
    assert!(bounded("XRPL oversize", KIB, || codec::decode(&oversized)).is_err());
}

#[test]
fn xrpl_generated_account_set_round_trips() {
    initialize_definitions();
    property(128, (any::<u32>(), any::<u32>(), noise(512)), |(sequence, fee, data)| {
        let tx = json!({
            "TransactionType": "AccountSet", "Sequence": sequence,
            "Fee": u64::from(fee).to_string(),
            "Account": "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh", "SigningPubKey": "",
            "Memos": [{"Memo": {"MemoData": hex::encode_upper(data)}}],
        });
        let encoded = codec::encode(&tx).unwrap();
        prop_assert_eq!(decode(&encoded).unwrap(), tx);
        Ok(())
    });
}

fn ledger_vectors() -> Vec<Vec<u8>> {
    let vectors: Value = serde_json::from_str(include_str!("../../../test-vectors/xrpl/testnet.json")).unwrap();
    vectors["rows"].as_array().unwrap().iter()
        .flat_map(|row| ["tx_blob", "meta_blob"].map(|field| hex::decode(row[field].as_str().unwrap()).unwrap()))
        .collect()
}

#[test]
fn xrpl_ledger_vectors_round_trip() {
    initialize_definitions();
    for bytes in ledger_vectors() {
        prop_assert_codec(&bytes, true).unwrap();
    }
}

#[test]
fn xrpl_ledger_vector_mutations_are_canonical_or_rejected() {
    initialize_definitions();
    let fixtures = ledger_vectors();
    property(1024, (0..fixtures.len(), mutation()), |(index, (position, mask))| {
        let mut bytes = fixtures[index].clone();
        let at = change(&mut bytes, position, mask);
        prop_assert!(prop_assert_codec(&bytes, false).is_ok(), "noncanonical XRPL: vector {index}, offset {at}, mask {mask:#04x}");
        Ok(())
    });
}

fn prop_assert_codec(bytes: &[u8], valid: bool) -> TestCaseResult {
    match decode(bytes) {
        Ok(value) => {
            let encoded = codec::encode(&value).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert!(encoded == bytes, "noncanonical XRPL encoding");
        }
        Err(_) => prop_assert!(!valid, "valid ledger blob rejected"),
    }
    Ok(())
}
