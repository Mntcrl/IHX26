use std::{
    future::{ready, Future},
    pin::pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

use galaxy_protocol::{
    network::{NetworkId, MAX_FEE_DROPS_LIMIT},
    wire::{CaptureMemo, ChunkMemo, DeviceManifest, MemoKind, ProofMemo},
    AccountId, Hash32,
};
use galaxy_xrpl::{
    keys::encode_family_seed,
    ledger::{self, LedgerSource, RpcLedger, RpcTransport},
    mobile,
    submission::ResultClass,
    transaction::decode_galaxy_memo,
};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{exact, guarded, network_id, GalaxyError, Memo, Result};

const MAX_RPC_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, uniffi::Record)]
pub struct XrplNetwork {
    pub network_id: u32,
    pub allow_mainnet: bool,
    pub max_fee_drops: u64,
}

impl TryFrom<XrplNetwork> for mobile::NetworkParams {
    type Error = GalaxyError;

    fn try_from(value: XrplNetwork) -> Result<Self> {
        if network_id(value.network_id)? == NetworkId::Mainnet && !value.allow_mainnet {
            return Err(GalaxyError::Network);
        }
        if !(1..=MAX_FEE_DROPS_LIMIT).contains(&value.max_fee_drops) {
            return Err(GalaxyError::InvalidInput);
        }
        Ok(Self {
            network_id: value.network_id,
            allow_mainnet: value.allow_mainnet,
            max_fee_drops: value.max_fee_drops,
        })
    }
}

#[derive(uniffi::Record)]
pub struct WalletIdentity {
    pub classic_address: String,
    pub account_id: Vec<u8>,
    pub public_key: Vec<u8>,
}

#[derive(Clone, uniffi::Record)]
pub struct ValidatedLedger {
    pub index: u32,
    pub hash: Vec<u8>,
    pub close_time: u64,
}

impl ValidatedLedger {
    pub(crate) fn checked_hash(&self) -> Result<Hash32> {
        if self.index == 0 {
            return Err(GalaxyError::LedgerWindow);
        }
        exact(&self.hash)
    }
}

#[derive(Clone, uniffi::Record)]
pub struct XrplAutofill {
    pub sequence: u32,
    pub fee_drops: u64,
    pub last_ledger_sequence: u32,
    pub first_possible_ledger: u32,
    /// A 32-byte predecessor (including the initial zero sentinel); absent before setup.
    pub account_txn_id: Option<Vec<u8>>,
}

#[derive(Clone, uniffi::Record)]
pub struct SignedTransaction {
    pub purpose: String,
    pub hash_hex: String,
    pub blob: Vec<u8>,
    pub network_id: u32,
    pub first_possible_ledger: u32,
    pub last_ledger: u32,
}

impl From<mobile::SignedRecord> for SignedTransaction {
    fn from(r: mobile::SignedRecord) -> Self {
        Self {
            purpose: r.purpose,
            hash_hex: r.hash_hex,
            blob: r.blob,
            network_id: r.network_id,
            first_possible_ledger: r.first_possible_ledger,
            last_ledger: r.last_ledger,
        }
    }
}

impl From<SignedTransaction> for mobile::SignedRecord {
    fn from(r: SignedTransaction) -> Self {
        Self {
            purpose: r.purpose,
            hash_hex: r.hash_hex,
            blob: r.blob,
            network_id: r.network_id,
            first_possible_ledger: r.first_possible_ledger,
            last_ledger: r.last_ledger,
        }
    }
}

#[derive(uniffi::Enum)]
pub enum XrplSubmitClass {
    Success,
    ClaimedFee,
    Failure,
    Malformed,
    Local,
    Retry,
    Unknown,
}

#[derive(uniffi::Record)]
pub struct XrplSubmitOutcome {
    pub engine_result: String,
    pub result_class: XrplSubmitClass,
}

