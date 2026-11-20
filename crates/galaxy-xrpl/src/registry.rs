//! On-ledger device and circuit registry. This reconstructs the committed
//! bytes; it does not establish trust in the device or the verifying key.

use crate::keys::{classic_address, tx_hash, Wallet};
use crate::ledger::{LedgerTx, RpcTransport};
use crate::submission::{Submitter, TxState, TxStore};
use crate::transaction::{decode_galaxy_object, Memo};
use crate::{codec, AccountId, Error, Hash32, LedgerSource};
use galaxy_protocol::{ids, wire::{ChunkMemo, CircuitBundle, CircuitManifest, DeviceBundle, DeviceManifest, MemoKind, WireError}};
use serde_json::Value;
use std::{collections::HashSet, future::Future};

pub const MAX_BUNDLE_BYTES: usize = 1024 * 1024;
pub const CHUNK_BYTES: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind { Device, Circuit }

impl BundleKind {
    fn manifest(self) -> MemoKind {
        match self { Self::Device => MemoKind::Device, Self::Circuit => MemoKind::Circuit }
    }
    fn chunk(self) -> MemoKind {
        match self { Self::Device => MemoKind::DeviceChunk, Self::Circuit => MemoKind::CircuitChunk }
    }
}

#[derive(Debug)]
pub enum Manifest {
    Device(DeviceManifest),
    Circuit(CircuitManifest),
}

#[derive(Debug)]
pub struct PublishedBundle {
    pub manifest_tx: Hash32,
    pub chunk_txs: Vec<Hash32>,
}

#[derive(Debug)]
pub struct FetchedBundle {
    pub manifest: Manifest,
    pub manifest_ledger: u32,
    pub manifest_tx_index: u32,
    pub account: AccountId,
    pub bytes: Vec<u8>,
}

/// Both the journal purpose and the signed payload are stable across crashes.
/// An expired unvalidated chunk/manifest may be re-signed only after its entire
/// old ledger window was searched. Transaction execution is strictly serial.
pub async fn publish_bundle<R, S, D, F>(
    submitter: &Submitter<'_, R, S>, wallet: &Wallet, kind: BundleKind, bytes: &[u8], delay: D,
) -> Result<PublishedBundle, Error>
where R: RpcTransport, S: TxStore, D: Fn() -> F, F: Future<Output = ()>,
{
    check_bundle_length(bytes.len())?;
    let account = wallet.account_id();
    let network = submitter.network_id();
    let (device, circuit, deadline) = match kind {
        BundleKind::Device => {
            let b = DeviceBundle::decode(bytes)?;
            if b.account_id != account || b.network != network {
                return Err(Error::Invalid("device bundle is not bound to wallet/network"));
            }
            let deadline = b.reg_ledger_index.checked_add(100).ok_or(Error::Invalid("registration deadline overflow"))?;
            (Some(b), None, Some(deadline))
        }
        BundleKind::Circuit => (None, Some(CircuitBundle::decode(bytes)?), None),
    };
    let bundle_id = ids::bundle_id(bytes);
    let chunk_total = u16::try_from(bytes.len().div_ceil(CHUNK_BYTES)).unwrap();
    let mut chunk_txs = Vec::with_capacity(usize::from(chunk_total));
    let scope = format!("registry/{network}/{}/{}/{}", classic_address(&account), kind.manifest().as_str(), hex::encode(bundle_id));
    let mut first_chunk_ledger = u32::MAX;
    let mut last_chunk_ledger = 0;
    for (index, payload) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let wire = ChunkMemo { bundle_id, index: index as u16, total: chunk_total, payload: payload.to_vec() }.encode();
        let tx = publish_one(submitter, wallet, &format!("{scope}/chunk/{index}"),
            Memo { kind: kind.chunk().as_str(), data: &wire }, deadline, &delay).await?;
        first_chunk_ledger = first_chunk_ledger.min(tx.ledger_index);
        last_chunk_ledger = last_chunk_ledger.max(tx.ledger_index);
        chunk_txs.push(tx.hash);
    }
    let bundle_len = bytes.len() as u32;
    let wire = match (device, circuit) {
        (Some(b), None) => DeviceManifest {
            network, bundle_id, bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger,
            kind: b.kind, env: b.env, key_id: b.key_id, account_id: account,
            reg_ledger_index: b.reg_ledger_index, reg_ledger_hash: b.reg_ledger_hash,
        }.encode(),
        (None, Some(b)) => {
            let (circuit_id, policy_id, vk_digest) = circuit_ids(&b)?;
            CircuitManifest {
                network, bundle_id, bundle_len, chunk_total, first_chunk_ledger, last_chunk_ledger,
                circuit_id, policy_id, vk_digest,
            }.encode()
        }
        _ => unreachable!(),
    };
    let manifest_tx = publish_one(submitter, wallet, &format!("{scope}/manifest"),
        Memo { kind: kind.manifest().as_str(), data: &wire }, deadline, &delay).await?.hash;
    Ok(PublishedBundle { manifest_tx, chunk_txs })
}

