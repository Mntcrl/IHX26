//! Differential vectors produced by `scripts/xrpl-cross-check.mjs`.

use galaxy_xrpl::codec::{decode, encode};
use galaxy_xrpl::keys::{decode_family_seed, encode_family_seed, Wallet};
use galaxy_xrpl::transaction::{AccountSet, Memo};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const TESTNET: &str = include_str!("../../../test-vectors/xrpl/testnet.json");
const SIGNING: &str = include_str!("../../../test-vectors/xrpl/account-set.json");

#[derive(Deserialize)]
struct LedgerVectors {
    network_id: u32,
    rows: Vec<LedgerVector>,
}

#[derive(Deserialize)]
struct LedgerVector {
    ledger_index: u32,
    hash: String,
    tx_blob: String,
    meta_blob: String,
    tx: Value,
    meta: Value,
}

#[derive(Deserialize)]
struct SigningVectors {
    entropy: [u8; 16],
    seed: String,
    address: String,
    account_id: String,
    public_key: String,
    memo_type: String,
    cases: Vec<SigningCase>,
}

#[derive(Deserialize)]
struct SigningCase {
    network_id: u32,
    memo_data: String,
    unsigned: Value,
    unsigned_blob: String,
    tx_blob: String,
    hash: String,
    tx: Value,
}

// JSON object order is immaterial; field names, values, types and array order must match.
fn assert_fields(actual: &Value, expected: &Value, path: &str) {
    if actual == expected {
        return;
    }
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (name, field) in expected {
                let got = actual
                    .get(name)
                    .unwrap_or_else(|| panic!("missing field {path}.{name}"));
                if got != field {
                    assert_fields(got, field, &format!("{path}.{name}"));
                }
            }
            for name in actual.keys() {
                assert!(expected.contains_key(name), "unexpected field {path}.{name}");
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "array length at {path}");
            for (index, (got, field)) in actual.iter().zip(expected).enumerate() {
                if got != field {
                    assert_fields(got, field, &format!("{path}[{index}]"));
                }
            }
        }
        _ => panic!("{path}: Rust {actual}; xrpl.js {expected}"),
    }
}

fn assert_decode(blob: &[u8], expected: &Value, context: &str) {
    let actual = decode(blob).unwrap_or_else(|error| panic!("{context}: {error}"));
    assert_fields(&actual, expected, context);
}

fn check_ledger_vectors(json: &str, minimum_rows: usize) {
    let corpus: LedgerVectors = serde_json::from_str(json).expect("Testnet vector JSON");
    assert_eq!(corpus.network_id, 1, "fixture must be XRPL Testnet");
    assert!(
        corpus.rows.len() >= minimum_rows,
        "insufficient transactions"
    );
    let mut hashes = BTreeSet::new();
    let mut types = BTreeSet::new();
    for row in &corpus.rows {
        let tx_type = row.tx["TransactionType"].as_str().expect("transaction type");
        types.insert(tx_type);
        assert!(hashes.insert(&row.hash), "duplicate hash {}", row.hash);
        let context = format!("ledger {} {tx_type} {}", row.ledger_index, row.hash);
        let tx_blob = hex::decode(&row.tx_blob).expect("transaction hex");
        let meta_blob = hex::decode(&row.meta_blob).expect("metadata hex");
        assert_decode(&tx_blob, &row.tx, &format!("{context}.tx"));
        assert_decode(&meta_blob, &row.meta, &format!("{context}.meta"));
    }
    assert!(types.len() >= 12, "insufficient transaction type diversity");
    println!(
        "cross-checked {} Testnet pairs of {} types",
        corpus.rows.len(),
        types.len()
    );
}

#[test]
fn representative_testnet_pairs() {
    check_ledger_vectors(TESTNET, 12);
}

#[test]
#[ignore = "generate the corpus with scripts/xrpl-cross-check.mjs and set XRPL_TESTNET_CORPUS"]
fn captured_testnet_corpus() {
    let path = std::env::var_os("XRPL_TESTNET_CORPUS").expect("set XRPL_TESTNET_CORPUS");
    let corpus = std::fs::read_to_string(path).expect("read captured Testnet corpus");
    check_ledger_vectors(&corpus, 200);
}

#[test]
fn account_set_memo_signatures() {
    let vectors: SigningVectors = serde_json::from_str(SIGNING).expect("signing vector JSON");
    let wallet = Wallet::from_entropy(&vectors.entropy);
    assert_eq!(*encode_family_seed(&vectors.entropy), vectors.seed);
    assert_eq!(*decode_family_seed(&vectors.seed).unwrap(), vectors.entropy);
    assert_eq!(Wallet::from_family_seed(&vectors.seed).unwrap().public_key(), wallet.public_key());
    assert_eq!(wallet.classic_address(), vectors.address);
    assert_eq!(hex::encode_upper(wallet.account_id()), vectors.account_id);
    assert_eq!(hex::encode_upper(wallet.public_key()), vectors.public_key);
    assert_eq!(vectors.cases.len(), 2);
    for case in vectors.cases {
        let tx = &case.unsigned;
        let unsigned_blob = hex::decode(case.unsigned_blob).expect("unsigned transaction hex");
        let signed_blob = hex::decode(case.tx_blob).expect("signed transaction hex");
        let memo_data = hex::decode(case.memo_data).expect("memo data hex");
        let network = format!("network ID {}", case.network_id);
        assert_eq!(
            encode(tx).expect("encode unsigned transaction"),
            unsigned_blob,
            "{network}"
        );
        assert_decode(&unsigned_blob, tx, &network);
        assert_decode(&signed_blob, &case.tx, &network);
        assert_eq!(
            encode(&case.tx).expect("encode signed transaction"),
            signed_blob,
            "{network}"
        );
        let signed = AccountSet {
            account: wallet.account_id(),
            sequence: tx["Sequence"].as_u64().unwrap().try_into().unwrap(),
            fee_drops: tx["Fee"].as_str().unwrap().parse().unwrap(),
            last_ledger_sequence: tx["LastLedgerSequence"]
                .as_u64()
                .unwrap()
                .try_into()
                .unwrap(),
            network_id: case.network_id,
            memo: Memo {
                kind: &vectors.memo_type,
                data: &memo_data,
            },
        }
        .sign(&wallet)
        .expect("sign AccountSet");
        assert_eq!(signed.blob, signed_blob, "{network}");
        assert_eq!(hex::encode_upper(signed.hash), case.hash, "{network}");
    }
}
