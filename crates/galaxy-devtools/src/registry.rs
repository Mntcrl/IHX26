use std::{path::Path, time::Instant};

use ed25519_dalek::{Signer, SigningKey};
use galaxy_attestation::{verify_registration, AcceptedAttestation, VerifiedCredential};
use galaxy_protocol::{
    ids,
    wire::{AttestationEnv, CircuitBundle, DeviceBundle, DeviceKind},
    Hash32,
};
use galaxy_xrpl::{
    codec,
    ledger::LedgerTx,
    registry::{fetch_bundle, publish_bundle, BundleKind, Manifest, PublishedBundle},
    transaction::{decode_galaxy_memo, Memo, MAX_MEMOS_BYTES},
    LedgerSource,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{delay, error, ledger, output, storage, Result, Session, NETWORK};

#[derive(Serialize)]
pub(crate) struct TxObservation {
    pub tx: String,
    pub ledger: u32,
    pub transaction_index: u32,
    pub result: String,
    pub last_ledger_sequence: u32,
    pub memo_type: String,
    pub memo_data_bytes: usize,
    pub memos_bytes: usize,
    pub blob_bytes: usize,
    pub fee_drops: u64,
}

pub(crate) fn observe(tx: &LedgerTx, fee_cap: u64) -> Result<TxObservation> {
    if !tx.validated || tx.transaction_result != "tesSUCCESS" {
        return Err(error(&format!("transaction {} did not validate successfully ({})", hex::encode_upper(tx.hash), tx.transaction_result)));
    }
    let memo = decode_galaxy_memo(&tx.tx_blob)?;
    let mut without_memos = codec::decode(&tx.tx_blob)?;
    let map = without_memos.as_object_mut().ok_or_else(|| error("invalid serialized AccountSet"))?;
    let fee_drops = map.get("Fee").and_then(|v| v.as_str()).and_then(|s| s.parse::<u64>().ok())
        .filter(|fee| (1..=fee_cap).contains(fee)).ok_or_else(|| error("validated fee outside configured limits"))?;
    let last_ledger_sequence = map.get("LastLedgerSequence").and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok())
        .filter(|&end| end >= tx.ledger_index).ok_or_else(|| error("invalid validated transaction deadline"))?;
    map.remove("Memos").ok_or_else(|| error("validated transaction is missing its Memos field"))?;
    let memos_bytes = tx.tx_blob.len().checked_sub(codec::encode(&without_memos)?.len())
        .filter(|&size| size <= MAX_MEMOS_BYTES && size == Memo { kind: &memo.kind, data: &memo.data }.serialized_len())
        .ok_or_else(|| error("serialized Memos size does not match Galaxy envelope"))?;
    Ok(TxObservation {
        tx: hex::encode_upper(tx.hash), ledger: tx.ledger_index,
        transaction_index: tx.transaction_index, result: tx.transaction_result.clone(), last_ledger_sequence,
        memo_type: memo.kind, memo_data_bytes: memo.data.len(), memos_bytes,
        blob_bytes: tx.tx_blob.len(), fee_drops,
    })
}

async fn publish(session: &Session<'_>, kind: BundleKind, bytes: &[u8]) -> Result<(PublishedBundle, Vec<TxObservation>)> {
    let published = publish_bundle(&session.submitter, session.wallet, kind, bytes, delay).await?;
    let fetched = fetch_bundle(session.rpc, &published.manifest_tx, NETWORK, false).await?;
    if fetched.account != session.wallet.account_id() || fetched.bytes != bytes {
        return Err(error("historical registry retrieval did not recover the published bytes"));
    }
    let mut transactions = Vec::with_capacity(published.chunk_txs.len() + 1);
    for hash in published.chunk_txs.iter().chain(std::iter::once(&published.manifest_tx)) {
        let tx = session.rpc.tx(hash).await?.ok_or_else(|| error("published transaction unavailable by hash"))?;
        transactions.push(observe(&tx, session.config.max_fee_drops)?);
    }
    Ok((published, transactions))
}

pub(crate) async fn circuit(session: &Session<'_>, path: &Path) -> Result<()> {
    let bytes = storage::read(path, storage::BUNDLE_CAP)?;
    let circuit = CircuitBundle::decode(&bytes)?;
    let _ = galaxy_crypto::groth16::VerifyingKey::from_bytes(&circuit.vk_bytes)?;
    let start = Instant::now();
    let (published, transactions) = publish(session, BundleKind::Circuit, &bytes).await?;
    output(json!({
        "registry": "circuit",
        "circuit_id": hex::encode(ids::circuit_id(&circuit.descriptor_json)),
        "policy_id": hex::encode(ids::policy_id(&circuit.policy_json)),
        "vk_digest": hex::encode(ids::vk_digest(&circuit.vk_bytes)),
        "registration_tx": hex::encode_upper(published.manifest_tx),
        "transactions": transactions,
        "publish_and_retrieve_ms": start.elapsed().as_millis(),
        "note": "Publication does not confer trust. Explicitly review and allowlist this development circuit in the verification config."
    }));
    Ok(())
}

