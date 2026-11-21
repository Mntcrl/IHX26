use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{extract::State, routing::post, Json, Router};
use galaxy_protocol::Hash32;
use galaxy_xrpl::{
    codec, keys,
    ledger::LedgerHeader,
    native::RpcClient,
    transaction::{AccountSet, Memo},
    LedgerSource,
};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    task::JoinHandle,
    time::{sleep, timeout},
};

use super::*;
use crate::{
    api,
    db::{Db, Range},
    discovery::{self, Identity},
};

const TESTNET: &str = include_str!("../../../deployments/networks/xrpl-testnet.toml");
const REPLAY_NETWORK: &str =
    include_str!("../../../test-vectors/verifier/dev-unattested/network.toml");
const REPLAY: &str = include_str!("../../../test-vectors/verifier/dev-unattested/recording.json");
const REFS: &str = include_str!("../../../test-vectors/verifier/dev-unattested/refs.json");

struct Fixture {
    ledgers: BTreeMap<u32, Value>,
    transactions: Vec<Value>,
    gap: AtomicBool,
    calls: std::sync::Mutex<Vec<u32>>,
}

impl Fixture {
    fn new() -> Self {
        let mut ledgers = BTreeMap::new();
        let mut transactions = Vec::new();
        for call in serde_json::from_str::<Vec<Value>>(REPLAY).unwrap() {
            match call["method"].as_str().unwrap() {
                "ledger" => {
                    ledgers.insert(
                        call["params"]["ledger_index"].as_u64().unwrap() as u32,
                        call["result"].clone(),
                    );
                }
                "tx" => transactions.push(call["result"].clone()),
                _ => (),
            }
        }
        transactions.sort_unstable_by(|a, b| {
            a["ledger_index"]
                .as_u64()
                .cmp(&b["ledger_index"].as_u64())
                .then(a["meta_blob"].as_str().cmp(&b["meta_blob"].as_str()))
        });
        Self {
            ledgers,
            transactions,
            gap: AtomicBool::new(true),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

async fn rpc(State(fixture): State<Arc<Fixture>>, Json(request): Json<Value>) -> Json<Value> {
    let params = &request["params"][0];
    assert_eq!(params["api_version"], 2);
    let response = match request["method"].as_str().unwrap() {
        "server_info" => {
            let complete = if fixture.gap.load(Ordering::Relaxed) {
                "990-1003,1005-1020"
            } else {
                "990-1020"
            };
            json!({"info":{"network_id":1,"complete_ledgers":complete,"validated_ledger":{"seq":1020}}})
        }
        "ledger" => {
            let index = params["ledger_index"].as_u64().unwrap() as u32;
            if params["transactions"] == true {
                assert_eq!(params["expand"], true);
                assert_eq!(params["binary"], true);
                fixture.calls.lock().unwrap().push(index);
                let mut response = fixture.ledgers[&index].clone();
                let mut transactions: Vec<_> = fixture
                    .transactions
                    .iter()
                    .filter(|tx| tx["ledger_index"] == index)
                    .cloned()
                    .collect();
                // Root checked here is the declared empty/nonempty distinction;
                // the transport's binary header and hash are exercised on Testnet.
                if let Some(tx) = transactions.first() {
                    response["ledger"]["transaction_hash"] = tx["hash"].clone();
                }
                transactions.reverse(); // typed backfill must restore TransactionIndex order
                response["ledger"]["transactions"] = transactions.into();
                response
            } else {
                fixture.ledgers[&index].clone()
            }
        }
        "tx" => fixture
            .transactions
            .iter()
            .find(|tx| tx["hash"] == params["transaction"])
            .cloned()
            .unwrap_or_else(|| json!({"error":"txnNotFound"})),
        "account_tx" => {
            let account = params["account"].as_str().unwrap();
            let from = params["ledger_index_min"].as_u64().unwrap();
            let through = params["ledger_index_max"].as_u64().unwrap();
            let rows: Vec<_> = fixture
                .transactions
                .iter()
                .filter(|tx| {
                    let ledger = tx["ledger_index"].as_u64().unwrap();
                    (from..=through).contains(&ledger)
                        && codec::decode(&hex::decode(tx["tx_blob"].as_str().unwrap()).unwrap())
                            .unwrap()["Account"]
                            == account
                })
                .cloned()
                .collect();
            // Emit two rows per page so account cursor continuity is exercised.
            let offset = params["marker"]["offset"].as_u64().unwrap_or(0) as usize;
            let mut result = json!({"account":account,"ledger_index_min":from,"ledger_index_max":through,
                "validated":true,"transactions":rows.iter().skip(offset).take(2).collect::<Vec<_>>()});
            if rows.len() > offset + 2 {
                result["marker"] = json!({"offset":offset + 2});
            }
            result
        }
        method => panic!("unexpected request method: {method}"),
    };
    Json(json!({"result":response}))
}

async fn bind() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}
fn address(listener: &TcpListener) -> SocketAddr {
    listener.local_addr().unwrap()
}
fn origin(listener: &TcpListener) -> String {
    format!("http://{}", address(listener))
}

fn config(dir: &Path, listen: SocketAddr, network: &str, additional: &str) -> Config {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("network.toml"), network).unwrap();
    let path = dir.join("node.toml");
    fs::write(
        &path,
        format!(
            r#"network_config = "network.toml"
data_dir = "data"
listen = "{listen}"
public_url = "http://{listen}"
allow_private_peers = true
{additional}
[limits]
sync_requests_per_second = 50
discovery_interval_secs = 5
"#
        ),
    )
    .unwrap();
    Config::load(path).unwrap()
}

struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn serve(listener: TcpListener, router: Router) -> Task {
    Task(tokio::spawn(async {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    }))
}

fn api_state(config: Config, db: Arc<Db>) -> (api::State, Arc<Identity>) {
    let identity = Arc::new(Identity::load_or_create(&config.node.data_dir).unwrap());
    (
        api::State::new(Arc::new(config), db, identity.clone()),
        identity,
    )
}

#[tokio::test]
async fn resumes_at_gap_and_serves_replayed_proof_report() {
    let dir = tempfile::tempdir().unwrap();
    let listener = bind().await;
    let fixture = Arc::new(Fixture::new());
    let network =
        REPLAY_NETWORK.replace("https://s.altnet.rippletest.net:51234/", &origin(&listener));
    let _rpc = serve(
        listener,
        Router::new()
            .route("/", post(rpc))
            .with_state(fixture.clone()),
    );
    let listener = bind().await;
    let base = origin(&listener);
    let config = config(
        dir.path(),
        address(&listener),
        &network,
        "live = false\ndiscovery = false",
    );
    let db = Db::open(&config).unwrap();
    let sources = sync::Sources::new(&config).unwrap();
    sync::catch_up(&config, &db, &sources, &sync::Limiter::new(50)).await.unwrap();
    let before = api::status_document(&config, &db).unwrap();
    drop(db);
    assert_eq!(before["gaps"], json!([{"from":1004,"through":1004}]));
    assert_eq!(before["checkpoints"], 20);
    fixture.gap.store(false, Ordering::Relaxed);
    fixture.calls.lock().unwrap().clear();
    let after = sync_once(config.clone()).await.unwrap();
    assert_eq!(*fixture.calls.lock().unwrap(), [1004]);
    assert_eq!(
        after["indexed_ranges"],
        json!([{"from":1000,"through":1020}])
    );
    assert_eq!(after["incomplete"], false);
    assert_eq!(after["checkpoints"], 21);
    assert_eq!(after["records"], 10);

    let db = Arc::new(Db::open(&config).unwrap());
    let (state, _) = api_state(config, db);
    let _api = serve(listener, api::router(state));
    let refs: Value = serde_json::from_str(REFS).unwrap();
    let photo_id = refs["photo_id"].as_str().unwrap();
    let photo: Hash32 = hex::decode(photo_id).unwrap().try_into().unwrap();
    let http = reqwest::Client::new();
    let response = http
        .get(format!("{base}/v1/photos/{photo_id}?network_id=1&limit=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let payload = response.bytes().await.unwrap();
    let page = galaxy_verifier::parse_lookup_response(&payload, &photo, 1, 1).unwrap();
    assert_eq!(page.result.candidates.len(), 1);
    assert_eq!(
        hex::encode_upper(page.result.candidates[0].manifest_tx),
        refs["manifest_tx"]
    );
    assert_eq!(
        hex::encode_upper(page.result.candidates[0].proof_tx.unwrap()),
        refs["proof_tx"]
    );
    assert!(page.next_cursor.is_none());

    let capture_id = refs["capture_id"].as_str().unwrap();
    let reports: Value = http
        .get(format!("{base}/v1/reports/{capture_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let report = &reports["reports"][0];
    assert_eq!(report["image_binding_checked"], false);
    assert_eq!(report["report_version"], "galaxy-report/v1");
    assert_eq!(
        report["report"]["state"], "UNSUPPORTED",
        "{}",
        report["report"]["summary"]
    );
    let checks = report["report"]["checks"].as_array().unwrap();
    assert!(checks
        .iter()
        .any(|check| check["id"] == "proof.groth16" && check["status"] == "pass"));
    assert!(checks
        .iter()
        .any(|check| check["id"] == "capture.photo_binding" && check["status"] == "not_applicable"));
    assert_eq!(report["proof_tx"], refs["proof_tx"]);
}

#[tokio::test]
async fn account_pages_never_claim_global_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let listener = bind().await;
    let fixture = Arc::new(Fixture::new());
    fixture.gap.store(false, Ordering::Relaxed);
    let network =
        REPLAY_NETWORK.replace("https://s.altnet.rippletest.net:51234/", &origin(&listener));
    let _rpc = serve(
        listener,
        Router::new().route("/", post(rpc)).with_state(fixture),
    );
    let listener = bind().await;
    let config = config(
        dir.path(),
        address(&listener),
        &network,
        r#"live=false
sync_mode="account"
accounts=["raJ8s1YsReiYm53wEvZnnq2wveTDaEaSL4"]"#,
    );
    let status = sync_once(config).await.unwrap();
    assert_eq!(status["indexed_ranges"], json!([]));
    assert_eq!(status["incomplete"], true);
    assert_eq!(
        status["account_coverage"][0]["indexed_ranges"],
        json!([{"from":1000,"through":1020}])
    );
    assert_eq!(status["records"], 6);
    assert_eq!(status["checkpoints"], 0);
}

#[test]
fn checkpoints_bridge_reverse_arrival_and_reject_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_path(&dir.path().join("checkpoint.sqlite3")).unwrap();
    let header = |index: u8, parent: u8| LedgerHeader {
        ledger_index: u32::from(index),
        hash: [index; 32],
        parent_hash: [parent; 32],
        close_time: 0,
        validated: true,
    };
    db.insert_ledger(&header(12, 11), &[]).unwrap();
    db.insert_ledger(&header(10, 9), &[]).unwrap();
    assert!(matches!(
        db.insert_ledger(&header(11, 8), &[]),
        Err(Error::Chain(11))
    ));
    assert_eq!(db.missing("", 10, 12).unwrap(), Some(11));
    assert_eq!(db.hash(11).unwrap(), None);
    db.insert_ledger(&header(11, 10), &[]).unwrap();
    db.insert_ledger(&header(11, 10), &[]).unwrap();
    assert_eq!(db.counts().unwrap().0, 3);
    assert_eq!(
        db.ranges("", 0, 10).unwrap(),
        [Range {
            from: 10,
            through: 12
        }]
    );
}

fn recorded_reference(fixture: &Fixture, key: &str) -> records::Record {
    let refs: Value = serde_json::from_str(REFS).unwrap();
    let tx = fixture
        .transactions
        .iter()
        .find(|tx| tx["hash"] == refs[key])
        .unwrap();
    records::parse(&galaxy_xrpl::ledger::parse_tx(tx).unwrap(), 1, None)
        .unwrap()
        .unwrap()
}

#[test]
fn conflicting_transaction_position_does_not_advance_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_path(&dir.path().join("index.sqlite3")).unwrap();
    let fixture = Fixture::new();
    let proof = recorded_reference(&fixture, "proof_tx");
    let mut conflict = recorded_reference(&fixture, "proof_tx");
    conflict.hash = hex::encode_upper([0; 32]);
    let header = LedgerHeader {
        ledger_index: proof.ledger,
        hash: [2; 32],
        parent_hash: [1; 32],
        close_time: 0,
        validated: true,
    };
    assert!(matches!(
        db.insert_ledger(&header, &[proof, conflict]),
        Err(Error::Chain(index)) if index == header.ledger_index
    ));
    assert_eq!(db.hash(header.ledger_index).unwrap(), None);
    assert!(db.ranges("", 0, 10).unwrap().is_empty());
    assert_eq!(db.counts().unwrap(), (0, 0, 0, 0));
}

#[test]
fn revocation_requeues_exact_proof_in_both_arrival_orders() {
    let fixture = Fixture::new();
    for revocation_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_path(&dir.path().join("index.sqlite3")).unwrap();
        let capture = recorded_reference(&fixture, "manifest_tx");
        let proof = recorded_reference(&fixture, "proof_tx");
        let revoke = records::Record {
            hash: hex::encode_upper([55; 32]),
            ledger: proof.ledger + 1,
            index: 0,
            account: capture.account.clone(),
            kind: records::REVOKE.into(),
            rejected: None,
            id: None,
            reference: None,
            registration: capture.registration.clone(),
            bundle: None,
            counter: None,
            announcement: None,
        };
        let store = |record: &records::Record| {
            db.insert_account_page(
                record.account.as_deref().unwrap(),
                Range {
                    from: record.ledger,
                    through: record.ledger,
                },
                std::slice::from_ref(record),
                true,
            )
            .unwrap()
        };
        let (first, second) = if revocation_first {
            (&revoke, &capture)
        } else {
            (&capture, &revoke)
        };
        store(&proof);
        store(first);
        let (hash, generation) = db.job(0).unwrap().unwrap();
        assert_eq!(hash, proof.hash);
        db.store_report(&hash, generation, 0, "galaxy-report/v1", "{}", None)
            .unwrap();
        assert!(db.job(0).unwrap().is_none());
        store(second);
        store(second); // replay is idempotent, including report invalidation
        assert_eq!(db.job(0).unwrap(), Some((hash.clone(), generation + 1)));
        // An in-flight evaluation that started before invalidation is stale.
        db.store_report(&hash, generation, 0, "galaxy-report/v1", "{}", None)
            .unwrap();
        let reports = db
            .reports(
                capture.id.as_deref().unwrap(),
                &db::Cursor(0, 0, String::new()),
                2,
            )
            .unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].report.is_none());
    }
}

