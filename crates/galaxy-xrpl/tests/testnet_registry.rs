#![cfg(all(feature = "native", unix))]
//! Live XRPL Testnet round trip of the registry publisher and retriever. All
//! records use the `galaxy/test/*` MemoType namespace (never `galaxy/v1/*`).

use ed25519_dalek::SigningKey;
use galaxy_protocol::ids;
use galaxy_protocol::wire::{AttestationEnv, ChunkMemo, CircuitBundle, CircuitManifest, DeviceBundle, DeviceKind};
use galaxy_xrpl::dev_wallet::wallet_load;
use galaxy_xrpl::keys::Wallet;
use galaxy_xrpl::ledger::{AccountTxPage, LedgerHeader, LedgerTx, RpcTransport, ServerInfo};
use galaxy_xrpl::native::RpcClient;
use galaxy_xrpl::registry::{fetch_bundle, fetch_bundle_in, publish_bundle_in, BundleKind, Manifest, Namespace};
use galaxy_xrpl::submission::{account_info_params, autofill_from_responses, FileTxStore, PendingTx, Submitter, TxState, TxStore};
use galaxy_xrpl::transaction::{decode_galaxy_memo, sign_transaction, AccountSet, Memo};
use galaxy_xrpl::{codec, AccountId, Error, Hash32, LedgerSource};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

async fn delay() { tokio::time::sleep(Duration::from_millis(900)).await; }

async fn test_record(
    submitter: &Submitter<'_, RpcClient, FileTxStore>, wallet: &Wallet, purpose: &str,
    kind: &str, data: &[u8],
) -> LedgerTx {
    let memo = Memo { kind, data };
    let pending = submitter.prepare(purpose, wallet, memo, None).await.unwrap();
    let TxState::Validated(tx) = submitter.run(&pending, delay).await.unwrap() else {
        panic!("test record did not validate");
    };
    assert_eq!(tx.transaction_result, "tesSUCCESS");
    tx
}

async fn failed_chunk(rpc: &RpcClient, store: &FileTxStore, submitter: &Submitter<'_, RpcClient, FileTxStore>,
    wallet: &Wallet, purpose: &str, kind: &str, data: &[u8],
) -> LedgerTx {
    let head = submitter.validated_ledger().await.unwrap();
    let address = wallet.classic_address();
    let fee = rpc.request("fee", json!({})).await.unwrap();
    let account = rpc.request("account_info", account_info_params(&address, head)).await.unwrap();
    let fields = autofill_from_responses(head, None, 1000, &address, &fee, &account).unwrap();
    let mut entropy = Zeroizing::new([0u8; 16]);
    getrandom::fill(&mut entropy[..]).unwrap();
    let destination = Wallet::from_entropy(&entropy).classic_address();
    // A one-drop payment to this fresh destination claims a fee but fails.
    let memo = Memo { kind, data };
    let signed = sign_transaction(json!({
        "TransactionType": "Payment", "Account": address, "Destination": destination, "Amount": "1",
        "Sequence": fields.sequence, "Fee": fields.fee_drops.to_string(),
        "LastLedgerSequence": fields.last_ledger_sequence, "Memos": memo.to_json().unwrap(),
    }), wallet).unwrap();
    let pending = PendingTx::new(purpose, signed, 1, &fields).unwrap();
    assert!(store.compare_and_swap(None, &pending).unwrap());
    let TxState::Validated(tx) = submitter.run(&pending, delay).await.unwrap() else {
        panic!("failed chunk did not validate");
    };
    assert_eq!(tx.transaction_result, "tecNO_DST_INSUF_XRP");
    tx
}

