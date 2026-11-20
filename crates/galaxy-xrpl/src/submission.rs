//! Reliable submission: persist the exact signed bytes before submitting. The
//! caller controls the clock, delays and I/O through transport/store adapters.

use crate::keys::{classic_address, tx_hash, Wallet};
use crate::ledger::{parse_tx, LedgerTx, RpcTransport};
use crate::transaction::{decode_galaxy_object, AccountSet, Memo};
use crate::{codec, Error, Hash32};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;

/// Pending polls between rebroadcasts of the identical blob in [`Submitter::run`].
pub const REBROADCAST_POLLS: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTx {
    pub purpose: String,
    pub hash: Hash32,
    pub blob: Vec<u8>,
    pub network_id: u32,
    pub first_possible_ledger: u32,
    pub last_ledger: u32,
}

impl PendingTx {
    fn check(&self, purpose: &str, network_id: u32) -> Result<Value, Error> {
        check_purpose(purpose)?;
        if self.purpose != purpose || self.network_id != network_id
            || self.first_possible_ledger == 0
            || self.last_ledger.checked_sub(self.first_possible_ledger).is_none_or(|n| n >= 20)
            || tx_hash(&self.blob) != self.hash
        {
            return Err(Error::Invalid("invalid persisted transaction identity/window"));
        }
        let decoded = codec::decode(&self.blob)?;
        if decoded["LastLedgerSequence"] != self.last_ledger
            || match decoded.get("NetworkID") {
                Some(id) => network_id <= 1024 || id != network_id,
                None => network_id > 1024,
            }
        {
            return Err(Error::Invalid("persisted transaction bounds do not match blob"));
        }
        Ok(decoded)
    }

    pub(crate) fn check_fee(&self, network_id: u32, max_fee_drops: u64) -> Result<Value, Error> {
        let decoded = self.check(&self.purpose, network_id)?;
        let fee = decoded["Fee"].as_str().and_then(|fee| fee.parse::<u64>().ok());
        if !fee.is_some_and(|fee| fee > 0 && fee <= max_fee_drops) {
            return Err(Error::Invalid("persisted transaction fee exceeds configured cap"));
        }
        Ok(decoded)
    }
}

/// `compare_and_swap` must be atomic and durable before returning `true`.
/// Expected `None` inserts only if no record exists. Purposes are scoped to the
/// store; serialize distinct purposes for the same wallet (one live Sequence).
pub trait TxStore {
    fn load(&self, purpose: &str) -> Result<Option<PendingTx>, Error>;
    fn compare_and_swap(&self, expected: Option<Hash32>, tx: &PendingTx) -> Result<bool, Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultClass {
    Success,
    ClaimedFee,
    Failure,
    Malformed,
    Local,
    Retry,
    Unknown,
}

pub fn classify_result(code: &str) -> ResultClass {
    if code == "tesSUCCESS" { return ResultClass::Success; }
    match code.get(..3) {
        Some("tec") => ResultClass::ClaimedFee,
        Some("tef") => ResultClass::Failure,
        Some("tem") => ResultClass::Malformed,
        Some("tel") => ResultClass::Local,
        Some("ter") => ResultClass::Retry,
        _ => ResultClass::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitResult {
    pub code: String,
    pub class: ResultClass,
}

pub enum TxState {
    Pending,
    Expired,
    Validated(LedgerTx),
}

impl TxState {
    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Validated(tx) if tx.validated && tx.transaction_result == "tesSUCCESS")
    }
}

pub struct Submitter<'a, R, S> {
    rpc: &'a R,
    store: &'a S,
    network_id: u32,
    allow_mainnet: bool,
    max_fee_drops: u64,
}

pub(crate) async fn checked_head<R: RpcTransport>(rpc: &R, network_id: u32, allow_mainnet: bool) -> Result<u32, Error> {
    if network_id == 0 && !allow_mainnet {
        return Err(Error::Network);
    }
    let status = request(rpc, "server_info", json!({"api_version": 2})).await?;
    let info = crate::ledger::parse_server_info(&status)?;
    if info.network_id != network_id { return Err(Error::Network); }
    Ok(info.validated_ledger)
}