#[derive(uniffi::Enum)]
pub enum XrplTxStatus {
    Pending,
    Expired,
    Validated {
        ledger_index: u32,
        transaction_index: u32,
        transaction_result: String,
        success: bool,
    },
}

fn rpc_error(err: mobile::MobileError) -> GalaxyError {
    match err {
        mobile::MobileError::Network => GalaxyError::Network,
        _ => GalaxyError::Rpc,
    }
}

fn check_rpc_body(body: &str) -> Result<()> {
    if body.len() > MAX_RPC_BYTES {
        return Err(GalaxyError::Rpc);
    }
    Ok(())
}

// WalletSeedStore holds precisely these 16 CSPRNG bytes. The textual sEd seed
// is only needed transiently by the shared mobile API and never leaves Rust.
fn family_seed(entropy: &[u8]) -> Result<String> {
    let raw: &[u8; 16] = entropy.try_into().map_err(|_| GalaxyError::InvalidInput)?;
    Ok(std::mem::take(&mut *encode_family_seed(raw)))
}

#[uniffi::export]
pub fn wallet_identity(entropy: Vec<u8>) -> Result<WalletIdentity> {
    let entropy = Zeroizing::new(entropy);
    guarded(|| {
        let wallet =
            mobile::wallet_info(family_seed(&entropy)?).map_err(|_| GalaxyError::Transaction)?;
        Ok(WalletIdentity {
            classic_address: wallet.classic_address,
            account_id: wallet.account_id,
            public_key: wallet.public_key,
        })
    })
}

#[uniffi::export]
pub fn xrpl_server_info_request() -> String {
    mobile::server_info_request()
}

#[uniffi::export]
pub fn xrpl_fee_request() -> String {
    mobile::fee_request()
}

#[uniffi::export]
pub fn xrpl_account_info_request(classic_address: String, validated_ledger: u32) -> Result<String> {
    guarded(|| {
        galaxy_xrpl::keys::decode_address(&classic_address)
            .map_err(|_| GalaxyError::InvalidInput)?;
        if validated_ledger == 0 {
            return Err(GalaxyError::LedgerWindow);
        }
        Ok(mobile::account_info_request(
            classic_address,
            validated_ledger,
        ))
    })
}

#[uniffi::export]
pub fn xrpl_latest_validated(network: XrplNetwork, body: String) -> Result<u32> {
    guarded(|| {
        check_rpc_body(&body)?;
        mobile::validated_ledger_from_server_info(network.try_into()?, body).map_err(rpc_error)
    })
}

/// Capture, registration and challenge ledger headers must be read by exact index.
#[uniffi::export]
pub fn xrpl_ledger_request(index: u32) -> Result<String> {
    guarded(|| {
        if index == 0 {
            return Err(GalaxyError::LedgerWindow);
        }
        Ok(serde_json::json!({"method": "ledger", "params": [{"api_version": 2, "ledger_index": index, "transactions": false}]}).to_string())
    })
}

// Reuse the ledger decoder with the supplied result, not a networking client.
// This transport's one request is immediately ready; nothing needs an executor.
struct LedgerReply(Mutex<Option<Value>>);
impl RpcTransport for LedgerReply {
    fn request(
        &self,
        method: &str,
        _: Value,
    ) -> impl Future<Output = Result<Value, galaxy_xrpl::Error>> + Send {
        let res = if method == "ledger" {
            self.0.lock().ok().and_then(|mut reply| reply.take())
        } else {
            None
        };
        ready(res.ok_or(galaxy_xrpl::Error::Invalid("expected one ledger request")))
    }
}