#[test]
fn memo_parser_rejects_malformed_but_excludes_failed_records() {
    let wallet = keys::Wallet::from_entropy(&[42; 16]);
    let tx = AccountSet {
        account: wallet.account_id(),
        sequence: 1,
        fee_drops: 10,
        last_ledger_sequence: 1020,
        network_id: 1,
        memo: Memo {
            kind: "galaxy/v1/capture",
            data: &[1],
        },
    }
    .sign(&wallet)
    .unwrap();
    let mut record = galaxy_xrpl::ledger::LedgerTx {
        hash: tx.hash,
        tx_blob: tx.blob,
        meta_blob: Vec::new(),
        transaction_result: "tesSUCCESS".into(),
        transaction_index: 0,
        ledger_index: 1010,
        validated: true,
        close_time: None,
    };
    assert_eq!(
        crate::records::parse(&record, 1, None)
            .unwrap()
            .unwrap()
            .rejected,
        Some("invalid_payload")
    );
    record.transaction_result = "tecCLAIM".into();
    assert!(crate::records::parse(&record, 1, None).unwrap().is_none());
}

#[tokio::test]
async fn discovers_transitive_peer_from_one_bootstrap() {
    let root = tempfile::tempdir().unwrap();
    let a = bind().await;
    let b = bind().await;
    let c = bind().await;
    let (a_url, b_url, c_url) = (origin(&a), origin(&b), origin(&c));
    let c_config = config(&root.path().join("c"), address(&c), TESTNET, "");
    let b_config = config(
        &root.path().join("b"),
        address(&b),
        TESTNET,
        &format!("bootstrap_peers = [{c_url:?}]"),
    );
    let a_config = config(
        &root.path().join("a"),
        address(&a),
        TESTNET,
        &format!("bootstrap_peers = [{b_url:?}]"),
    );
    let c_db = Arc::new(Db::open(&c_config).unwrap());
    let b_db = Arc::new(Db::open(&b_config).unwrap());
    let a_db = Arc::new(Db::open(&a_config).unwrap());
    let (a_state, a_key) = api_state(a_config.clone(), a_db.clone());
    let (b_state, b_key) = api_state(b_config.clone(), b_db.clone());
    let (c_state, _) = api_state(c_config, c_db);
    let _a = serve(a, api::router(a_state));
    let _b = serve(b, api::router(b_state));
    let _c = serve(c, api::router(c_state));
    let _b_process = Task(tokio::spawn(async move {
        discovery::run(Arc::new(b_config), b_db, b_key)
            .await
            .unwrap()
    }));
    async fn wait_peers(url: &str, n: usize) -> Value {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(result) = peers(url).await {
                    if result["peers"].as_array().is_some_and(|p| p.len() == n) {
                        return result;
                    }
                }
                sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("expected discovered peers")
    }
    wait_peers(&b_url, 1).await;
    let _a_process = Task(tokio::spawn(async move {
        discovery::run(Arc::new(a_config), a_db, a_key)
            .await
            .unwrap()
    }));
    let peers = wait_peers(&a_url, 2).await;
    assert!(peers["peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["endpoints"][0] == format!("{c_url}/")));
    assert_ne!(peers["peers"][0]["node_id"], peers["peers"][1]["node_id"]);
}

#[tokio::test]
#[ignore = "requires public XRPL Testnet, enable GALAXY_NODE_TESTNET_SMOKE=1"]
async fn live_testnet_smoke() {
    if std::env::var("GALAXY_NODE_TESTNET_SMOKE").as_deref() != Ok("1") {
        panic!("set GALAXY_NODE_TESTNET_SMOKE=1 to run this network test");
    }
    let dir = tempfile::tempdir().unwrap();
    let rpc = RpcClient::new(["https://s.altnet.rippletest.net:51234/"], 1, false).unwrap();
    let initial = rpc.server_info().await.unwrap().validated_ledger;
    let inception = initial - 4;
    let network = TESTNET
        .lines()
        .map(|line| {
            if line.starts_with("inception_ledger = ") {
                format!("inception_ledger = {inception}")
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let listener = bind().await;
    let (addr, url) = (address(&listener), origin(&listener));
    drop(listener);
    let config = config(dir.path(), addr, &network, "discovery = false");
    let backfill = sync_once(config.clone()).await.unwrap();
    assert_eq!(backfill["indexed_from"], inception);
    assert!(backfill["checkpoints"].as_u64().unwrap() >= 5);
    let node = Task(tokio::spawn(async move { run(config).await.unwrap() }));
    sleep(Duration::from_secs(120)).await;
    let report = status(&url).await.unwrap();
    assert_eq!(report["indexed_from"], inception);
    assert_eq!(report["indexed_ranges"].as_array().unwrap().len(), 1);
    assert!(report["indexed_through"].as_u64().unwrap() > u64::from(initial));
    assert!(report["live_connections"].as_u64().unwrap() >= 1);
    assert!(report["live_ledger_events"].as_u64().unwrap() >= 10);
    println!("Testnet backfill: {}", backfill);
    println!("Testnet after 120 seconds: {}", report);
    drop(node);
}

#[tokio::test]
async fn photo_pagination_and_invalid_queries_use_stable_errors() {
    let root = tempfile::tempdir().unwrap();
    let listener = bind().await;
    let base = origin(&listener);
    let config = config(root.path(), address(&listener), REPLAY_NETWORK, "");
    let db = Arc::new(Db::open(&config).unwrap());
    let photo_id = [0xa1; 32];
    let ledger = 1000;
    let make_capture = |n| crate::records::Record {
        hash: hex::encode_upper([n; 32]),
        ledger,
        index: u32::from(n),
        account: Some(keys::classic_address(&[1; 20])),
        kind: crate::records::CAPTURE.into(),
        rejected: None,
        id: Some(hex::encode([n; 32])),
        reference: Some(hex::encode(photo_id)),
        registration: Some(hex::encode_upper([9; 32])),
        bundle: None,
        counter: None,
        announcement: None,
    };
    db.insert_ledger(
        &LedgerHeader {
            ledger_index: ledger,
            hash: [5; 32],
            parent_hash: [4; 32],
            close_time: 0,
            validated: true,
        },
        &[make_capture(2), make_capture(4)],
    )
    .unwrap();
    db.put_tip(1002).unwrap();
    let (state, _) = api_state(config, db);
    let _api = serve(listener, api::router(state));
    let url = format!(
        "{base}/v1/photos/{}?network_id=1&limit=1",
        hex::encode(photo_id)
    );
    let http = reqwest::Client::new();
    let first = http.get(&url).send().await.unwrap().bytes().await.unwrap();
    let first = galaxy_verifier::parse_lookup_response(&first, &photo_id, 1, 1).unwrap();
    assert!(first.result.incomplete);
    assert_eq!(first.result.candidates[0].manifest_tx, [2; 32]);
    let cursor = first.next_cursor.unwrap();
    let second = http
        .get(format!("{url}&cursor={cursor}"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let second = galaxy_verifier::parse_lookup_response(&second, &photo_id, 1, 1).unwrap();
    assert_eq!(second.result.candidates[0].manifest_tx, [4; 32]);
    assert!(second.next_cursor.is_none());
    let bad = http
        .get(format!(
            "{base}/v1/photos/{}?network_id=2",
            hex::encode(photo_id)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        bad.json::<Value>().await.unwrap()["error"]["code"],
        "wrong_network"
    );
}

#[test]
fn descriptor_signature_binds_fields_and_checks_expiry() {
    let root = tempfile::tempdir().unwrap();
    let config = config(
        root.path(),
        "127.0.0.1:8000".parse().unwrap(),
        REPLAY_NETWORK,
        "",
    );
    let db = Db::open(&config).unwrap();
    let key = Identity::load_or_create(&config.node.data_dir).unwrap();
    let descriptor = key.descriptor(&config, &db).unwrap();
    assert!(descriptor.verify(&config, unix_now()).is_ok());
    assert!(descriptor.verify(&config, descriptor.expires_at()).is_err());
    let mut encoded = serde_json::to_value(descriptor).unwrap();
    encoded["software_version"] = "9.0.0".into();
    let changed: crate::discovery::Descriptor = serde_json::from_value(encoded).unwrap();
    assert!(matches!(
        changed.verify(&config, unix_now()),
        Err(Error::Peer("invalid descriptor signature"))
    ));
}
