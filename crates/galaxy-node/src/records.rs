use ed25519_dalek::{Signature, VerifyingKey};
use galaxy_protocol::{ids, wire::{CaptureMemo, ChunkMemo, CircuitManifest, DeviceManifest, NodeAnnouncement, ProofMemo, RevokeMemo}, AccountId, Hash32};
use galaxy_xrpl::{codec, keys, ledger::LedgerTx, transaction};

use crate::Result;

const PREFIX_HEX: &str = "67616c6178792f76312f"; // galaxy/v1/
pub(crate) const CAPTURE: &str = "galaxy/v1/capture";
pub(crate) const PROOF: &str = "galaxy/v1/proof";
pub(crate) const DEVICE: &str = "galaxy/v1/device";
pub(crate) const CIRCUIT: &str = "galaxy/v1/circuit";
pub(crate) const REVOKE: &str = "galaxy/v1/revoke";
pub(crate) const NODE: &str = "galaxy/v1/node";

#[derive(Default)]
struct Links {
    id: Option<Hash32>,
    reference: Option<Hash32>,
    registration: Option<Hash32>,
    bundle: Option<Hash32>,
    counter: Option<u32>,
    announcement: Option<(String, u64, Hash32)>,
}

pub(crate) struct Record {
    pub hash: String,
    pub ledger: u32,
    pub index: u32,
    pub account: Option<String>,
    pub kind: String,
    pub rejected: Option<&'static str>,
    pub id: Option<String>,
    pub reference: Option<String>,
    pub registration: Option<String>,
    pub bundle: Option<String>,
    pub counter: Option<u32>,
    pub announcement: Option<(String, u64, Hash32)>,
}

/// `LedgerTx` is obtained from XRPL's typed binary reads, which recompute the
/// hash and validate metadata. Never use a WS notice as a transaction record.
pub(crate) fn parse(tx: &LedgerTx, network: u32, scope: Option<&AccountId>) -> Result<Option<Record>> {
    if !tx.validated || tx.transaction_result != "tesSUCCESS" {
        return Ok(None);
    }
    let object = codec::decode(&tx.tx_blob)?;
    let Some(candidate) = object["Memos"].as_array().into_iter().flatten()
        .filter_map(|entry| entry["Memo"]["MemoType"].as_str())
        .find(|raw| raw.get(..PREFIX_HEX.len()).is_some_and(|s| s.eq_ignore_ascii_case(PREFIX_HEX))) else {
        return Ok(None);
    };
    let account = object["Account"].as_str();
    if let Some(scope) = scope {
        // account_tx includes transactions incoming to this account. Do not
        // mistake them for records published by the scoped account.
        if account.map(keys::decode_address).transpose()?.as_ref() != Some(scope) {
            return Ok(None);
        }
    }
    let mut record = Record {
        hash: hex::encode_upper(tx.hash), ledger: tx.ledger_index, index: tx.transaction_index,
        account: account.map(str::to_owned), kind: display_kind(candidate), rejected: None,
        id: None, reference: None, registration: None, bundle: None, counter: None, announcement: None,
    };
    let memo = match transaction::decode_galaxy_object(&object) {
        Ok(memo) => memo,
        Err(_) => {
            record.rejected = Some("invalid_memo_envelope");
            return Ok(Some(record));
        }
    };
    match links(&memo, network) {
        Ok(links) => {
            record.id = links.id.map(hex::encode);
            record.reference = links.reference.map(|value| if record.kind == CAPTURE { hex::encode(value) } else { hex::encode_upper(value) });
            record.registration = links.registration.map(hex::encode_upper);
            record.bundle = links.bundle.map(hex::encode);
            record.counter = links.counter;
            record.announcement = links.announcement;
        }
        Err(reason) => record.rejected = Some(reason),
    }
    Ok(Some(record))
}

fn display_kind(raw: &str) -> String {
    if raw.len() <= 96 {
        if let Ok(text) = hex::decode(raw).and_then(|bytes| String::from_utf8(bytes).map_err(|_| hex::FromHexError::InvalidStringLength)) {
            if text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-')) {
                return text;
            }
        }
    }
    "galaxy/v1/unknown".into()
}

fn links(memo: &transaction::GalaxyMemo, network: u32) -> std::result::Result<Links, &'static str> {
    let mut links = Links::default();
    let wire = |_| "invalid_payload";
    let net = |actual| (actual == network).then_some(()).ok_or("wrong_network");
    let account = |actual| (actual == memo.account).then_some(()).ok_or("wrong_account");
    match memo.kind.as_str() {
        CAPTURE => {
            let capture = CaptureMemo::decode(&memo.data).map_err(wire)?;
            net(capture.core.network)?;
            account(capture.core.account_id)?;
            // The validated decoder binds the first 299 bytes to capture.core.
            links.id = Some(ids::capture_id(&memo.data[..299]));
            links.reference = Some(capture.core.photo_id);
            links.registration = Some(capture.core.registration_tx);
            links.counter = galaxy_attestation::assertion_counter_hint(&capture.assertion).ok();
        }
        PROOF => {
            let proof = ProofMemo::decode(&memo.data).map_err(wire)?;
            net(proof.network)?;
            links.reference = Some(proof.manifest_tx);
        }
        DEVICE => {
            let device = DeviceManifest::decode(&memo.data).map_err(wire)?;
            net(device.network)?;
            account(device.account_id)?;
            links.bundle = Some(device.bundle_id);
        }
        CIRCUIT => {
            let circuit = CircuitManifest::decode(&memo.data).map_err(wire)?;
            net(circuit.network)?;
            links.id = Some(circuit.circuit_id);
            links.bundle = Some(circuit.bundle_id);
        }
        "galaxy/v1/device-chunk" | "galaxy/v1/circuit-chunk" => {
            let chunk = ChunkMemo::decode(&memo.data).map_err(wire)?;
            links.bundle = Some(chunk.bundle_id);
        }
        REVOKE => {
            let revoke = RevokeMemo::decode(&memo.data).map_err(wire)?;
            net(revoke.network)?;
            links.registration = Some(revoke.registration_tx);
        }
        NODE => {
            let announcement = NodeAnnouncement::decode(&memo.data).map_err(wire)?;
            net(announcement.network)?;
            let key = VerifyingKey::from_bytes(&announcement.node_pubkey).map_err(|_| "invalid_node_signature")?;
            key.verify_strict(&announcement.signing_message(), &Signature::from_bytes(&announcement.sig))
                .map_err(|_| "invalid_node_signature")?;
            links.announcement = Some((announcement.url, announcement.expires_at, announcement.node_pubkey));
        }
        _ => return Err("unsupported_memo_type"),
    }
    Ok(links)
}
