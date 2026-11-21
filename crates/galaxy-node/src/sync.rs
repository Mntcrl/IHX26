use std::{collections::HashSet, sync::Arc, time::Duration};

use futures_util::{stream, StreamExt};
use galaxy_protocol::{AccountId, Hash32};
use galaxy_xrpl::{
    keys,
    ledger::LedgerBackfill,
    native::{RpcClient, SubscriptionEvent},
    LedgerSource,
};
use tokio::{
    sync::{Mutex, Notify},
    time::{sleep, sleep_until, Instant},
};

use crate::{
    config::SyncMode,
    db::{Db, Range},
    records::{self, Record},
    Config, Error, Result, RETRY_DELAY,
};

const HISTORY_IN_FLIGHT: usize = 16;

pub(crate) struct Limiter {
    next: Mutex<Instant>,
    spacing: Duration,
}

impl Limiter {
    pub fn new(per_second: u32) -> Self {
        Self {
            next: Mutex::new(Instant::now()),
            spacing: Duration::from_secs(1) / per_second,
        }
    }

    pub async fn acquire(&self) {
        let mut next = self.next.lock().await;
        sleep_until(*next).await;
        *next = Instant::now() + self.spacing;
    }
}

pub(crate) fn rpc_client(config: &Config) -> Result<RpcClient> {
    Ok(RpcClient::new(
        config.rpc_urls(),
        config.network.network_id.as_u32(),
        config.network.allow_mainnet,
    )?)
}

pub(crate) struct Sources(Vec<RpcClient>);

impl Sources {
    pub fn new(config: &Config) -> Result<Self> {
        let mut unique = HashSet::new();
        Ok(Self(
            config
                .rpc_urls()
                .into_iter()
                .filter(|url| unique.insert(*url))
                .map(|url| {
                    RpcClient::new(
                        [url],
                        config.network.network_id.as_u32(),
                        config.network.allow_mainnet,
                    )
                })
                .collect::<std::result::Result<_, _>>()?,
        ))
    }

    async fn available(&self, db: &Db, limiter: &Limiter) -> Result<Vec<(&RpcClient, Vec<Range>)>> {
        let mut out = Vec::with_capacity(self.0.len());
        let mut last_error = None;
        for rpc in &self.0 {
            limiter.acquire().await;
            match rpc.server_info().await {
                Ok(info) => {
                    db.put_tip(info.validated_ledger)?;
                    match parse_complete(&info.complete_ledgers, info.validated_ledger) {
                        Ok(ranges) => out.push((rpc, ranges)),
                        Err(error) => last_error = Some(error),
                    }
                }
                Err(error) => last_error = Some(error.into()),
            }
        }
        if out.is_empty() {
            return Err(last_error.unwrap_or(Error::Invariant("no configured XRPL endpoints")));
        }
        Ok(out)
    }

    async fn live_ledger(
        &self,
        index: u32,
        hash: Hash32,
        count: Option<u32>,
        limiter: &Limiter,
    ) -> Result<LedgerBackfill> {
        let mut error = Error::Chain(index);
        for rpc in &self.0 {
            limiter.acquire().await;
            match rpc.ledger_transactions(index).await {
                Ok(ledger)
                    if ledger.header.hash == hash
                        && count.is_none_or(|n| n as usize == ledger.transactions.len()) =>
                {
                    return Ok(ledger)
                }
                Ok(_) => error = Error::Chain(index),
                Err(e) => error = e.into(),
            }
        }
        Err(error)
    }
}

fn parse_complete(text: &str, tip: u32) -> Result<Vec<Range>> {
    if matches!(text, "empty" | "") {
        return Ok(Vec::new());
    }
    let mut ranges: Vec<Range> = Vec::new();
    for part in text.split(',') {
        let (from, through) = part.split_once('-').unwrap_or((part, part));
        let invalid = || {
            Error::Xrpl(galaxy_xrpl::Error::Invalid(
                "invalid complete_ledgers range",
            ))
        };
        let from = from.parse::<u32>().map_err(|_| invalid())?;
        let through = through.parse::<u32>().map_err(|_| invalid())?;
        if from == 0 || from > through {
            return Err(invalid());
        }
        if from <= tip {
            ranges.push(Range {
                from,
                through: through.min(tip),
            });
        }
    }
    ranges.sort_unstable_by_key(|range| range.from);
    let mut merged: Vec<Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged
            .last_mut()
            .filter(|last| range.from <= last.through.saturating_add(1))
        {
            last.through = last.through.max(range.through);
        } else {
            merged.push(range);
        }
    }
    Ok(merged)
}

