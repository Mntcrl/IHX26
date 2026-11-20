use super::{MAX_ARRAY_ITEMS, MAX_BLOB, MAX_DEPTH, MAX_VL, decode, encode, encode_for_signing};
use serde_json::{Value, json};

const ACCOUNT: &str = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
const ACCOUNT_HEX: &str = "B5F762798A53D543A014CAF8B297CFF8F2F937E8";
const ZERO_ACCOUNT: &str = "rrrrrrrrrrrrrrrrrrrrrhoLvTp";
const MPT: &str = "00010203B5F762798A53D543A014CAF8B297CFF8F2F937E8";

fn bytes(hex: &str) -> Vec<u8> {
    hex::decode(hex).unwrap()
}

fn wire(object: Value, hex: &str) {
    let expected = bytes(hex);
    assert_eq!(encode(&object).unwrap(), expected);
    assert_eq!(decode(&expected).unwrap(), object);
}

#[test]
fn accountset_memos_and_xrpl_js_signing_preimage() {
    let mut tx = json!({
        "TransactionType": "AccountSet", "Flags": 2_147_483_648u32,
        "Account": ACCOUNT, "Sequence": 7, "LastLedgerSequence": 100,
        "Fee": "12", "SigningPubKey": "", "TxnSignature": "ABCD",
        "Memos": [{"Memo": { "MemoType": "67616C617879", "MemoFormat": "78", "MemoData": "00FF" }}]
    });
    let common = "12000322800000002400000007201B0000006468400000000000000C7300";
    let account_and_memos = format!("8114{ACCOUNT_HEX}F9EA7C0667616C6178797D0200FF7E0178E1F1");
    let expected = format!("{common}7402ABCD{account_and_memos}");
    wire(tx.clone(), &expected);
    let signing = format!("53545800{common}{account_and_memos}");
    assert_eq!(encode_for_signing(&tx).unwrap(), bytes(&signing));
    tx["Signers"] = json!([{"Signer": {"Account": ACCOUNT, "SigningPubKey": "23", "TxnSignature": "01"}}]);
    tx["validated"] = json!(true);
    tx["hash"] = json!("ignored");
    assert_eq!(encode_for_signing(&tx).unwrap(), bytes(&signing));
}

#[test]
fn metadata_enums_and_nested_containers() {
    let ledger_index = "AA".repeat(32);
    let node = format!("E511006156{ledger_index}E662400000000000000AE1E762400000000000000B8114{ACCOUNT_HEX}E1E1");
    let metadata = json!({
        "AffectedNodes": [{"ModifiedNode": {
            "LedgerEntryType": "AccountRoot", "LedgerIndex": ledger_index,
            "FinalFields": {"Account": ACCOUNT, "Balance": "11"},
            "PreviousFields": {"Balance": "10"}
        }}],
        "TransactionIndex": 17, "TransactionResult": "tecUNFUNDED_PAYMENT"
    });
    wire(metadata, &format!("201C00000011F8{node}F1031068"));
    wire(json!({"AffectedNodes": [], "TransactionResult": "tesSUCCESS"}), "F8F1031000");
    wire(json!({"PermissionValue": "TrustlineFreeze"}), "203400010002");
    wire(json!({"PermissionValue": "AccountSet"}), "203400000004");
    wire(json!({"TransactionType": "UNLModify", "Account": ZERO_ACCOUNT}), "1200668100");
}

#[test]
fn fixed_types_and_extended_headers() {
    let hash128 = "AB".repeat(16);
    let hash160 = "EF".repeat(20);
    let hash256 = "AB".repeat(32);
    let vector = "FF".repeat(32);
    wire(json!({
        "IndexNext": "0000000000000ABC", "MaximumAmount": "18446744073709551615",
        "SignerWeight": 65535, "TickSize": 15, "EmailHash": hash128,
        "TakerGetsIssuer": hash160, "MPTokenIssuanceID": MPT, "PreviousTxnID": hash256,
        "Hashes": [vector, hash256], "LoanScale": -12, "BaseAsset": "a?C"
    }), &format!(
        "13FFFF310000000000000ABC3018FFFFFFFFFFFFFFFF41{hash128}55{hash256}A1FFFFFFF40010100F0411{hash160}021340{vector}{hash256}0115{MPT}011A000000000000000000000000613F430000000000"
    ));
    let non_iso = "FF000000000000000000000000000000000000AB";
    wire(json!({"BaseAsset": non_iso}), &format!("011A{non_iso}"));
    let non_native_xrp = "0000000000000000000000005852500000000000";
    wire(json!({"BaseAsset": non_native_xrp}), &format!("011A{non_native_xrp}"));
    assert_eq!(encode(&json!({"IndexNext": "abc", "EmailHash": hash128.to_lowercase()})).unwrap(), bytes(&format!("310000000000000ABC41{hash128}")));
    assert!(encode(&json!({"TickSize": 256})).is_err());
    assert!(encode(&json!({"LoanScale": -2147483649i64})).is_err());
    assert!(encode(&json!({"PreviousTxnID": "AA"})).is_err());
}