/// Identifiers committed by a circuit manifest; the bundle JSON must be canonical.
fn circuit_ids(b: &CircuitBundle) -> Result<(Hash32, Hash32, Hash32), Error> {
    let circuit_id = ids::circuit_id(&b.descriptor_json).map_err(WireError::from)?;
    let policy_id = ids::policy_id(&b.policy_json).map_err(WireError::from)?;
    Ok((circuit_id, policy_id, ids::vk_digest(&b.vk_bytes)))
}

async fn publish_one<R, S, D, F>(
    submitter: &Submitter<'_, R, S>, wallet: &Wallet, purpose: &str, memo: Memo<'_>, deadline: Option<u32>, delay: &D,
) -> Result<LedgerTx, Error>
where R: RpcTransport, S: TxStore, D: Fn() -> F, F: Future<Output = ()>,
{
    let pending = submitter.prepare(purpose, wallet, memo, deadline).await?;
    let mut state = submitter.run(&pending, delay).await?;
    if matches!(state, TxState::Expired) {
        let replacement = submitter.replace_expired(purpose, wallet, memo, deadline).await?;
        state = submitter.run(&replacement, delay).await?;
    }
    match state {
        TxState::Validated(tx) if tx.transaction_result == "tesSUCCESS" => Ok(tx),
        TxState::Validated(tx) => Err(Error::Rpc(format!("registry transaction {} validated with {}", hex::encode_upper(tx.hash), tx.transaction_result))),
        _ => Err(Error::Invalid("registry transaction not validated before deadline")),
    }
}

fn check_bundle_length(len: usize) -> Result<(), Error> {
    if !(1..=MAX_BUNDLE_BYTES).contains(&len) { return Err(Error::Invalid("registry bundle size out of range")); }
    Ok(())
}

fn validate_record(tx: &LedgerTx) -> Result<(), Error> {
    if !tx.validated || tx.transaction_result != "tesSUCCESS" || tx_hash(&tx.tx_blob) != tx.hash {
        return Err(Error::Invalid("registry record is failed, unvalidated or hash-mismatched"));
    }
    Ok(())
}