#[uniffi::export]
pub fn xrpl_parse_validated_ledger(index: u32, body: String) -> Result<ValidatedLedger> {
    guarded(|| {
        if index == 0 {
            return Err(GalaxyError::LedgerWindow);
        }
        check_rpc_body(&body)?;
        let envelope = serde_json::from_str(&body).map_err(|_| GalaxyError::Rpc)?;
        let result = ledger::take_http_result(envelope).map_err(|_| GalaxyError::Rpc)?;
        let ledger = RpcLedger::new(LedgerReply(Mutex::new(Some(result))));
        let mut call = pin!(ledger.ledger_header(index));
        let Poll::Ready(result) = call.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            return Err(GalaxyError::Internal);
        };
        let header = result.map_err(|_| GalaxyError::Rpc)?;
        if !header.validated {
            return Err(GalaxyError::LedgerWindow);
        }
        Ok(ValidatedLedger {
            index: header.ledger_index,
            hash: header.hash.to_vec(),
            close_time: header.close_time,
        })
    })
}

#[uniffi::export]
pub fn xrpl_autofill(
    network: XrplNetwork,
    validated_ledger: u32,
    deadline: Option<u32>,
    classic_address: String,
    fee_body: String,
    account_info_body: String,
) -> Result<XrplAutofill> {
    guarded(|| {
        check_rpc_body(&fee_body)?;
        check_rpc_body(&account_info_body)?;
        if validated_ledger == 0 || deadline.is_some_and(|d| d <= validated_ledger) {
            return Err(GalaxyError::LedgerWindow);
        }
        let a = mobile::autofill(
            network.try_into()?,
            validated_ledger,
            deadline,
            classic_address,
            fee_body,
            account_info_body,
        )
        .map_err(rpc_error)?;
        Ok(XrplAutofill {
            sequence: a.sequence,
            fee_drops: a.fee_drops,
            last_ledger_sequence: a.last_ledger_sequence,
            first_possible_ledger: a.first_possible_ledger,
            account_txn_id: a.account_txn_id.map(|id| id.to_vec()),
        })
    })
}

fn memo_binding(memo: &Memo) -> Result<(Option<u32>, Option<AccountId>)> {
    let invalid = |_| GalaxyError::Transaction;
    match MemoKind::from_memo_type(&memo.memo_type) {
        Some(MemoKind::Capture) => {
            let capture = CaptureMemo::decode(&memo.data).map_err(invalid)?;
            galaxy_attestation::assertion_counter_hint(&capture.assertion)
                .map_err(|_| GalaxyError::Attestation)?;
            Ok((Some(capture.core.network), Some(capture.core.account_id)))
        }
        Some(MemoKind::Device) => {
            let device = DeviceManifest::decode(&memo.data).map_err(invalid)?;
            if device.kind != galaxy_protocol::wire::DeviceKind::AppleAppAttest {
                return Err(GalaxyError::Attestation);
            }
            Ok((Some(device.network), Some(device.account_id)))
        }
        Some(MemoKind::Proof) => Ok((
            Some(ProofMemo::decode(&memo.data).map_err(invalid)?.network),
            None,
        )),
        Some(MemoKind::DeviceChunk) => {
            ChunkMemo::decode(&memo.data).map_err(invalid)?;
            Ok((None, None))
        }
        _ => Err(GalaxyError::Transaction),
    }
}

/// Persist the returned record durably before the first submit.
#[uniffi::export]
pub fn xrpl_sign_account_set(
    entropy: Vec<u8>,
    network: XrplNetwork,
    purpose: String,
    fields: XrplAutofill,
    memo: Memo,
) -> Result<SignedTransaction> {
    let entropy = Zeroizing::new(entropy);
    guarded(|| {
        let params = network.try_into()?;
        let (bound_network, bound_account) = memo_binding(&memo)?;
        if bound_network.is_some_and(|id| id != network.network_id) {
            return Err(GalaxyError::Network);
        }
        let fields = mobile_autofill(fields)?;
        let signed = mobile::sign_galaxy_account_set(
            family_seed(&entropy)?,
            params,
            purpose,
            fields,
            memo.memo_type,
            memo.data,
        )
        .map_err(|_| GalaxyError::Transaction)?;
        if let Some(account) = bound_account {
            let envelope =
                decode_galaxy_memo(&signed.blob).map_err(|_| GalaxyError::Transaction)?;
            if envelope.account != account {
                return Err(GalaxyError::CaptureBinding);
            }
        }
        Ok(signed.into())
    })
}

