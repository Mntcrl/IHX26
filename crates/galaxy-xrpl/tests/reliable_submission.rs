#![cfg(all(feature = "native", unix))]

use galaxy_xrpl::keys::Wallet;
use galaxy_xrpl::ledger::RpcTransport;
use galaxy_xrpl::submission::{classify_result, FileTxStore, ResultClass, Submitter, TxState, TxStore};
use galaxy_xrpl::transaction::Memo;
use galaxy_xrpl::Error;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::Mutex;

struct TestRpc {
    head: AtomicU32,
    fee: AtomicU32,
    fully_searched: AtomicBool,
    submitted: Mutex<Vec<String>>,
}

impl RpcTransport for TestRpc {
    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        Ok(match method {
            "server_info" => json!({"status":"success", "info": {
                "network_id":1, "complete_ledgers":"1-200", "validated_ledger":{"seq":self.head.load(Relaxed)}
            }}),
            "fee" => json!({"drops":{"open_ledger_fee":self.fee.load(Relaxed).to_string()}}),
            "account_info" => json!({"validated":true,"ledger_index":params["ledger_index"],
                "account_data":{"Account":params["account"],"Sequence":21}}),
            "submit" => {
                self.submitted.lock().unwrap().push(params["tx_blob"].as_str().unwrap().to_owned());
                json!({"engine_result":"tesSUCCESS", "tx_blob":params["tx_blob"]})
            }
            "tx" => {
                let searched = if params.get("min_ledger").is_some() {
                    assert_eq!(params["min_ledger"], 101);
                    assert_eq!(params["max_ledger"], 107);
                    self.fully_searched.load(Relaxed)
                } else { false };
                json!({"error":"txnNotFound","searched_all":searched})
            }
            _ => panic!("unexpected method {method}"),
        })
    }
}

#[tokio::test]
async fn persist_recover_and_replace_only_after_complete_expiry_search() {
    let rpc = TestRpc { head: AtomicU32::new(100), fee: AtomicU32::new(10), fully_searched: AtomicBool::new(false),
        submitted: Mutex::new(Vec::new()) };
    let wallet = Wallet::from_entropy(&[0x48; 16]);
    let parent = std::env::temp_dir().join(format!("galaxy-tx-test-{}", std::process::id()));
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("nested").join("journal");
    let memo = Memo { kind: "galaxy/test/store", data: b"journal" };
    let tx = {
        let store = FileTxStore::new(&path).unwrap();
        let sm = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();
        rpc.fee.store(1001, Relaxed);
        assert!(sm.prepare("same", &wallet, memo, Some(107)).await.is_err());
        assert!(store.load("same").unwrap().is_none());
        rpc.fee.store(10, Relaxed);
        let p = sm.prepare("same", &wallet, memo, Some(107)).await.unwrap();
        assert_eq!(p.last_ledger, 107);
        assert_eq!(p.first_possible_ledger, 101);
        assert_eq!(store.load("same").unwrap().unwrap(), p);
        assert!(!store.compare_and_swap(None, &p).unwrap());
        let mut not_stored = p.clone();
        not_stored.purpose = "different".into();
        assert!(sm.submit_saved(&not_stored).await.is_err());
        assert_eq!(sm.submit_saved(&p).await.unwrap().class, ResultClass::Success);
        p
    };
    let store = FileTxStore::new(&path).unwrap();
    let sm = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();
    assert_eq!(sm.prepare("same", &wallet, memo, Some(107)).await.unwrap(), tx);
    assert!(sm.prepare("same", &wallet, memo, Some(106)).await.is_err());
    assert!(sm.prepare("same", &wallet, Memo { data: b"other", ..memo }, None).await.is_err());
    assert!(matches!(sm.state(&tx).await.unwrap(), TxState::Pending));
    sm.submit_saved(&tx).await.unwrap();
    assert_eq!(*rpc.submitted.lock().unwrap(), vec![hex::encode_upper(&tx.blob); 2]);
    rpc.head.store(108, Relaxed);
    assert!(sm.state(&tx).await.is_err());
    assert!(sm.replace_expired("same", &wallet, memo, None).await.is_err());
    rpc.fully_searched.store(true, Relaxed);
    assert!(matches!(sm.state(&tx).await.unwrap(), TxState::Expired));
    assert!(sm.submit_saved(&tx).await.is_err());
    let replacement = sm.replace_expired("same", &wallet, memo, None).await.unwrap();
    assert_ne!(tx.hash, replacement.hash);
    assert_eq!(replacement.last_ledger, 128);
    assert_eq!(store.load("same").unwrap().unwrap(), replacement);
    std::fs::remove_dir_all(parent).unwrap();
}

