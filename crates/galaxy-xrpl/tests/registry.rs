#![cfg(feature = "native")]

use galaxy_protocol::{ids, wire::{ChunkMemo, CircuitBundle, CircuitManifest, MemoKind}};
use galaxy_xrpl::registry::{fetch_bundle_in, BundleKind, Namespace};
use galaxy_xrpl::keys::Wallet;
use galaxy_xrpl::ledger::{AccountTxPage, LedgerHeader, LedgerTx, ServerInfo};
use galaxy_xrpl::registry::fetch_bundle;
use galaxy_xrpl::transaction::{AccountSet, Memo};
use galaxy_xrpl::{AccountId, Error, Hash32, LedgerSource};
use serde_json::{json, Value};

struct History {
    manifest: LedgerTx,
    chunk0: LedgerTx,
    chunk1: LedgerTx,
    extra: Option<LedgerTx>,
    complete: &'static str,
}

impl LedgerSource for History {
    async fn tx(&self, hash: &Hash32) -> Result<Option<LedgerTx>, Error> {
        Ok((hash == &self.manifest.hash).then(|| self.manifest.clone()))
    }
    async fn ledger_header(&self, _: u32) -> Result<LedgerHeader, Error> { panic!("unexpected header query") }
    async fn account_tx(&self, _: &AccountId, min: u32, max: u32, marker: Option<Value>, _: u16) -> Result<AccountTxPage, Error> {
        assert_eq!((min, max), (100, 102));
        if let Some(marker) = marker {
            assert_eq!(marker, json!({"ledger": 102, "seq": 0}));
            let mut transactions = vec![self.chunk1.clone()];
            transactions.extend(self.extra.clone());
            Ok(AccountTxPage { transactions, marker: None })
        } else {
            Ok(AccountTxPage { transactions: vec![self.chunk0.clone()], marker: Some(json!({"ledger":102,"seq":0})) })
        }
    }
    async fn server_info(&self) -> Result<ServerInfo, Error> {
        Ok(ServerInfo { network_id: 1, validated_ledger: 105, complete_ledgers: self.complete.to_owned() })
    }
}

fn record(wallet: &Wallet, kind: MemoKind, data: &[u8], ledger_index: u32) -> LedgerTx {
    record_typed(wallet, kind.as_str(), data, ledger_index)
}

fn record_typed(wallet: &Wallet, kind: &str, data: &[u8], ledger_index: u32) -> LedgerTx {
    let tx = AccountSet { account:wallet.account_id(), sequence:1, fee_drops:10, last_ledger_sequence:105,
        network_id:1, memo:Memo { kind, data } }.sign(wallet).unwrap();
    LedgerTx { hash:tx.hash, tx_blob:tx.blob, meta_blob:Vec::new(), transaction_result:"tesSUCCESS".into(),
        transaction_index:0, ledger_index, validated:true, close_time:None }
}