#[test]
fn amounts_and_number_are_exact() {
    let usd = "0000000000000000000000005553440000000000";
    wire(json!({"Amount": "100000000000000000", "FeeAmountDelta": "-12"}), "61416345785D8A00006022000000000000000C");
    wire(json!({"Amount": {"currency": "USD", "issuer": ACCOUNT, "value": "0"}}), &format!("618000000000000000{usd}{ACCOUNT_HEX}"));
    wire(json!({"Amount": {"mpt_issuance_id": MPT, "value": "9223372036854775807"}}), &format!("61607FFFFFFFFFFFFFFF{MPT}"));
    wire(json!({"Balance": {"mpt_issuance_id": MPT, "value": "-15"}}), &format!("6220000000000000000F{MPT}"));
    wire(json!({"Number": "1"}), "910DE0B6B3A7640000FFFFFFEE");
    wire(json!({"Number": "0"}), "91000000000000000080000000");
    for amount in ["-12.00000000000001", "123.4567890123456", "0.0000304", "0.001"] {
        let object = json!({"Balance": {"currency": "USD", "issuer": ACCOUNT, "value": amount}});
        assert_eq!(decode(&encode(&object).unwrap()).unwrap(), object);
    }
    for (input, expected) in [
        ("1e-81", format!("0.{}1", "0".repeat(80))),
        ("1e95", format!("1{}", "0".repeat(95))),
        ("+00001.000e-1", "0.1".into()),
    ] {
        let input = json!({"Balance": {"currency": "USD", "issuer": ACCOUNT, "value": input}});
        assert_eq!(decode(&encode(&input).unwrap()).unwrap()["Balance"]["value"], expected);
    }
    // Expected wire bytes and decoded text were produced by ripple-binary-codec 2.11.0.
    for (input, hex, expected) in [
        ("-1.2345", "91EEDE2CCA68C7C000FFFFFFEE", "-1.2345"),
        ("0.0000000001", "910DE0B6B3A7640000FFFFFFE4", "0.0000000001"),
        ("1e12", "910DE0B6B3A7640000FFFFFFFA", "1e12"),
        ("0.00000000001", "910DE0B6B3A7640000FFFFFFE3", "1e-11"),
        ("1.2345678901234567895", "91112210F47DE98116FFFFFFEE", "1.23456789012345679"),
        ("9223372036854775807", "917FFFFFFFFFFFFFFF00000000", "9223372036854775807"),
        ("9223372036854775808", "910CCCCCCCCCCCCCCD00000001", "9223372036854775810"),
        ("99999999999999999999", "910DE0B6B3A764000000000002", "1e20"),
    ] {
        let binary = encode(&json!({"Number": input})).unwrap();
        assert_eq!(hex::encode_upper(&binary), hex, "{input}");
        assert_eq!(decode(&binary).unwrap()["Number"], expected, "{input}");
    }
    for amount in ["1e96", "1e-82", "1.2345678901234567", "1e", "NaN"] {
        assert!(encode(&json!({"Balance": {"currency": "USD", "issuer": ACCOUNT, "value": amount}})).is_err());
    }
    assert!(encode(&json!({"Number": "1e32787"})).is_err());
    assert!(encode(&json!({"Amount": "100000000000000001"})).is_err());
    assert!(decode(&bytes("610000000000000000")).is_err());
    let mut truncated_mpt = bytes(&format!("61607FFFFFFFFFFFFFFF{MPT}"));
    truncated_mpt.pop();
    assert!(decode(&truncated_mpt).is_err());
    assert!(decode(&bytes("91000000000000000100000000")).is_err());
}