#[test]
fn result_classes() {
    assert_eq!(classify_result("tesSUCCESS"), ResultClass::Success);
    assert_eq!(classify_result("tecNO_DST_INSUF_XRP"), ResultClass::ClaimedFee);
    assert_eq!(classify_result("tefMAX_LEDGER"), ResultClass::Failure);
    assert_eq!(classify_result("temMALFORMED"), ResultClass::Malformed);
    assert_eq!(classify_result("telINSUF_FEE_P"), ResultClass::Local);
    assert_eq!(classify_result("terQUEUED"), ResultClass::Retry);
    assert_eq!(classify_result("tesNONSENSE"), ResultClass::Unknown);
}

/// Applies the second submission of a blob (the first gets a provisional
/// `telCAN_NOT_QUEUE`) one ledger later with `code`.
struct FlowRpc {
    head: AtomicU32,
    code: u8,
    submitted: Mutex<Vec<String>>,
    applied: Mutex<Option<(String, u32)>>,
}

impl RpcTransport for FlowRpc {
    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let head = self.head.load(Relaxed);
        Ok(match method {
            "server_info" => json!({"status":"success","info":{"network_id":1,"complete_ledgers":"1-900","validated_ledger":{"seq":head}}}),
            "fee" => json!({"drops":{"open_ledger_fee":"12"}}),
            "account_info" => json!({"validated":true,"ledger_index":params["ledger_index"],
                "account_data":{"Account":params["account"],"Sequence":5}}),
            "submit" => {
                let blob = params["tx_blob"].as_str().unwrap().to_owned();
                let mut submitted = self.submitted.lock().unwrap();
                submitted.push(blob.clone());
                if submitted.len() == 1 {
                    json!({"engine_result":"telCAN_NOT_QUEUE","tx_blob":blob})
                } else {
                    self.applied.lock().unwrap().get_or_insert((blob.clone(), head + 1));
                    json!({"engine_result":"tesSUCCESS","tx_blob":blob})
                }
            }
            "tx" => match self.applied.lock().unwrap().clone() {
                Some((blob, ledger)) if head >= ledger => {
                    let hash = hex::encode_upper(galaxy_xrpl::keys::tx_hash(&hex::decode(&blob).unwrap()));
                    assert_eq!(params["transaction"], hash);
                    json!({"tx_blob":blob,"hash":hash,"validated":true,"ledger_index":ledger,
                        "meta_blob":format!("201C00000002F8F10310{:02X}", self.code)})
                }
                _ => json!({"error":"txnNotFound","searched_all":false}),
            },
            _ => panic!("unexpected method {method}"),
        })
    }
}

#[tokio::test]
async fn crash_recovery_rebroadcasts_the_identical_blob_and_classifies_results() {
    use galaxy_xrpl::submission::REBROADCAST_POLLS;
    let wallet = Wallet::from_entropy(&[0x49; 16]);
    let dir = std::env::temp_dir().join(format!("galaxy-tx-flow-{}", std::process::id()));
    let memo = Memo { kind: "galaxy/test/flow", data: b"flow" };
    for (code, success) in [(0u8, true), (125u8, false)] {
        let rpc = FlowRpc { head: AtomicU32::new(300), code, submitted: Mutex::new(Vec::new()), applied: Mutex::new(None) };
        let purpose = format!("flow-{code}");
        // Signed and persisted, then the process stops before submitting.
        let persisted = {
            let store = FileTxStore::new(&dir).unwrap();
            Submitter::new(&rpc, &store, 1, false, 1000).unwrap().prepare(&purpose, &wallet, memo, None).await.unwrap()
        };
        assert!(rpc.submitted.lock().unwrap().is_empty());
        rpc.head.fetch_add(3, Relaxed);
        let store = FileTxStore::new(&dir).unwrap();
        let sm = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();
        let recovered = sm.prepare(&purpose, &wallet, memo, None).await.unwrap();
        assert_eq!(recovered, persisted);
        let polls = AtomicU32::new(0);
        let state = sm.run(&recovered, || async { polls.fetch_add(1, Relaxed); rpc.head.fetch_add(1, Relaxed); }).await.unwrap();
        assert_eq!(state.succeeded(), success);
        let TxState::Validated(tx) = state else { panic!("expected a validated transaction") };
        assert_eq!(classify_result(&tx.transaction_result), if success { ResultClass::Success } else { ResultClass::ClaimedFee });
        assert_eq!(tx.tx_blob, persisted.blob);
        // Rebroadcast at the REBROADCAST_POLLS-th pending poll; validated one ledger later.
        assert_eq!(polls.load(Relaxed), REBROADCAST_POLLS);
        // Exactly two broadcasts, both of the persisted bytes.
        assert_eq!(*rpc.submitted.lock().unwrap(), vec![hex::encode_upper(&persisted.blob); 2]);
        // A repeated run (duplicate request) observes the final state without submitting again.
        assert!(matches!(sm.run(&recovered, || async {}).await.unwrap(), TxState::Validated(_)));
        assert_eq!(rpc.submitted.lock().unwrap().len(), 2);
        // A validated purpose is never re-signed.
        assert!(sm.replace_expired(&purpose, &wallet, memo, None).await.is_err());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