pub(crate) async fn request<R: RpcTransport>(rpc: &R, method: &str, params: Value) -> Result<Value, Error> {
    let value = rpc.request(method, params).await?;
    if let Some(code) = value["error"].as_str() {
        return Err(Error::Rpc(format!("{method}: {code}")));
    }
    Ok(value)
}

fn index(value: &Value) -> Result<u32, Error> {
    value.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|&n| n != 0)
        .ok_or(Error::Invalid("missing or invalid ledger index/sequence"))
}

/// Common fields of a Galaxy AccountSet derived from validated RPC answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Autofill {
    pub sequence: u32,
    pub fee_drops: u64,
    /// `min(validated + 20, deadline)`.
    pub last_ledger_sequence: u32,
    /// `validated + 1`: the first ledger that can include the transaction.
    pub first_possible_ledger: u32,
}

/// `LastLedgerSequence = min(validated + 20, deadline)`; fails once the
/// deadline is not after the validated ledger.
pub fn last_ledger_for(validated_ledger: u32, deadline: Option<u32>) -> Result<u32, Error> {
    if validated_ledger == 0 {
        return Err(Error::Invalid("cannot autofill without a validated ledger"));
    }
    let last_ledger = validated_ledger.saturating_add(20).min(deadline.unwrap_or(u32::MAX));
    if last_ledger <= validated_ledger { return Err(Error::Invalid("transaction deadline already passed")); }
    Ok(last_ledger)
}

/// `account_info` parameters pinned to the validated ledger used for autofill.
pub fn account_info_params(address: &str, validated_ledger: u32) -> Value {
    json!({"account": address, "ledger_index": validated_ledger})
}

/// Sans-IO autofill from the `fee` result and the `account_info` result
/// requested with [`account_info_params`]. The fee is the open-ledger fee and
/// must not exceed `max_fee_drops`.
pub fn autofill_from_responses(
    validated_ledger: u32, deadline: Option<u32>, max_fee_drops: u64, address: &str, fee: &Value, account_info: &Value,
) -> Result<Autofill, Error> {
    let last_ledger_sequence = last_ledger_for(validated_ledger, deadline)?;
    let fee_drops = fee["drops"]["open_ledger_fee"].as_str().and_then(|n| n.parse().ok())
        .filter(|&n| n > 0 && n <= max_fee_drops)
        .ok_or(Error::Invalid("missing fee quote or fee exceeds configured cap"))?;
    if account_info["validated"] != true || account_info["ledger_index"] != validated_ledger
        || account_info["account_data"]["Account"] != address
    {
        return Err(Error::Invalid("account_info does not match validated account/ledger"));
    }
    let sequence = index(&account_info["account_data"]["Sequence"])?;
    Ok(Autofill { sequence, fee_drops, last_ledger_sequence, first_possible_ledger: validated_ledger + 1 })
}

/// `tx` parameters for looking up a persisted transaction. Once the validated
/// ledger passed `last_ledger`, the bounded window lets the server prove absence.
pub fn tx_lookup_params(pending: &PendingTx, validated_ledger: u32) -> Value {
    let mut params = json!({"transaction":hex::encode_upper(pending.hash),"binary":true,"api_version":2});
    if validated_ledger > pending.last_ledger {
        params["min_ledger"] = json!(pending.first_possible_ledger);
        params["max_ledger"] = json!(pending.last_ledger);
    }
    params
}

/// Sans-IO evaluation of a `tx` result obtained with [`tx_lookup_params`] for
/// the same `validated_ledger`. `txnNotFound` alone is never evidence of expiry;
/// the server must report that it searched every ledger in the bounded window.
pub fn evaluate_tx_lookup(pending: &PendingTx, validated_ledger: u32, result: &Value) -> Result<TxState, Error> {
    let past = validated_ledger > pending.last_ledger;
    match result["error"].as_str() {
        Some("txnNotFound") if past && result["searched_all"] == true => return Ok(TxState::Expired),
        Some("txnNotFound") if past => return Err(Error::Rpc("expiry not provable: RPC did not search full ledger window".into())),
        Some("txnNotFound") => return Ok(TxState::Pending),
        Some(code) => return Err(Error::Rpc(format!("tx: {code}"))),
        None => {}
    }
    if result["validated"] == false && !past { return Ok(TxState::Pending); }
    let tx = parse_tx(result)?;
    if tx.hash != pending.hash || tx.tx_blob != pending.blob || !tx.validated
        || !(pending.first_possible_ledger..=pending.last_ledger).contains(&tx.ledger_index)
    {
        return Err(Error::Invalid("RPC returned a different tx, non-final tx, or out-of-range ledger"));
    }
    if !matches!(classify_result(&tx.transaction_result), ResultClass::Success | ResultClass::ClaimedFee) {
        return Err(Error::Invalid("invalid result for a validated transaction"));
    }
    Ok(TxState::Validated(tx))
}