async fn reject_invalid_registry_records(
    rpc: &RpcClient, store: &FileTxStore, submitter: &Submitter<'_, RpcClient, FileTxStore>,
    wallet: &Wallet, good: &CircuitManifest, bytes: &[u8],
) {
    let kind = BundleKind::Circuit;
    let chunk_type = kind.chunk_type(Namespace::Test);
    let manifest_type = kind.manifest_type(Namespace::Test);
    let scope = format!("testnet/reject/{}", hex::encode(good.bundle_id));
    let mut chunk = ChunkMemo {
        bundle_id: good.bundle_id, index: 0, total: good.chunk_total, payload: bytes[..800].to_vec(),
    };
    chunk.payload[0] ^= 1;
    let conflict = test_record(submitter, wallet, &format!("{scope}/conflict-chunk"), chunk_type, &chunk.encode()).await;
    chunk.index = good.chunk_total;
    let index = test_record(submitter, wallet, &format!("{scope}/index-chunk"), chunk_type, &chunk.encode()).await;
    chunk.index = 0;
    chunk.total = 1;
    chunk.payload = vec![42];
    let digest = test_record(submitter, wallet, &format!("{scope}/digest-chunk"), chunk_type, &chunk.encode()).await;
    let failed = failed_chunk(rpc, store, submitter, wallet, &format!("{scope}/failed-chunk"), chunk_type, &chunk.encode()).await;

    for (label, manifest, expected) in [
        ("conflict", CircuitManifest { last_chunk_ledger: conflict.ledger_index, ..good.clone() }, "conflicting registry chunk"),
        ("index", CircuitManifest { first_chunk_ledger: index.ledger_index, last_chunk_ledger: index.ledger_index, ..good.clone() }, "chunk index or total"),
        ("digest", CircuitManifest { chunk_total: 1, bundle_len: 1, first_chunk_ledger: digest.ledger_index, last_chunk_ledger: digest.ledger_index, ..good.clone() }, "bundle digest mismatch"),
        ("failed", CircuitManifest { chunk_total: 1, bundle_len: 1, first_chunk_ledger: failed.ledger_index, last_chunk_ledger: failed.ledger_index, ..good.clone() }, "registry record is failed"),
        ("gap", CircuitManifest { last_chunk_ledger: good.first_chunk_ledger, ..good.clone() }, "missing chunks"),
        ("wrong-id", CircuitManifest { bundle_id: [0x33; 32], ..good.clone() }, "missing chunks"),
        ("oversize", CircuitManifest { bundle_len: 1024 * 1024 + 1, ..good.clone() }, "bundle chunk span"),
    ] {
        let tx = test_record(submitter, wallet, &format!("{scope}/{label}-manifest"), manifest_type, &manifest.encode()).await;
        let error = fetch_bundle_in(rpc, &tx.hash, 1, false, Namespace::Test).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{label}: {error}");
        println!("rejected {label} manifest {}: {error}", hex::encode_upper(tx.hash));
    }
}

// An honest account_tx cannot return an unvalidated AccountSet or an
// AccountSet from another account. Exercise those checks by altering only
// one matching row of the otherwise live Testnet response.
struct AlteredHistory<'a> {
    rpc: &'a RpcClient,
    target: Hash32,
    alter: fn(&mut LedgerTx),
}

impl LedgerSource for AlteredHistory<'_> {
    async fn tx(&self, hash: &Hash32) -> Result<Option<LedgerTx>, Error> {
        self.rpc.tx(hash).await
    }

    async fn ledger_header(&self, index: u32) -> Result<LedgerHeader, Error> {
        self.rpc.ledger_header(index).await
    }

    async fn server_info(&self) -> Result<ServerInfo, Error> {
        self.rpc.server_info().await
    }

    async fn account_tx(&self, account: &AccountId, min: u32, max: u32, marker: Option<Value>, limit: u16)
        -> Result<AccountTxPage, Error>
    {
        let mut page = self.rpc.account_tx(account, min, max, marker, limit).await?;
        for tx in page.transactions.iter_mut().filter(|tx| tx.hash == self.target) {
            (self.alter)(tx);
        }
        Ok(page)
    }
}

fn wrong_account(tx: &mut LedgerTx) {
    let memo = decode_galaxy_memo(&tx.tx_blob).unwrap();
    let wallet = Wallet::from_entropy(&[9; 16]);
    let replacement = AccountSet {
        account: wallet.account_id(), fee_drops: 10, sequence: 1, last_ledger_sequence: tx.ledger_index,
        network_id: 1, memo: Memo { kind: &memo.kind, data: &memo.data },
    }.sign(&wallet).unwrap();
    (tx.tx_blob, tx.hash) = (replacement.blob, replacement.hash);
}

fn unvalidated(tx: &mut LedgerTx) { tx.validated = false; }