#[tokio::test]
async fn test_namespace_is_isolated_and_ledger_order_is_enforced() {
    let bundle = CircuitBundle { descriptor_json: br#"{"ns":"test"}"#.to_vec(), vk_bytes: vec![2; 900], policy_json: b"{}".to_vec() };
    let bytes = bundle.encode();
    let id = ids::bundle_id(&bytes);
    let wallet = Wallet::from_entropy(&[10;16]);
    let chunk = |index: u16, range: std::ops::Range<usize>| ChunkMemo { bundle_id:id, index, total:2, payload:bytes[range].to_vec() }.encode();
    let manifest = CircuitManifest { network:1, bundle_id:id, bundle_len:bytes.len() as u32, chunk_total:2,
        first_chunk_ledger:100, last_chunk_ledger:102, circuit_id:ids::circuit_id(&bundle.descriptor_json).unwrap(),
        policy_id:ids::policy_id(&bundle.policy_json).unwrap(), vk_digest:ids::vk_digest(&bundle.vk_bytes) }.encode();
    let (c, cc) = (BundleKind::Circuit.manifest_type(Namespace::Test), BundleKind::Circuit.chunk_type(Namespace::Test));
    assert_eq!((c, cc), ("galaxy/test/circuit", "galaxy/test/circuit-chunk"));
    let mut history = History { manifest: record_typed(&wallet, c, &manifest, 103),
        chunk0: record_typed(&wallet, cc, &chunk(0, 0..800), 100),
        chunk1: record_typed(&wallet, cc, &chunk(1, 800..bytes.len()), 102), extra: None, complete: "90-105" };
    let hash = history.manifest.hash;
    assert_eq!(fetch_bundle_in(&history, &hash, 1, false, Namespace::Test).await.unwrap().bytes, bytes);
    // A test manifest is never a v1 manifest.
    assert!(fetch_bundle(&history, &hash, 1, false).await.unwrap_err().to_string().contains("unexpected registry manifest"));
    // v1 chunks are ignored by a test manifest (bundle incomplete).
    history.chunk1 = record(&wallet, MemoKind::CircuitChunk, &chunk(1, 800..bytes.len()), 102);
    assert!(fetch_bundle_in(&history, &hash, 1, false, Namespace::Test).await.unwrap_err().to_string().contains("missing chunks"));
    // A chunk at or after the manifest position is rejected.
    history.chunk1 = record_typed(&wallet, cc, &chunk(1, 800..bytes.len()), 102);
    history.manifest.ledger_index = 102;
    assert!(fetch_bundle_in(&history, &hash, 1, false, Namespace::Test).await.is_err());
    history.manifest.ledger_index = 103;
    // Endpoint ledgers must match the manifest exactly.
    history.chunk1.ledger_index = 101;
    assert!(fetch_bundle_in(&history, &hash, 1, false, Namespace::Test).await.unwrap_err().to_string().contains("incorrect ledger endpoints"));
    history.chunk1.ledger_index = 102;
    // Wrong network on the reader side.
    assert!(matches!(fetch_bundle_in(&history, &hash, 2, false, Namespace::Test).await, Err(Error::Network)));
    assert!(matches!(fetch_bundle_in(&history, &hash, 0, false, Namespace::Test).await, Err(Error::Network)));
    // Manifest not found by the RPC is missing history, not invalid data.
    assert!(matches!(fetch_bundle_in(&history, &[0; 32], 1, false, Namespace::Test).await, Err(Error::HistoryUnavailable(_))));
}

#[tokio::test]
async fn reconstruct_pages_and_reject_conflicts_gaps_and_untrusted_records() {
    let bundle = CircuitBundle { descriptor_json: br#"{"test":true}"#.to_vec(), vk_bytes: vec![1; 808], policy_json: b"{}".to_vec() };
    let bytes = bundle.encode();
    let id = ids::bundle_id(&bytes);
    let wallet = Wallet::from_entropy(&[8;16]);
    let first = ChunkMemo { bundle_id:id, index:0, total:2, payload:bytes[..800].to_vec() };
    let mut last = ChunkMemo { bundle_id:id, index:1, total:2, payload:bytes[800..].to_vec() };
    let mut manifest = CircuitManifest { network:1, bundle_id:id, bundle_len:bytes.len() as u32, chunk_total:2,
        first_chunk_ledger:100, last_chunk_ledger:102, circuit_id:ids::circuit_id(&bundle.descriptor_json).unwrap(),
        policy_id:ids::policy_id(&bundle.policy_json).unwrap(), vk_digest:ids::vk_digest(&bundle.vk_bytes) };
    let manifest_tx = record(&wallet, MemoKind::Circuit, &manifest.encode(), 103);
    let chunk1 = record(&wallet, MemoKind::CircuitChunk, &last.encode(), 102);
    let mut history = History { manifest:manifest_tx.clone(), chunk0:record(&wallet, MemoKind::CircuitChunk, &first.encode(), 100),
        chunk1:chunk1.clone(), extra:Some(chunk1.clone()), complete:"90-105" };
    async fn fetch(h: &History) -> Result<galaxy_xrpl::registry::FetchedBundle, Error> {
        fetch_bundle(h, &h.manifest.hash, 1, false).await
    }
    assert_eq!(fetch(&history).await.unwrap().bytes, bytes);
    history.complete = "90-100,102-105";
    assert!(matches!(fetch(&history).await, Err(Error::HistoryUnavailable(_))));
    history.complete = "90-105";
    last.payload[0] ^= 1;
    history.extra = Some(record(&wallet, MemoKind::CircuitChunk, &last.encode(), 102));
    assert!(fetch(&history).await.unwrap_err().to_string().contains("conflicting"));
    history.extra = None;
    history.chunk1 = record(&wallet, MemoKind::CircuitChunk, &last.encode(), 102);
    assert!(fetch(&history).await.unwrap_err().to_string().contains("digest mismatch"));
    last.payload[0] ^= 1;
    history.chunk1 = record(&Wallet::from_entropy(&[9;16]), MemoKind::CircuitChunk, &last.encode(), 102);
    assert!(fetch(&history).await.unwrap_err().to_string().contains("wrong type, account or ledger"));
    last.index = 2;
    history.chunk1 = record(&wallet, MemoKind::CircuitChunk, &last.encode(), 102);
    assert!(fetch(&history).await.is_err());
    last.index = 1;
    last.bundle_id = [0x33;32];
    history.chunk1 = record(&wallet, MemoKind::CircuitChunk, &last.encode(), 102);
    assert!(fetch(&history).await.unwrap_err().to_string().contains("missing chunks"));
    history.chunk1 = chunk1.clone();
    history.chunk1.validated = false;
    assert!(fetch(&history).await.is_err());
    history.chunk1.validated = true;
    history.chunk1.transaction_result = "tecFAILED_PROCESSING".into();
    assert!(fetch(&history).await.is_err());
    history.chunk1 = chunk1;
    manifest.bundle_len = 1024 * 1024 + 1;
    history.manifest = record(&wallet, MemoKind::Circuit, &manifest.encode(), 103);
    assert!(fetch(&history).await.is_err());
    history.manifest = manifest_tx;
    history.manifest.tx_blob[0] ^= 1;
    assert!(fetch(&history).await.is_err());
}
