use std::{sync::Arc, time::Duration};

use galaxy_protocol::{AccountId, Hash32};
use galaxy_verifier::{verify_proof_transaction, State, VerifyContext};
use galaxy_xrpl::{
    ledger::{AccountRoot, AccountTxPage, LedgerHeader, LedgerTx, ServerInfo},
    native::RpcClient,
    LedgerSource,
};
use serde_json::Value;
use tokio::{
    sync::Notify,
    time::{sleep, timeout},
};

use crate::{db::Db, sync::Limiter, unix_now, Config, Error, Result, SOFTWARE_VERSION};

const RETRY_SECONDS: i64 = 60;
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REPORT_BYTES: usize = 96 * 1024;

struct RateLimited<'a> {
    rpc: &'a RpcClient,
    limit: &'a Limiter,
}

impl LedgerSource for RateLimited<'_> {
    async fn tx(&self, hash: &Hash32) -> Result<Option<LedgerTx>, galaxy_xrpl::Error> {
        self.limit.acquire().await;
        self.rpc.tx(hash).await
    }

    async fn ledger_header(&self, index: u32) -> Result<LedgerHeader, galaxy_xrpl::Error> {
        self.limit.acquire().await;
        self.rpc.ledger_header(index).await
    }

    async fn account_root(&self, account: &AccountId, index: u32) -> Result<AccountRoot, galaxy_xrpl::Error> {
        self.limit.acquire().await;
        self.rpc.account_root(account, index).await
    }

    async fn account_tx(
        &self,
        account: &AccountId,
        min: u32,
        max: u32,
        marker: Option<Value>,
        limit: u16,
    ) -> Result<AccountTxPage, galaxy_xrpl::Error> {
        self.limit.acquire().await;
        self.rpc.account_tx(account, min, max, marker, limit).await
    }

    async fn server_info(&self) -> Result<ServerInfo, galaxy_xrpl::Error> {
        self.limit.acquire().await;
        self.rpc.server_info().await
    }
}

fn refresh(config: &Config, db: &Db) -> Result<()> {
    db.refresh_reports(&format!(
        "{SOFTWARE_VERSION}:exact-proof-v1-account-txn-id:{:?}:{:?}:{:?}",
        config.network.accepted_circuits,
        config.network.accepted_app_ids,
        config.network.accepted_attestation_environments
    ))
}

async fn process(
    config: &Config,
    db: &Db,
    rpc: &RpcClient,
    limiter: &Limiter,
    cutoff: i64,
) -> Result<bool> {
    let Some((proof, generation)) = db.job(cutoff)? else {
        return Ok(false);
    };
    let mut hash = [0u8; 32];
    hex::decode_to_slice(&proof, &mut hash)
        .map_err(|_| Error::Invariant("invalid proof job hash"))?;
    // A job is keyed by exact PROOF hash, not by whichever sibling is valid.
    let verify = async {
        let mut attempts = rpc.pinned_endpoints().peekable();
        let mut report = None;
        while let Some((url, pinned)) = attempts.next() {
            let ledger = RateLimited { rpc: &pinned, limit: limiter };
            let mut context = VerifyContext::new(&ledger, &config.network);
            // Public reports disclose only the origin (paths/queries can include credentials).
            context.rpc_endpoints.push(url.origin().ascii_serialization());
            if config.network.trusted_local_rippled.as_deref() == Some(url.as_str()) {
                context.rpc_trust = galaxy_verifier::RpcTrust::Local;
            }
            let attempt = verify_proof_transaction(hash, &context).await;
            let retry = attempt.needs_rpc_retry() && attempts.peek().is_some();
            report = Some(attempt);
            if !retry { break; }
        }
        report
    };
    if let Ok(Some(report)) = timeout(VERIFY_TIMEOUT, verify).await {
        let now = unix_now() as i64;
        let retry = matches!(
            report.state,
            State::Unavailable | State::Inconclusive | State::NotFound | State::Pending
        )
        .then_some(now + RETRY_SECONDS);
        let json = serde_json::to_string(&report)?;
        if json.len() <= MAX_REPORT_BYTES {
            db.store_report(&proof, generation, now, report.version, &json, retry)?;
            return Ok(true);
        }
    }
    db.defer_report(&proof, generation, unix_now() as i64 + RETRY_SECONDS)?;
    Ok(true)
}

pub(crate) async fn drain(
    config: &Config,
    db: &Db,
    rpc: &RpcClient,
    limiter: &Limiter,
) -> Result<()> {
    refresh(config, db)?;
    let cutoff = unix_now() as i64;
    while process(config, db, rpc, limiter, cutoff).await? {}
    Ok(())
}

pub(crate) async fn run(
    config: Arc<Config>,
    db: Arc<Db>,
    rpc: RpcClient,
    limiter: Arc<Limiter>,
    wake: Arc<Notify>,
) -> Result<()> {
    refresh(&config, &db)?;
    loop {
        if !process(&config, &db, &rpc, &limiter, unix_now() as i64).await? {
            tokio::select! {
                () = sleep(Duration::from_secs(5)) => {},
                () = wake.notified() => {},
            }
        }
    }
}