/// Checks a `submit` result for the persisted blob. Engine results are
/// provisional, even `tesSUCCESS`; only a validated lookup is final.
pub fn evaluate_submit(pending: &PendingTx, result: &Value) -> Result<SubmitResult, Error> {
    if let Some(code) = result["error"].as_str() { return Err(Error::Rpc(format!("submit: {code}"))); }
    if let Some(blob) = result.get("tx_blob") {
        if blob.as_str().and_then(|s| hex::decode(s).ok()).as_deref() != Some(&pending.blob) {
            return Err(Error::Invalid("submit RPC returned a different blob"));
        }
    }
    let code = result["engine_result"].as_str().ok_or(Error::Invalid("missing submit engine result"))?.to_owned();
    Ok(SubmitResult { class: classify_result(&code), code })
}

impl PendingTx {
    /// Builds the journal record of a freshly signed transaction (the caller
    /// persists it durably before the first submit).
    pub fn new(purpose: &str, signed: crate::transaction::SignedTransaction, network_id: u32, autofill: &Autofill) -> Result<Self, Error> {
        let pending = Self { purpose: purpose.to_owned(), hash: signed.hash, blob: signed.blob, network_id,
            first_possible_ledger: autofill.first_possible_ledger, last_ledger: autofill.last_ledger_sequence };
        pending.check(purpose, network_id)?;
        Ok(pending)
    }

    /// Integrity of a journal record: hash, network and ledger window match the blob.
    pub fn verify(&self, network_id: u32) -> Result<(), Error> {
        self.check(&self.purpose, network_id).map(drop)
    }
}

fn check_purpose(purpose: &str) -> Result<(), Error> {
    if purpose.is_empty() || purpose.len() > 256 || !purpose.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Invalid("invalid transaction purpose"));
    }
    Ok(())
}

impl<'a, R: RpcTransport, S: TxStore> Submitter<'a, R, S> {
    pub fn new(rpc: &'a R, store: &'a S, network_id: u32, allow_mainnet: bool, max_fee_drops: u64) -> Result<Self, Error> {
        if network_id == 0 && !allow_mainnet { return Err(Error::Network); }
        if max_fee_drops == 0 { return Err(Error::Invalid("zero max_fee_drops")); }
        Ok(Self { rpc, store, network_id, allow_mainnet, max_fee_drops })
    }

    pub fn network_id(&self) -> u32 { self.network_id }

    pub async fn validated_ledger(&self) -> Result<u32, Error> {
        checked_head(self.rpc, self.network_id, self.allow_mainnet).await
    }

    async fn build(&self, purpose: &str, wallet: &Wallet, memo: Memo<'_>, deadline: Option<u32>) -> Result<PendingTx, Error> {
        let head = self.validated_ledger().await?;
        last_ledger_for(head, deadline)?;
        let fee = request(self.rpc, "fee", json!({})).await?;
        let address = wallet.classic_address();
        let account = request(self.rpc, "account_info", account_info_params(&address, head)).await?;
        let autofill = autofill_from_responses(head, deadline, self.max_fee_drops, &address, &fee, &account)?;
        let signed = AccountSet { account: wallet.account_id(), sequence: autofill.sequence, fee_drops: autofill.fee_drops,
            last_ledger_sequence: autofill.last_ledger_sequence, network_id: self.network_id, memo }.sign(wallet)?;
        PendingTx::new(purpose, signed, self.network_id, &autofill)
    }