/// Publishes (1) a 3-chunk test circuit bundle and (2) a development-unattested
/// DEVICE bundle (fresh ed25519 key whose private half is dropped, real
/// validated registration ledger, random nonce), both under `galaxy/test/*`,
/// then retrieves them and exercises rejection rules with validated Testnet records.
#[tokio::test]
#[ignore = "publishes on XRPL Testnet; set GALAXY_XRPL_TESTNET_WALLET to a funded dev wallet name"]
async fn testnet_test_namespace_registry_round_trip() {
    let name = std::env::var("GALAXY_XRPL_TESTNET_WALLET").expect("set GALAXY_XRPL_TESTNET_WALLET");
    let wallet = wallet_load(&name).expect("load dev wallet");
    let rpc = RpcClient::new(["https://s.altnet.rippletest.net:51234/"], 1, false).unwrap();
    let dir = std::env::temp_dir().join(format!("galaxy-xrpl-registry-{}", std::process::id()));
    let store = FileTxStore::new(&dir).unwrap();
    let submitter = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();

    let mut vk = vec![0u8; 2000];
    getrandom::fill(&mut vk).unwrap();
    let circuit = CircuitBundle {
        descriptor_json: br#"{"name":"galaxy-test-registry","purpose":"testnet registry exercise"}"#.to_vec(),
        vk_bytes: vk,
        policy_json: br#"{"test":true}"#.to_vec(),
    };
    let circuit_bytes = circuit.encode();
    let started = Instant::now();
    let published = publish_bundle_in(&submitter, &wallet, Namespace::Test, BundleKind::Circuit, &circuit_bytes, delay)
        .await.unwrap();
    let circuit_ms = started.elapsed().as_millis();
    assert_eq!(published.chunk_txs.len(), circuit_bytes.len().div_ceil(800));
    let fetched = fetch_bundle_in(&rpc, &published.manifest_tx, 1, false, Namespace::Test).await.unwrap();
    assert_eq!(fetched.bytes, circuit_bytes);
    let Manifest::Circuit(manifest) = &fetched.manifest else { panic!("expected a CIRCUIT manifest") };
    assert_eq!(manifest.vk_digest, ids::vk_digest(&circuit.vk_bytes));
    let circuit_manifest = manifest.clone();
    // Test records are never accepted as Galaxy v1 records.
    assert!(fetch_bundle(&rpc, &published.manifest_tx, 1, false).await.is_err());
    let manifest_tx = rpc.tx(&published.manifest_tx).await.unwrap().unwrap();
    let decoded = codec::decode(&manifest_tx.tx_blob).unwrap();
    assert_eq!(decoded["Memos"][0]["Memo"]["MemoType"], hex::encode_upper("galaxy/test/circuit"));
    for (alter, expected) in [
        (wrong_account as fn(&mut LedgerTx), "wrong type, account or ledger order"),
        (unvalidated, "registry record is failed, unvalidated or hash-mismatched"),
    ] {
        let history = AlteredHistory { rpc: &rpc, target: published.chunk_txs[0], alter };
        let error = fetch_bundle_in(&history, &published.manifest_tx, 1, false, Namespace::Test).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        println!("rejected altered history row for {}: {error}", hex::encode_upper(published.chunk_txs[0]));
    }

    let head = submitter.validated_ledger().await.unwrap();
    let header = rpc.ledger_header(head).await.unwrap();
    assert!(header.validated && header.ledger_index == head);
    let mut secret = Zeroizing::new([0; 32]);
    getrandom::fill(&mut secret[..]).unwrap();
    let public_key = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
    let mut reg_nonce = [0; 32];
    getrandom::fill(&mut reg_nonce).unwrap();
    let device = DeviceBundle {
        kind: DeviceKind::DevUnattested,
        env: AttestationEnv::NotApplicable,
        app_id: "GALAXYDEV.org.xrpl-galaxy.development".into(),
        key_id: ids::dev_key_id(&public_key),
        network: 1,
        reg_ledger_index: head,
        reg_ledger_hash: header.hash,
        reg_nonce,
        account_id: wallet.account_id(),
        attestation: public_key.to_vec(),
    };
    let device_bytes = device.encode();
    let started = Instant::now();
    let device_published = publish_bundle_in(&submitter, &wallet, Namespace::Test, BundleKind::Device, &device_bytes, delay)
        .await.unwrap();
    let device_ms = started.elapsed().as_millis();
    let device_fetched = fetch_bundle_in(&rpc, &device_published.manifest_tx, 1, false, Namespace::Test).await.unwrap();
    assert_eq!(device_fetched.bytes, device_bytes);
    assert_eq!(device_fetched.account, wallet.account_id());
    let Manifest::Device(manifest) = &device_fetched.manifest else { panic!("expected a DEVICE manifest") };
    assert_eq!((manifest.key_id, manifest.reg_ledger_index), (device.key_id, head));
    assert!(fetch_bundle(&rpc, &device_published.manifest_tx, 1, false).await.is_err());

    println!("{}", serde_json::json!({
        "circuit": {
            "manifest_tx": hex::encode_upper(published.manifest_tx),
            "chunk_txs": published.chunk_txs.iter().map(hex::encode_upper).collect::<Vec<_>>(),
            "bundle_bytes": circuit_bytes.len(),
            "manifest_ledger": fetched.manifest_ledger,
            "publish_ms": circuit_ms,
        },
        "device": {
            "manifest_tx": hex::encode_upper(device_published.manifest_tx),
            "chunk_txs": device_published.chunk_txs.iter().map(hex::encode_upper).collect::<Vec<_>>(),
            "bundle_bytes": device_bytes.len(),
            "reg_ledger": head,
            "manifest_ledger": device_fetched.manifest_ledger,
            "publish_ms": device_ms,
        },
    }));
    reject_invalid_registry_records(&rpc, &store, &submitter, &wallet, &circuit_manifest, &circuit_bytes).await;
    std::fs::remove_dir_all(dir).unwrap();
}
