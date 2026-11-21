use std::{sync::Arc, time::Duration};

use galaxy_protocol::{AccountId, Hash32};
use galaxy_verifier::{verify_proof_transaction, Report, State, VerifyContext};
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

use crate::{
    config::Limits,
    db::{Db, Job},
    sync::Limiter,
    unix_now, Config, Error, Result, SOFTWARE_VERSION,
};

const MAX_CONSECUTIVE_JOBS: u8 = 4;
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

struct Worker {
    limiter: Limiter,
    last_key: String,
    consecutive: u8,
}

impl Worker {
    fn new(limits: &Limits) -> Self {
        Self {
            limiter: Limiter::new(limits.verify_requests_per_second),
            last_key: String::new(),
            consecutive: 0,
        }
    }

    fn next_job(&self, db: &Db, cutoff: i64) -> Result<Option<Job>> {
        if self.consecutive == MAX_CONSECUTIVE_JOBS {
            if let Some(job) = db.job(cutoff, Some(&self.last_key))? {
                return Ok(Some(job));
            }
        }
        db.job(cutoff, None)
    }

    fn record_attempt(&mut self, key: String) {
        if self.last_key == key {
            self.consecutive = (self.consecutive + 1).min(MAX_CONSECUTIVE_JOBS);
        } else {
            self.last_key = key;
            self.consecutive = 1;
        }
    }

    async fn process(
        &mut self,
        config: &Config,
        db: &Db,
        rpc: &RpcClient,
        cutoff: i64,
    ) -> Result<bool> {
        let Some(job) = self.next_job(db, cutoff)? else {
            return Ok(false);
        };
        let mut hash = [0u8; 32];
        hex::decode_to_slice(&job.proof, &mut hash)
            .map_err(|_| Error::Invariant("invalid proof job hash"))?;
        // A job is keyed by exact PROOF hash, not by whichever sibling is valid.
        let verify = async {
            let mut attempts = rpc.pinned_endpoints().peekable();
            while let Some((url, pinned)) = attempts.next() {
                let ledger = RateLimited {
                    rpc: &pinned,
                    limit: &self.limiter,
                };
                let mut context = VerifyContext::new(&ledger, &config.network);
                // Public reports disclose only the origin (paths/queries can include credentials).
                context.rpc_endpoints.push(url.origin().ascii_serialization());
                if config.network.trusted_local_rippled.as_deref() == Some(url.as_str()) {
                    context.rpc_trust = galaxy_verifier::RpcTrust::Local;
                }
                let report = verify_proof_transaction(hash, &context).await;
                if !report.needs_rpc_retry() || attempts.peek().is_none() {
                    return Some(report);
                }
            }
            None
        };
        let report = timeout(VERIFY_TIMEOUT, verify).await.ok().flatten();
        finish_job(db, &job, unix_now() as i64, report.as_ref())?;
        self.record_attempt(job.fairness_key);
        Ok(true)
    }
}

fn finish_job(db: &Db, job: &Job, now: i64, report: Option<&Report>) -> Result<()> {
    if let Some(report) = report {
        let retry = matches!(
            report.state,
            State::Unavailable | State::Inconclusive | State::NotFound | State::Pending
        );
        let json = serde_json::to_string(report)?;
        if json.len() <= MAX_REPORT_BYTES {
            return db.store_report(job, now, report.version, &json, retry);
        }
    }
    db.defer_report(job, now)
}

pub(crate) async fn drain(config: &Config, db: &Db, rpc: &RpcClient) -> Result<()> {
    refresh(config, db)?;
    let cutoff = unix_now() as i64;
    let mut worker = Worker::new(&config.node.limits);
    while worker.process(config, db, rpc, cutoff).await? {}
    Ok(())
}