    fn check_intent(&self, pending: &PendingTx, purpose: &str, wallet: &Wallet, memo: Memo<'_>, deadline: Option<u32>) -> Result<(), Error> {
        let decoded = pending.check(purpose, self.network_id)?;
        let actual = decode_galaxy_object(&decoded)?;
        if actual.account != wallet.account_id() || actual.kind != memo.kind || actual.data != memo.data {
            return Err(Error::Invalid("persisted purpose is bound to a different account or memo"));
        }
        if deadline.is_some_and(|deadline| pending.last_ledger > deadline) {
            return Err(Error::Invalid("persisted transaction exceeds caller's deadline"));
        }
        Ok(())
    }

    /// Reuse the existing blob for this purpose even if the fee quote or
    /// sequence has since changed. Never creates a new attempt automatically.
    pub async fn prepare(&self, purpose: &str, wallet: &Wallet, memo: Memo<'_>, deadline: Option<u32>) -> Result<PendingTx, Error> {
        check_purpose(purpose)?;
        if let Some(existing) = self.store.load(purpose)? {
            self.check_intent(&existing, purpose, wallet, memo, deadline)?;
            return Ok(existing);
        }
        let pending = self.build(purpose, wallet, memo, deadline).await?;
        if self.store.compare_and_swap(None, &pending)? { return Ok(pending); }
        let existing = self.store.load(purpose)?.ok_or(Error::Invalid("transaction store lost persisted entry"))?;
        self.check_intent(&existing, purpose, wallet, memo, deadline)?;
        Ok(existing)
    }

    /// Create a replacement only after the entire old window was searched by
    /// the RPC and a validated ledger cleared the deadline. CAPTURE callers
    /// must explicitly choose whether to replace and supply the capture deadline.
    pub async fn replace_expired(&self, purpose: &str, wallet: &Wallet, memo: Memo<'_>, deadline: Option<u32>) -> Result<PendingTx, Error> {
        check_purpose(purpose)?;
        let old = self.store.load(purpose)?.ok_or(Error::Invalid("no transaction to replace"))?;
        self.check_intent(&old, purpose, wallet, memo, None)?;
        if !matches!(self.state(&old).await?, TxState::Expired) {
            return Err(Error::Invalid("previous transaction is not provably expired"));
        }
        let new = self.build(purpose, wallet, memo, deadline).await?;
        if self.store.compare_and_swap(Some(old.hash), &new)? { return Ok(new); }
        Err(Error::Invalid("persisted transaction was concurrently replaced"))
    }

    /// Observe validation/expiry. `txnNotFound` alone is never evidence of
    /// expiry; the server must search every ledger in the bounded window.
    pub async fn state(&self, pending: &PendingTx) -> Result<TxState, Error> {
        pending.check(&pending.purpose, self.network_id)?;
        self.lookup(pending).await
    }

    async fn lookup(&self, pending: &PendingTx) -> Result<TxState, Error> {
        let head = self.validated_ledger().await?;
        let result = self.rpc.request("tx", tx_lookup_params(pending, head)).await?;
        evaluate_tx_lookup(pending, head, &result)
    }

    /// Submit exactly the persisted blob. Submission engine results are
    /// provisional, even `tesSUCCESS`; call `state` or `run` for a final outcome.
    pub async fn submit_saved(&self, pending: &PendingTx) -> Result<SubmitResult, Error> {
        pending.check_fee(self.network_id, self.max_fee_drops)?;
        let stored = self.store.load(&pending.purpose)?;
        if stored.as_ref() != Some(pending) { return Err(Error::Invalid("tx is not the persisted transaction for this purpose")); }
        if self.validated_ledger().await? >= pending.last_ledger {
            return Err(Error::Invalid("refusing to submit against an already closed last ledger"));
        }
        let result = request(self.rpc, "submit", json!({"tx_blob":hex::encode_upper(&pending.blob)})).await?;
        evaluate_submit(pending, &result)
    }