fn collect(
    txs: &[galaxy_xrpl::ledger::LedgerTx],
    config: &Config,
    account: Option<&AccountId>,
) -> Result<Vec<Record>> {
    let mut rows = Vec::new();
    for tx in txs {
        if let Some(record) = records::parse(tx, config.network.network_id.as_u32(), account)? {
            rows.push(record);
        }
    }
    Ok(rows)
}

fn store_ledger(config: &Config, db: &Db, ledger: LedgerBackfill) -> Result<()> {
    db.insert_ledger(
        &ledger.header,
        &collect(&ledger.transactions, config, None)?,
    )
}

async fn scan_account(
    rpc: &RpcClient,
    config: &Config,
    db: &Db,
    limiter: &Limiter,
    scope: &str,
    range: Range,
) -> Result<()> {
    let account = keys::decode_address(scope)?;
    let mut marker = None;
    let mut previous = None;
    // A pass is bounded even if an endpoint supplies a cyclic marker or
    // keeps returning empty pages. A later pass can try another endpoint.
    for _ in 0..1024 {
        limiter.acquire().await;
        let mut page = rpc
            .account_tx(&account, range.from, range.through, marker, 200)
            .await?;
        if page.transactions.is_empty() && page.marker.is_some() {
            return Err(Error::Xrpl(galaxy_xrpl::Error::Invalid(
                "empty account page with marker",
            )));
        }
        page.transactions
            .sort_unstable_by_key(|tx| (tx.ledger_index, tx.transaction_index));
        for transaction in &page.transactions {
            let position = (transaction.ledger_index, transaction.transaction_index);
            if previous.is_some_and(|p| position <= p) {
                return Err(Error::Xrpl(galaxy_xrpl::Error::Invalid(
                    "account history did not advance",
                )));
            }
            previous = Some(position);
        }
        let complete = page.marker.is_none();
        db.insert_account_page(
            scope,
            range,
            &collect(&page.transactions, config, Some(&account))?,
            complete,
        )?;
        if complete {
            return Ok(());
        }
        marker = page.marker;
    }
    Err(Error::Xrpl(galaxy_xrpl::Error::Invalid(
        "account page limit reached",
    )))
}

async fn fetch_history(
    available: &[(&RpcClient, Vec<Range>)],
    limiter: &Limiter,
    index: u32,
    next_source: usize,
    mut error: Error,
) -> Result<(usize, LedgerBackfill)> {
    for (source, (rpc, ranges)) in available.iter().enumerate().skip(next_source) {
        if !ranges.iter().any(|r| (r.from..=r.through).contains(&index)) {
            continue;
        }
        limiter.acquire().await;
        match rpc.ledger_transactions(index).await {
            Ok(ledger) => return Ok((source, ledger)),
            Err(e) => error = e.into(),
        }
    }
    Err(error)
}