pub async fn fetch_bundle<L: LedgerSource>(source: &L, hash: &Hash32, network_id: u32, allow_mainnet: bool) -> Result<FetchedBundle, Error> {
    if network_id == 0 && !allow_mainnet { return Err(Error::Network); }
    let tx = source.tx(hash).await?.ok_or(Error::HistoryUnavailable("registry manifest not found"))?;
    let info = source.server_info().await?;
    if info.network_id != network_id { return Err(Error::Network); }
    if &tx.hash != hash || tx.ledger_index > info.validated_ledger { return Err(Error::Invalid("registry manifest identity mismatch")); }
    validate_record(&tx)?;
    let object = codec::decode(&tx.tx_blob)?;
    let memo = decode_galaxy_object(&object)?;
    check_network(memo.network_id, network_id)?;
    let account = memo.account;
    let (kind, manifest, id, len, total, first, last) = match memo.kind.as_str() {
        kind if kind == MemoKind::Device.as_str() => {
            let b = DeviceManifest::decode(&memo.data)?;
            if b.account_id != account || b.network != network_id
                || !(b.reg_ledger_index..=b.reg_ledger_index.saturating_add(100)).contains(&tx.ledger_index)
                || b.first_chunk_ledger < b.reg_ledger_index
            {
                return Err(Error::Invalid("device manifest account, network or registration ledger differs"));
            }
            let fields = (b.bundle_id, b.bundle_len, b.chunk_total, b.first_chunk_ledger, b.last_chunk_ledger);
            (BundleKind::Device, Manifest::Device(b), fields.0, fields.1, fields.2, fields.3, fields.4)
        }
        kind if kind == MemoKind::Circuit.as_str() => {
            let b = CircuitManifest::decode(&memo.data)?;
            if b.network != network_id { return Err(Error::Network); }
            let fields = (b.bundle_id, b.bundle_len, b.chunk_total, b.first_chunk_ledger, b.last_chunk_ledger);
            (BundleKind::Circuit, Manifest::Circuit(b), fields.0, fields.1, fields.2, fields.3, fields.4)
        }
        _ => return Err(Error::Invalid("unexpected registry manifest memo type")),
    };
    let len = len as usize;
    check_bundle_length(len)?;
    if total == 0 || usize::from(total) > len || len > usize::from(total) * CHUNK_BYTES
        || first == 0 || first > last || last > tx.ledger_index
    {
        return Err(Error::Invalid("invalid manifest chunk bounds"));
    }
    if !covers(&info.complete_ledgers, first, last) {
        return Err(Error::HistoryUnavailable("required registry chunk ledgers not available"));
    }
    let mut chunks = vec![None; usize::from(total)];
    let mut observed = (u32::MAX, 0);
    let mut stored = 0usize;
    let mut marker = None;
    let mut markers = HashSet::new();
    let memo_type = hex::encode_upper(kind.chunk().as_str());
    loop {
        let mut page = source.account_tx(&account, first, last, marker, 200).await?;
        for entry in page.transactions.drain(..) {
            let object = codec::decode(&entry.tx_blob)?;
            // A different bundle can legitimately have chunks on this account in
            // the same ledger range. Do not try to reconstruct it. A malformed
            // envelope explicitly naming this bundle is not silently ignored.
            if !names_bundle(&object, &memo_type, &id) { continue; }
            validate_record(&entry)?;
            let decoded = decode_galaxy_object(&object)?;
            check_network(decoded.network_id, network_id)?;
            if decoded.kind != kind.chunk().as_str() || decoded.account != account
                || !(first..=last).contains(&entry.ledger_index)
                || (entry.ledger_index, entry.transaction_index) >= (tx.ledger_index, tx.transaction_index)
            {
                return Err(Error::Invalid("registry chunk has wrong type, account or ledger order"));
            }
            let chunk = ChunkMemo::decode(&decoded.data)?;
            if chunk.bundle_id != id || chunk.total != total || chunk.index >= total {
                return Err(Error::Invalid("registry chunk identity/count mismatch"));
            }
            let slot = &mut chunks[usize::from(chunk.index)];
            match slot {
                Some(old) if old != &chunk.payload => return Err(Error::Invalid("conflicting registry chunk duplicate")),
                Some(_) => {}
                None => {
                    stored += chunk.payload.len();
                    if stored > len { return Err(Error::Invalid("registry chunks exceed manifest length")); }
                    *slot = Some(chunk.payload);
                }
            }
            observed.0 = observed.0.min(entry.ledger_index);
            observed.1 = observed.1.max(entry.ledger_index);
        }
        marker = page.marker;
        match &marker {
            None => break,
            Some(value) if !markers.insert(serde_json::to_string(value)?) || markers.len() > 4096 => {
                return Err(Error::Invalid("registry account_tx pagination did not terminate"));
            }
            _ => {}
        }
    }
    if stored != len || observed != (first, last) { return Err(Error::Invalid("missing chunks or incorrect ledger endpoints")); }
    let mut bytes = Vec::with_capacity(len);
    for chunk in chunks { bytes.extend(chunk.ok_or(Error::Invalid("gap in registry chunks"))?); }
    if ids::bundle_id(&bytes) != id { return Err(Error::Invalid("registry bundle digest mismatch")); }
    match &manifest {
        Manifest::Device(manifest) => {
            let b = DeviceBundle::decode(&bytes)?;
            if b.kind != manifest.kind || b.env != manifest.env || b.key_id != manifest.key_id
                || b.network != network_id || b.account_id != account
                || b.reg_ledger_hash != manifest.reg_ledger_hash || b.reg_ledger_index != manifest.reg_ledger_index
            { return Err(Error::Invalid("device manifest differs from bundle")); }
        }
        Manifest::Circuit(manifest) => {
            let b = CircuitBundle::decode(&bytes)?;
            if circuit_ids(&b)? != (manifest.circuit_id, manifest.policy_id, manifest.vk_digest) {
                return Err(Error::Invalid("circuit manifest differs from bundle"));
            }
        }
    }
    Ok(FetchedBundle { manifest, manifest_ledger: tx.ledger_index, manifest_tx_index: tx.transaction_index, account, bytes })
}

fn check_network(field: Option<u32>, network: u32) -> Result<(), Error> {
    if field != (network > 1024).then_some(network) { return Err(Error::Network); }
    Ok(())
}

fn names_bundle(object: &Value, memo_type: &str, id: &Hash32) -> bool {
    object["Memos"].as_array().is_some_and(|memos| memos.iter().any(|entry| {
        let memo = &entry["Memo"];
        if memo["MemoType"] != memo_type { return false; }
        let mut given = [0; 32];
        memo["MemoData"].as_str().and_then(|s| s.get(2..66)).is_some_and(|s| {
            hex::decode_to_slice(s, &mut given).is_ok() && &given == id
        })
    }))
}

/// Whether a well-formed rippled `complete_ledgers` range list covers every
/// ledger between `min` and `max`, inclusive.
pub fn covers(ranges: &str, min: u32, max: u32) -> bool {
    if min == 0 || min > max { return false; }
    let mut next = u64::from(min);
    let mut previous_end = 0;
    for range in ranges.split(',') {
        let (a, b) = range.split_once('-').unwrap_or((range, range));
        let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) else { return false; };
        if a > b || a <= previous_end { return false; }
        previous_end = b;
        if u64::from(a) <= next && u64::from(b) >= next { next = u64::from(b) + 1; }
    }
    next > u64::from(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coverage_never_skips_a_gap() {
        assert!(covers("2-3,8-10,11-19", 8, 16));
        assert!(!covers("2-3,8-10,12-19", 8, 16));
        assert!(covers("7", 7, 7));
        assert!(!covers("7,broken", 7, 7));
        assert!(!covers("2-3,3-7", 7, 7));
        assert!(!covers("empty", 1, 20));
    }
}