pub(crate) fn device_key(session: &Session<'_>, name: &str, create: bool) -> Result<SigningKey> {
    storage::check_name(name)?;
    let keys = storage::PrivateDir::open(&session.wallet_root.join("development-credentials"))?;
    let _lock = keys.lock()?;
    let filename = format!("{}-{name}.ed25519", session.wallet.classic_address());
    let mut seed = Zeroizing::new([0; 32]);
    match keys.read(&filename, 32)? {
        Some(bytes) => {
            let bytes = Zeroizing::new(bytes);
            if bytes.len() != seed.len() {
                return Err(error("invalid development credential key file"));
            }
            seed.copy_from_slice(&bytes);
        }
        None if create => {
            storage::random_into(&mut seed[..])?;
            keys.write(&filename, &seed[..])?;
        }
        None => return Err(error("register the selected development device first")),
    }
    Ok(SigningKey::from_bytes(&seed))
}

pub(crate) async fn device(session: &Session<'_>, name: &str) -> Result<()> {
    let key = device_key(session, name, true)?;
    let key_bytes = key.verifying_key().to_bytes();
    let key_id: Hash32 = Sha256::digest(key_bytes).into();
    let journal = session.journal.subdir("registrations")?;
    let file = format!("{name}.gxdb");
    let bytes = match journal.read(&file, storage::BUNDLE_CAP)? {
        Some(bytes) => bytes,
        None => {
            let head = session.submitter.validated_ledger().await?;
            let reg_ledger = ledger(session.rpc, head).await?;
            let bundle = DeviceBundle {
                kind: DeviceKind::DevUnattested,
                env: AttestationEnv::NotApplicable,
                app_id: "GALAXYDEV.org.xrpl-galaxy.development".into(),
                key_id,
                network: NETWORK,
                reg_ledger_index: reg_ledger.ledger_index,
                reg_ledger_hash: reg_ledger.hash,
                reg_nonce: storage::random()?,
                account_id: session.wallet.account_id(),
                attestation: key_bytes.to_vec(),
            };
            let bytes = bundle.encode();
            DeviceBundle::decode(&bytes)?;
            journal.write(&file, &bytes)?;
            bytes
        }
    };
    if DeviceBundle::decode(&bytes)?.key_id != key_id {
        return Err(error("saved registration was signed for a different development credential"));
    }
    let start = Instant::now();
    let (published, transactions) = publish(session, BundleKind::Device, &bytes).await?;
    credential(session, &published.manifest_tx, &key).await?;
    output(json!({
        "registry": "device",
        "credential": "development unattested credential",
        "key_id": hex::encode(key_id),
        "registration_tx": hex::encode_upper(published.manifest_tx),
        "transactions": transactions,
        "publish_and_retrieve_ms": start.elapsed().as_millis()
    }));
    Ok(())
}

pub(crate) async fn credential(session: &Session<'_>, tx: &Hash32, key: &SigningKey) -> Result<VerifiedCredential> {
    let fetched = fetch_bundle(session.rpc, tx, NETWORK, false).await?;
    let Manifest::Device(manifest) = fetched.manifest else {
        return Err(error("registration is not a DEVICE transaction"));
    };
    let bundle = DeviceBundle::decode(&fetched.bytes)?;
    if fetched.account != session.wallet.account_id() || bundle.kind != DeviceKind::DevUnattested
        || key.verifying_key().as_bytes().as_slice() != bundle.attestation
    {
        return Err(error("DEVICE belongs to a different wallet or development credential"));
    }
    let registered = session.rpc.tx(tx).await?.ok_or_else(|| error("DEVICE transaction unavailable"))?;
    let registered_at = ledger(session.rpc, registered.ledger_index).await?;
    let anchored = ledger(session.rpc, manifest.reg_ledger_index).await?;
    if anchored.hash != manifest.reg_ledger_hash || registered_at.ledger_index <= anchored.ledger_index {
        return Err(error("DEVICE registration ledger challenge does not match XRPL"));
    }
    Ok(verify_registration(&bundle, &manifest, registered_at.close_time, &AcceptedAttestation::default())?)
}

pub(crate) fn assertion(core: &[u8], key: &SigningKey) -> Vec<u8> {
    let mut message = b"GALAXY/DEV-ASSERT/v1\0".to_vec();
    message.extend_from_slice(core);
    let mut result = Vec::with_capacity(65);
    result.push(0xf0);
    result.extend_from_slice(&key.sign(&message).to_bytes());
    result
}