async fn scan_history(
    config: &Config,
    db: &Db,
    available: &[(&RpcClient, Vec<Range>)],
    limiter: &Limiter,
    range: Range,
) -> Result<()> {
    // Fetch ahead on the shared limiter, but never commit a later result
    // across an earlier RPC or checkpoint error in this contiguous range.
    let mut results = std::pin::pin!(stream::iter(range.from..=range.through)
        .map(|index| fetch_history(
            available,
            limiter,
            index,
            0,
            Error::Invariant("no endpoint claims an available ledger"),
        ))
        .buffered(HISTORY_IN_FLIGHT));
    while let Some(result) = results.next().await {
        let (mut source, mut ledger) = result?;
        let index = ledger.header.ledger_index;
        loop {
            match store_ledger(config, db, ledger) {
                Ok(()) => break,
                Err(error @ (Error::Xrpl(_) | Error::Chain(_))) => {
                    (source, ledger) =
                        fetch_history(available, limiter, index, source + 1, error).await?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

async fn scan_scope(
    config: &Config,
    db: &Db,
    available: &[(&RpcClient, Vec<Range>)],
    scope: &str,
    limiter: &Limiter,
    end: u32,
    mut budget: u32,
) -> Result<bool> {
    let mut start = config.network.inception_ledger;
    while budget != 0 {
        let Some(missing) = db.missing(scope, start, end)? else {
            return Ok(false);
        };
        let Some(next) = available
            .iter()
            .flat_map(|(_, ranges)| ranges)
            .filter(|r| r.through >= missing)
            .map(|r| r.from.max(missing))
            .min()
            .filter(|&ledger| ledger <= end)
        else {
            return Ok(false);
        };
        if next != missing {
            start = next;
            continue;
        }
        let gap_end = db
            .next_covered(scope, missing)?
            .map_or(end, |covered| end.min(covered - 1));
        if scope.is_empty() {
            let through = available
                .iter()
                .flat_map(|(_, ranges)| ranges)
                .filter(|r| (r.from..=r.through).contains(&missing))
                .map(|r| r.through)
                .max()
                .ok_or(Error::Invariant("no endpoint claims an available ledger"))?
                .min(gap_end)
                .min(missing.saturating_add(budget - 1));
            scan_history(
                config,
                db,
                available,
                limiter,
                Range {
                    from: missing,
                    through,
                },
            )
            .await?;
            budget -= through - missing + 1;
            let Some(next) = through.checked_add(1) else {
                return Ok(false);
            };
            start = next;
            continue;
        }
        let mut progress = None;
        let mut error = Error::Invariant("no endpoint claims an available ledger");
        for (rpc, ranges) in available {
            let Some(covers) = ranges
                .iter()
                .find(|r| (r.from..=r.through).contains(&missing))
            else {
                continue;
            };
            let through = gap_end
                .min(covers.through)
                .min(missing.saturating_add(budget.min(128) - 1));
            let result = scan_account(
                rpc,
                config,
                db,
                limiter,
                scope,
                Range {
                    from: missing,
                    through,
                },
            )
            .await;
            match result {
                Ok(()) => {
                    progress = Some(through);
                    break;
                }
                Err(e @ (Error::Xrpl(_) | Error::Chain(_))) => error = e,
                Err(e) => return Err(e),
            }
        }
        let through = progress.ok_or(error)?;
        budget -= through - missing + 1;
        let Some(next) = through.checked_add(1) else {
            return Ok(false);
        };
        start = next;
    }
    Ok(true)
}

async fn pass(
    config: &Config,
    db: &Db,
    available: &[(&RpcClient, Vec<Range>)],
    limiter: &Limiter,
    end: u32,
) -> Result<bool> {
    let budget = config.node.limits.ledgers_per_pass;
    match config.node.sync_mode {
        SyncMode::Ledger => scan_scope(config, db, available, "", limiter, end, budget).await,
        SyncMode::Account => {
            let mut remaining = false;
            let mut error = None;
            for scope in db.accounts()? {
                match scan_scope(config, db, available, &scope, limiter, end, budget).await {
                    Ok(more) => remaining |= more,
                    Err(e) => error = Some(e),
                }
            }
            error.map_or(Ok(remaining), Err)
        }
    }
}

fn target(config: &Config, db: &Db) -> Result<u32> {
    let tip = db
        .tip()?
        .ok_or(Error::Invariant("XRPL has no validated tip"))?;
    Ok(config.node.end_ledger.map_or(tip, |end| end.min(tip)))
}

pub(crate) async fn catch_up(
    config: &Config,
    db: &Db,
    sources: &Sources,
    limiter: &Limiter,
) -> Result<()> {
    let available = sources.available(db, limiter).await?;
    let end = target(config, db)?;
    while pass(config, db, &available, limiter, end).await? {}
    db.set_note("sync", None)
}

pub(crate) async fn run(
    config: Arc<Config>,
    db: Arc<Db>,
    sources: Arc<Sources>,
    limiter: Arc<Limiter>,
    resync: Arc<Notify>,
    proof: Arc<Notify>,
) -> Result<()> {
    loop {
        let result = async {
            let available = sources.available(&db, &limiter).await?;
            pass(&config, &db, &available, &limiter, target(&config, &db)?).await
        }
        .await;
        proof.notify_one();
        match result {
            Ok(true) => {
                db.set_note("sync", None)?;
                continue;
            }
            Ok(false) => db.set_note("sync", None)?,
            Err(error) => {
                let reason = if matches!(error, Error::Chain(_)) {
                    "checkpoint_conflict"
                } else {
                    "history_unavailable"
                };
                tracing::warn!(
                    reason,
                    "XRPL backfill paused; coverage has not advanced across the gap"
                );
                db.set_note("sync", Some(reason))?;
            }
        }
        tokio::select! {
            () = sleep(Duration::from_secs(config.node.limits.poll_interval_secs)) => {},
            () = resync.notified() => {},
        }
    }
}

pub(crate) async fn live(
    config: Arc<Config>,
    db: Arc<Db>,
    rpc: RpcClient,
    sources: Arc<Sources>,
    limiter: Arc<Limiter>,
    resync: Arc<Notify>,
    proof: Arc<Notify>,
) -> Result<()> {
    loop {
        let mut subscription = match rpc.subscribe(config.ws_urls()).await {
            Ok(subscription) => subscription,
            Err(_) => {
                db.set_note("live", Some("disconnected"))?;
                sleep(RETRY_DELAY).await;
                continue;
            }
        };
        let mut last = None;
        loop {
            let (index, hash, count, connected) = match subscription.next_event().await {
                Ok(SubscriptionEvent::Connected {
                    ledger_index,
                    ledger_hash,
                }) => (ledger_index, ledger_hash, None, true),
                Ok(SubscriptionEvent::LedgerClosed(closed)) => (
                    closed.ledger_index,
                    closed.ledger_hash,
                    Some(closed.transaction_count),
                    false,
                ),
                Ok(SubscriptionEvent::Transaction(notice)) => {
                    if config.node.discover_accounts
                        && notice.galaxy_memo_hint
                        && notice.transaction_result == "tesSUCCESS"
                        && notice.ledger_index >= config.network.inception_ledger
                        && config
                            .node
                            .end_ledger
                            .is_none_or(|end| notice.ledger_index <= end)
                    {
                        limiter.acquire().await;
                        if let Ok(Some(tx)) = rpc.tx(&notice.hash).await {
                            if tx.ledger_index == notice.ledger_index
                                && tx.transaction_index == notice.transaction_index
                            {
                                if let Ok(Some(record)) =
                                    records::parse(&tx, config.network.network_id.as_u32(), None)
                                {
                                    if let (None, Some(account)) = (record.rejected, record.account)
                                    {
                                        db.discover(&account)?;
                                        resync.notify_one();
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                Err(_) => {
                    db.set_note("live", Some("disconnected"))?;
                    resync.notify_one();
                    last = None;
                    continue;
                }
            };
            db.increment(if connected {
                "live_connections"
            } else {
                "live_ledger_events"
            })?;
            if connected || last.is_none_or(|prev: u32| prev.checked_add(1) != Some(index)) {
                resync.notify_one();
            }
            last = Some(index);
            db.put_tip(index)?;
            if config.node.sync_mode == SyncMode::Account
                || index < config.network.inception_ledger
                || config.node.end_ledger.is_some_and(|end| index > end)
            {
                db.set_note("live", None)?;
                continue;
            }
            let result = match db.hash(index)? {
                Some(existing) if existing != hash => Err(Error::Chain(index)),
                Some(_) => Ok(()),
                None => match sources.live_ledger(index, hash, count, &limiter).await {
                    Ok(ledger) => store_ledger(&config, &db, ledger),
                    Err(error) => Err(error),
                },
            };
            if let Err(error) = result {
                let note = if matches!(error, Error::Chain(_)) {
                    "checkpoint_conflict"
                } else {
                    "history_unavailable"
                };
                db.set_note("live", Some(note))?;
                resync.notify_one();
            } else {
                db.set_note("live", None)?;
                proof.notify_one();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_internal_holes() {
        assert_eq!(
            parse_complete("300-400,10,11-20,30-41,40-70", 350).unwrap(),
            [
                Range {
                    from: 10,
                    through: 20
                },
                Range {
                    from: 30,
                    through: 70
                },
                Range {
                    from: 300,
                    through: 350
                },
            ]
        );
        assert!(parse_complete("20-10", 35).is_err());
    }
}