pub(crate) async fn run(
    config: Arc<Config>,
    db: Arc<Db>,
    rpc: RpcClient,
    wake: Arc<Notify>,
) -> Result<()> {
    refresh(&config, &db)?;
    let mut worker = Worker::new(&config.node.limits);
    loop {
        if !worker.process(&config, &db, &rpc, unix_now() as i64).await? {
            tokio::select! {
                () = sleep(Duration::from_secs(5)) => {},
                () = wake.notified() => {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use galaxy_verifier::TrustReport;
    use tokio::time::Instant;

    use super::*;
    use crate::{
        db::Range,
        records::{Record, CAPTURE, PROOF, REVOKE},
    };

    const ACCOUNT: &str = "raJ8s1YsReiYm53wEvZnnq2wveTDaEaSL4";

    fn hash(n: u8) -> String {
        hex::encode_upper([n; 32])
    }

    fn index(db: &Db, id: u8, kind: &str, capture: Option<u8>, registration: Option<u8>) {
        index_for(db, ACCOUNT, id, kind, capture, registration);
    }

    fn index_for(
        db: &Db,
        account: &str,
        id: u8,
        kind: &str,
        capture: Option<u8>,
        registration: Option<u8>,
    ) {
        let record = Record {
            hash: hash(id),
            ledger: 1,
            index: u32::from(id),
            account: Some(account.into()),
            kind: kind.into(),
            rejected: None,
            id: (kind == CAPTURE).then(|| hex::encode([id; 32])),
            reference: capture.map(hash),
            registration: registration.map(hash),
            bundle: None,
            counter: None,
            announcement: None,
        };
        db.insert_account_page(account, Range { from: 1, through: 1 }, &[record], true)
            .unwrap();
    }

    #[test]
    fn worker_yields_to_another_registration_at_the_consecutive_cap() {
        let db = Db::open_path(":memory:".as_ref()).unwrap();
        index(&db, 0x80, CAPTURE, None, Some(0xf0));
        index(&db, 0x81, CAPTURE, None, Some(0xf0));
        index(&db, 0x82, CAPTURE, None, Some(0xf1));
        for id in 1..=6 {
            index(&db, id, PROOF, Some(0x80 + id % 2), None);
        }
        index(&db, 0xfe, PROOF, Some(0x82), None);

        let mut worker = Worker::new(&Limits::default());
        for expected in [1, 2, 3, 4, 0xfe, 5, 6] {
            let job = worker.next_job(&db, 0).unwrap().unwrap();
            assert_eq!(job.proof, hash(expected));
            assert_eq!(
                job.fairness_key,
                format!("{ACCOUNT}:{}", hash(if expected == 0xfe { 0xf1 } else { 0xf0 }))
            );
            db.store_report(&job, 0, "galaxy-report/v1", "{}", false)
                .unwrap();
            worker.record_attempt(job.fairness_key);
        }
        assert!(worker.next_job(&db, 0).unwrap().is_none());

        // With no other due group the worker keeps making progress.
        for id in 7..=12 {
            index(&db, id, PROOF, Some(0x80), None);
            let job = worker.next_job(&db, 0).unwrap().unwrap();
            assert_eq!(job.proof, hash(id));
            db.store_report(&job, 0, "galaxy-report/v1", "{}", false)
                .unwrap();
            worker.record_attempt(job.fairness_key);
        }
        assert_eq!(worker.consecutive, MAX_CONSECUTIVE_JOBS);
    }

    #[test]
    fn unverified_registration_claims_are_scoped_to_the_proof_sender() {
        let db = Db::open_path(":memory:".as_ref()).unwrap();
        const OTHER: &str = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
        index(&db, 0x80, CAPTURE, None, Some(0xf0));
        index_for(&db, OTHER, 0x81, CAPTURE, None, Some(0xf0));
        index(&db, 1, PROOF, Some(0x80), None);
        index_for(&db, OTHER, 2, PROOF, Some(0x81), None);
        let first = db.job(0, None).unwrap().unwrap();
        assert_eq!(first.fairness_key, format!("{ACCOUNT}:{}", hash(0xf0)));
        let second = db.job(0, Some(&first.fairness_key)).unwrap().unwrap();
        assert_eq!(second.proof, hash(2));
        assert_eq!(second.fairness_key, format!("{OTHER}:{}", hash(0xf0)));
    }

    #[test]
    fn retries_persist_per_proof_and_back_off_after_unavailability_and_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.sqlite3");
        let db = Db::open_path(&path).unwrap();
        index(&db, 0x80, CAPTURE, None, Some(0xf0));
        index(&db, 1, PROOF, Some(0x80), None);
        index(&db, 2, PROOF, Some(0x80), None);
        let mut report = Report::unavailable(
            "fixture".into(),
            "proof-only",
            TrustReport::default(),
            "RPC unavailable",
        );
        let mut now = 1000;
        let job = db.job(now, None).unwrap().unwrap();
        assert_eq!(job.proof, hash(1));
        finish_job(&db, &job, now, Some(&report)).unwrap();
        let unaffected = db.job(now, None).unwrap().unwrap();
        assert_eq!(unaffected.proof, hash(2));
        db.store_report(&unaffected, now, report.version, "{}", false)
            .unwrap();
        drop(db);

        let db = Db::open_path(&path).unwrap();
        // Revocation clears the cached report but cannot bypass a retry delay.
        index(&db, 0x90, REVOKE, None, Some(0xf0));
        let invalidated = db.job(now, None).unwrap().unwrap();
        assert_eq!(invalidated.proof, hash(2));
        db.store_report(&invalidated, now, report.version, "{}", false)
            .unwrap();
        for delay in [60, 120, 240, 480, 960, 1920, 3840, 7680, 15360, 21600] {
            assert!(db.job(now + delay - 1, None).unwrap().is_none());
            now += delay;
            let job = db.job(now, None).unwrap().unwrap();
            assert_eq!(job.proof, hash(1));
            // No report is yielded when the outer verification deadline fires.
            finish_job(&db, &job, now, None).unwrap();
        }
        assert!(db.job(now + 21599, None).unwrap().is_none());
        now += 21600;
        let job = db.job(now, None).unwrap().unwrap();
        report.state = State::Invalid;
        finish_job(&db, &job, now, Some(&report)).unwrap();
        assert!(db.job(i64::MAX, None).unwrap().is_none());

        // A subsequent invalidation of a terminal report starts a clean backoff.
        index(&db, 0x91, REVOKE, None, Some(0xf0));
        let job = db.job(now, None).unwrap().unwrap();
        assert_eq!(job.proof, hash(1));
        finish_job(&db, &job, now, None).unwrap();
        let second = db.job(now, None).unwrap().unwrap();
        finish_job(&db, &second, now, Some(&report)).unwrap();
        assert!(db.job(now + 59, None).unwrap().is_none());
        assert_eq!(db.job(now + 60, None).unwrap().unwrap().proof, hash(1));
    }

    #[test]
    fn migrates_v1_queue_without_discarding_proofs_or_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.sqlite3");
        let db = Db::open_path(&path).unwrap();
        index(&db, 0x80, CAPTURE, None, Some(0xf0));
        index(&db, 1, PROOF, Some(0x80), None);
        index(&db, 2, PROOF, Some(0x81), None);
        drop(db);

        // Restore the v1 table shape and load an example pre-upgrade schedule.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX report_queue;
             ALTER TABLE reports DROP COLUMN fairness_key;
             ALTER TABLE reports DROP COLUMN retry_delay_secs;
             CREATE INDEX report_queue ON reports(due,proof) WHERE due IS NOT NULL;
             UPDATE reports SET due=42,generation=7;
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(conn);

        let db = Db::open_path(&path).unwrap();
        assert!(db.job(41, None).unwrap().is_none());
        let known = db.job(42, None).unwrap().unwrap();
        assert_eq!(known.proof, hash(1));
        assert_eq!(known.generation, 7);
        assert_eq!(known.fairness_key, format!("{ACCOUNT}:{}", hash(0xf0)));
        let unresolved = db.job(42, Some(&known.fairness_key)).unwrap().unwrap();
        assert_eq!(unresolved.proof, hash(2));
        assert_eq!(unresolved.fairness_key, ACCOUNT);
        db.defer_report(&known, 42).unwrap();
        assert!(db.job(101, Some(ACCOUNT)).unwrap().is_none());
        assert_eq!(db.job(102, Some(ACCOUNT)).unwrap().unwrap().proof, hash(1));
    }

    #[tokio::test(start_paused = true)]
    async fn verification_and_sync_have_independent_rpc_rate_budgets() {
        let limits = Limits {
            sync_requests_per_second: 10,
            verify_requests_per_second: 1,
            ..Limits::default()
        };
        let worker = Worker::new(&limits);
        let sync = Limiter::new(limits.sync_requests_per_second);
        let start = Instant::now();
        worker.limiter.acquire().await;
        sync.acquire().await;
        assert!(Instant::now() - start < Duration::from_millis(100));

        let (proof, ledger) = tokio::join!(
            async {
                worker.limiter.acquire().await;
                Instant::now() - start
            },
            async {
                sync.acquire().await;
                Instant::now() - start
            },
        );
        assert!(ledger >= Duration::from_millis(100) && ledger < proof);
        assert!(proof >= Duration::from_secs(1));
    }
}
