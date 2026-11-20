#![cfg(all(feature = "native", unix))]
//! Live XRPL Testnet round trip of the registry publisher and retriever. All
//! records use the `galaxy/test/*` MemoType namespace (never `galaxy/v1/*`).

use ed25519_dalek::SigningKey;
use galaxy_protocol::ids;
use galaxy_protocol::wire::{AttestationEnv, CircuitBundle, DeviceBundle, DeviceKind};
use galaxy_xrpl::dev_wallet::wallet_load;
use galaxy_xrpl::native::RpcClient;
use galaxy_xrpl::registry::{fetch_bundle, fetch_bundle_in, publish_bundle_in, BundleKind, Manifest, Namespace};
use galaxy_xrpl::submission::{FileTxStore, Submitter};
use galaxy_xrpl::{codec, LedgerSource};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Publishes (1) a 3-chunk test circuit bundle and (2) a development-unattested
/// DEVICE bundle (fresh ed25519 key whose private half is dropped, real
/// validated registration ledger, random nonce), both under `galaxy/test/*`,
/// then retrieves them with every §10 rule and checks namespace isolation.
#[tokio::test]
#[ignore = "publishes on XRPL Testnet; set GALAXY_XRPL_TESTNET_WALLET to a funded dev wallet name"]
async fn testnet_test_namespace_registry_round_trip() {
    let name = std::env::var("GALAXY_XRPL_TESTNET_WALLET").expect("set GALAXY_XRPL_TESTNET_WALLET");
    let wallet = wallet_load(&name).expect("load dev wallet");
    let rpc = RpcClient::new(["https://s.altnet.rippletest.net:51234/"], 1, false).unwrap();
    let dir = std::env::temp_dir().join(format!("galaxy-xrpl-registry-{}", std::process::id()));
    let store = FileTxStore::new(&dir).unwrap();
    let submitter = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();
    let delay = || tokio::time::sleep(Duration::from_millis(900));

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
    // Test records are never accepted as Galaxy v1 records.
    assert!(fetch_bundle(&rpc, &published.manifest_tx, 1, false).await.is_err());
    let manifest_tx = rpc.tx(&published.manifest_tx).await.unwrap().unwrap();
    let decoded = codec::decode(&manifest_tx.tx_blob).unwrap();
    assert_eq!(decoded["Memos"][0]["Memo"]["MemoType"], hex::encode_upper("galaxy/test/circuit"));

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
    std::fs::remove_dir_all(dir).unwrap();
}