    /// Safe after a crash before or after the original submit. A transport
    /// error leaves the purpose durable and can be retried by calling `run`.
    /// While pending, the identical blob is rebroadcast every
    /// `REBROADCAST_POLLS` polls until the window closes.
    pub async fn run<D, F>(&self, pending: &PendingTx, delay: D) -> Result<TxState, Error>
    where D: Fn() -> F, F: Future<Output = ()>,
    {
        let state = self.state(pending).await?;
        if !matches!(state, TxState::Pending) { return Ok(state); }
        if self.validated_ledger().await? < pending.last_ledger {
            // Network failure is ambiguous: recovery starts by looking up the hash again.
            let attempt = self.submit_saved(pending).await;
            if let Err(err) = attempt {
                // The last ledger may have closed between the two reads above.
                if self.validated_ledger().await? < pending.last_ledger { return Err(err); }
            }
        }
        // A provisional `tel*`/`ter*` result or a dropped relay can leave the
        // transaction out of every queue: rebroadcast the identical blob (same
        // hash and Sequence, so at most one copy can ever apply) while it is open.
        let mut polls = 0u32;
        loop {
            match self.lookup(pending).await? {
                TxState::Pending => {
                    polls += 1;
                    if polls.is_multiple_of(REBROADCAST_POLLS) {
                        // The next lookup is authoritative; a rebroadcast that fails
                        // (window closing, transport error) changes nothing.
                        let _ = self.submit_saved(pending).await;
                    }
                    delay().await;
                }
                final_state => return Ok(final_state),
            }
        }
    }

    pub async fn poll<D, F>(&self, pending: &PendingTx, delay: D) -> Result<TxState, Error>
    where D: Fn() -> F, F: Future<Output = ()>,
    {
        pending.check(&pending.purpose, self.network_id)?;
        loop {
            match self.lookup(pending).await? {
                TxState::Pending => delay().await,
                final_state => return Ok(final_state),
            }
        }
    }
}

#[cfg(unix)]
mod file_store {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    pub struct FileTxStore { dir: PathBuf }

    impl FileTxStore {
        pub fn new(dir: impl Into<PathBuf>) -> Result<Self, Error> {
            let dir = dir.into();
            Self::create_dir(&dir)?;
            Ok(Self { dir })
        }

        fn create_dir(path: &Path) -> Result<(), Error> {
            if path.is_dir() { return Ok(()); }
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            Self::create_dir(parent)?;
            match DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
                Err(err) => return Err(err.into()),
            }
            File::open(parent)?.sync_all()?;
            Ok(())
        }

        fn name(purpose: &str) -> String { hex::encode(Sha256::digest(purpose)) }

        fn read(path: &Path) -> Result<Option<PendingTx>, Error> {
            const MAX_BYTES: u64 = 16384;
            let file = match File::open(path) {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(err) => return Err(err.into()),
            };
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_BYTES { return Err(Error::Invalid("transaction journal entry is oversized")); }
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
    }

    impl TxStore for FileTxStore {
        fn load(&self, purpose: &str) -> Result<Option<PendingTx>, Error> {
            check_purpose(purpose)?;
            Self::read(&self.dir.join(Self::name(purpose)).with_extension("json"))
        }

        fn compare_and_swap(&self, expected: Option<Hash32>, tx: &PendingTx) -> Result<bool, Error> {
            check_purpose(&tx.purpose)?;
            let base = self.dir.join(Self::name(&tx.purpose));
            let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
                .open(base.with_extension("lock"))?;
            lock.lock()?;
            let path = base.with_extension("json");
            if Self::read(&path)?.map(|record| record.hash) != expected { return Ok(false); }
            let temp = base.with_extension("tmp");
            let mut file = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temp)?;
            serde_json::to_writer(&mut file, tx)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(temp, path)?;
            File::open(&self.dir)?.sync_all()?;
            Ok(true)
        }
    }
}

#[cfg(unix)]
pub use file_store::FileTxStore;

/// Validate account_info matching outside the submission state machine.
pub async fn balance<R: RpcTransport>(rpc: &R, account: &crate::AccountId) -> Result<u64, Error> {
    let address = classic_address(account);
    let response = request(rpc, "account_info", json!({"account":address,"ledger_index":"validated"})).await?;
    if response["validated"] != true || response["account_data"]["Account"] != address {
        return Err(Error::Invalid("account_info does not match validated account"));
    }
    response["account_data"]["Balance"].as_str().and_then(|n| n.parse().ok())
        .ok_or(Error::Invalid("missing or invalid validated balance"))
}