fn mobile_autofill(fields: XrplAutofill) -> Result<mobile::AutofillFields> {
    Ok(mobile::AutofillFields {
        sequence: fields.sequence,
        fee_drops: fields.fee_drops,
        last_ledger_sequence: fields.last_ledger_sequence,
        first_possible_ledger: fields.first_possible_ledger,
        account_txn_id: fields.account_txn_id.as_deref().map(exact).transpose()?,
    })
}

/// Persist the one-time memo-free account preparation before submission.
#[uniffi::export]
pub fn xrpl_sign_account_preparation(
    entropy: Vec<u8>,
    network: XrplNetwork,
    fields: XrplAutofill,
) -> Result<SignedTransaction> {
    let entropy = Zeroizing::new(entropy);
    guarded(|| {
        mobile::sign_account_tracking_preparation(
            family_seed(&entropy)?, network.try_into()?, mobile_autofill(fields)?,
        )
        .map(Into::into)
        .map_err(|_| GalaxyError::Transaction)
    })
}

#[uniffi::export]
pub fn xrpl_submit_request(
    network: XrplNetwork,
    record: SignedTransaction,
    validated_ledger: u32,
) -> Result<String> {
    guarded(|| {
        mobile::submit_request(network.try_into()?, record.into(), validated_ledger)
            .map_err(|_| GalaxyError::Transaction)
    })
}

#[uniffi::export]
pub fn xrpl_parse_submit(
    network: XrplNetwork,
    record: SignedTransaction,
    body: String,
) -> Result<XrplSubmitOutcome> {
    guarded(|| {
        check_rpc_body(&body)?;
        let outcome = mobile::evaluate_submit_response(network.try_into()?, record.into(), body)
            .map_err(rpc_error)?;
        let result_class = match outcome.class {
            ResultClass::Success => XrplSubmitClass::Success,
            ResultClass::ClaimedFee => XrplSubmitClass::ClaimedFee,
            ResultClass::Failure => XrplSubmitClass::Failure,
            ResultClass::Malformed => XrplSubmitClass::Malformed,
            ResultClass::Local => XrplSubmitClass::Local,
            ResultClass::Retry => XrplSubmitClass::Retry,
            ResultClass::Unknown => XrplSubmitClass::Unknown,
        };
        Ok(XrplSubmitOutcome {
            engine_result: outcome.engine_result,
            result_class,
        })
    })
}

#[uniffi::export]
pub fn xrpl_tx_request(
    network: XrplNetwork,
    record: SignedTransaction,
    validated_ledger: u32,
) -> Result<String> {
    guarded(|| {
        mobile::tx_request(network.try_into()?, record.into(), validated_ledger)
            .map_err(|_| GalaxyError::Transaction)
    })
}

#[uniffi::export]
pub fn xrpl_parse_tx(
    network: XrplNetwork,
    record: SignedTransaction,
    validated_ledger: u32,
    body: String,
) -> Result<XrplTxStatus> {
    guarded(|| {
        check_rpc_body(&body)?;
        Ok(
            match mobile::evaluate_tx_response(
                network.try_into()?,
                record.into(),
                validated_ledger,
                body,
            )
            .map_err(rpc_error)?
            {
                mobile::TxStatus::Pending => XrplTxStatus::Pending,
                mobile::TxStatus::Expired => XrplTxStatus::Expired,
                mobile::TxStatus::Validated {
                    ledger_index,
                    transaction_index,
                    transaction_result,
                    success,
                } => XrplTxStatus::Validated {
                    ledger_index,
                    transaction_index,
                    transaction_result,
                    success,
                },
            },
        )
    })
}