#[test]
fn issues_bridge_and_payment_paths() {
    let xrp = "00".repeat(20);
    let usd = "0000000000000000000000005553440000000000";
    let sentinel = "0000000000000000000000000000000000000001";
    let iou_issue = format!("{usd}{ACCOUNT_HEX}");
    wire(json!({"Asset": {"mpt_issuance_id": MPT}}), &format!("0318{ACCOUNT_HEX}{sentinel}03020100"));
    wire(json!({"Asset": {"currency": "XRP"}}), &format!("0318{xrp}"));
    wire(json!({"Asset": {"currency": "USD", "issuer": ACCOUNT}}), &format!("0318{iou_issue}"));
    let bridge = json!({"XChainBridge": {
        "LockingChainDoor": ACCOUNT, "LockingChainIssue": {"currency": "USD", "issuer": ACCOUNT},
        "IssuingChainDoor": ZERO_ACCOUNT, "IssuingChainIssue": {"currency": "XRP"}
    }});
    wire(bridge, &format!("011914{ACCOUNT_HEX}{iou_issue}14{xrp}{xrp}"));
    assert!(decode(&bytes(&format!("011913{ACCOUNT_HEX}{iou_issue}14{xrp}{xrp}"))).is_err());
    wire(json!({"Paths": [[{"account": ACCOUNT}, {"currency": "XRP"}], [{"currency": "USD", "issuer": ACCOUNT}]]}),
        &format!("011201{ACCOUNT_HEX}10{xrp}FF30{usd}{ACCOUNT_HEX}00"));
    for malformed in ["0112", "011200", "0112FF", "011202", "011201", "0112FF00"] {
        assert!(decode(&bytes(malformed)).is_err(), "accepted {malformed}");
    }
}

#[test]
fn vl_thresholds_and_bounds() {
    for (length, prefix) in [(0, "00"), (192, "C0"), (193, "C100"), (12_480, "F0FF"), (12_481, "F10000"), (MAX_VL, "FED417")] {
        let data = "AB".repeat(length);
        let object = json!({"MemoData": data});
        let blob = encode(&object).unwrap();
        assert!(blob.starts_with(&bytes(&format!("7D{prefix}"))));
        assert_eq!(decode(&blob).unwrap(), object);
    }
    let above_max = "AB".repeat(MAX_VL + 1);
    assert!(encode(&json!({"MemoData": above_max})).is_err());
    assert!(decode(&[0; 0]).unwrap().is_object());
    assert!(decode(&vec![0; MAX_BLOB + 1]).is_err());
    for malformed in ["7DFF", "7DFED418", "7DC1", "7DF100", "7D02AB", "8101FF", "01130100"] {
        assert!(decode(&bytes(malformed)).is_err(), "accepted {malformed}");
    }
}

#[test]
fn rejects_unknown_noncanonical_trailing_and_incomplete_fields() {
    for malformed in [
        "12", "1200", "1200FF", "120003E1", "120003F1", "12000300", "120003DF",
        "120003120003", "2200000001120003", "200101020304", "0308", "001010",
        "F9", "F9EAE1", "F9EAF1", "F92200000000F1", "E1", "E7E1E7E1", "E7",
        "811400000000000000000000000000000000000000", "0318FF", "03120000",
    ] {
        assert!(decode(&bytes(malformed)).is_err(), "accepted {malformed}");
    }
    for malformed in [json!([]), json!({"UnknownField": 1}), json!({"ObjectEndMarker": {}}), json!({"Memos": [{"Flags": 7}]}), json!({"Memos": [{"Memo": { "MemoData": "F" }}]})] {
        assert!(encode(&malformed).is_err(), "accepted {malformed}");
    }
    let mut nested = json!({});
    for _ in 0..=MAX_DEPTH {
        nested = json!({"FinalFields": nested});
    }
    assert!(encode(&nested).is_err());
    let mut wire = vec![0xE7; MAX_DEPTH + 1];
    wire.extend(std::iter::repeat_n(0xE1, MAX_DEPTH + 1));
    assert!(decode(&wire).is_err());
    let mut wire = vec![0xF9];
    for _ in 0..=MAX_ARRAY_ITEMS {
        wire.extend_from_slice(&[0xEA, 0xE1]);
    }
    wire.push(0xF1);
    assert!(decode(&wire).is_err());
}
