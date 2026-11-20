#![cfg(all(feature = "native", unix))]
//! Live XRPL Testnet round trip of the registry publisher and retriever.

use ed25519_dalek::SigningKey;
use galaxy_protocol::ids;
use galaxy_protocol::wire::{AttestationEnv, DeviceBundle, DeviceKind};
use galaxy_xrpl::dev_wallet::wallet_load;
use galaxy_xrpl::native::RpcClient;
use galaxy_xrpl::registry::{fetch_bundle, publish_bundle, BundleKind, Manifest};
use galaxy_xrpl::submission::{FileTxStore, Submitter};
use galaxy_xrpl::LedgerSource;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Publishes a development-unattested DEVICE registration (spec §10/§11): a
/// genuine ed25519 public key, a real validated registration ledger and a random
/// nonce. The private key is dropped after the test, so the registration can never
/// back a capture; development credentials never reach VERIFIED in any case.
#[tokio::test]
#[ignore = "publishes on XRPL Testnet; set GALAXY_XRPL_TESTNET_WALLET to a funded dev wallet name"]
async fn testnet_dev_registration_round_trip() {
    let name = std::env::var("GALAXY_XRPL_TESTNET_WALLET").expect("set GALAXY_XRPL_TESTNET_WALLET");
    let wallet = wallet_load(&name).expect("load dev wallet");
    let rpc = RpcClient::new(["https://s.altnet.rippletest.net:51234/"], 1, false).unwrap();
    let dir = std::env::temp_dir().join(format!("galaxy-xrpl-registry-{}", std::process::id()));
    let store = FileTxStore::new(&dir).unwrap();
    let submitter = Submitter::new(&rpc, &store, 1, false, 1000).unwrap();

    let head = submitter.validated_ledger().await.unwrap();
    let header = rpc.ledger_header(head).await.unwrap();
    assert!(header.validated && header.ledger_index == head);
    let mut secret = Zeroizing::new([0; 32]);
    getrandom::fill(&mut secret[..]).unwrap();
    let public_key = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
    let mut reg_nonce = [0; 32];
    getrandom::fill(&mut reg_nonce).unwrap();
    let bundle = DeviceBundle {
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
    let bytes = bundle.encode();

    let started = Instant::now();
    let published = publish_bundle(&submitter, &wallet, BundleKind::Device, &bytes,
        || tokio::time::sleep(Duration::from_millis(900))).await.unwrap();
    let publish_ms = started.elapsed().as_millis();
    let fetched = fetch_bundle(&rpc, &published.manifest_tx, 1, false).await.unwrap();
    assert_eq!(fetched.bytes, bytes);
    assert_eq!(fetched.account, wallet.account_id());
    let Manifest::Device(manifest) = &fetched.manifest else { panic!("expected a DEVICE manifest") };
    assert_eq!((manifest.key_id, manifest.reg_ledger_index), (bundle.key_id, head));
    println!("{}", serde_json::json!({
        "manifest_tx": hex::encode_upper(published.manifest_tx),
        "chunk_txs": published.chunk_txs.iter().map(hex::encode_upper).collect::<Vec<_>>(),
        "bundle_bytes": bytes.len(),
        "reg_ledger": head,
        "manifest_ledger": fetched.manifest_ledger,
        "publish_ms": publish_ms,
    }));
    std::fs::remove_dir_all(dir).unwrap();
}
