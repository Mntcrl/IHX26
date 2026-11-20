use std::{path::Path, sync::{Mutex, MutexGuard}, time::Duration};

use galaxy_protocol::Hash32;
use galaxy_xrpl::ledger::LedgerHeader;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::Serialize;

use crate::{config::{Config, MAX_ACCOUNTS, MAX_PEERS}, records::{Record, PROOF, REVOKE}, Error, Result};

pub(crate) struct Db(Mutex<Connection>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Range { pub from: u32, pub through: u32 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cursor(pub u32, pub u32, pub String);

#[derive(Serialize)]
pub(crate) struct Reference {
    pub tx: String,
    pub ledger_index: u32,
    pub transaction_index: u32,
    pub account: Option<String>,
    pub memo_type: String,
    pub id: Option<String>,
    pub reference: Option<String>,
    pub registration_tx: Option<String>,
    pub bundle_id: Option<String>,
    pub counter_hint: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_tx: Option<String>,
}

impl Reference {
    pub fn cursor(&self) -> Cursor { Cursor(self.ledger_index, self.transaction_index, self.tx.clone()) }
}

pub(crate) enum Lookup<'a> { Photo(&'a str), Capture(&'a str), Proof(&'a str), Device(&'a str), Circuit(&'a str) }

#[derive(Serialize)]
pub(crate) struct CachedReport {
    pub proof_tx: String,
    pub manifest_tx: String,
    pub ledger_index: u32,
    pub transaction_index: u32,
    pub report_version: Option<String>,
    pub evaluated_at_unix: Option<u64>,
    pub image_binding_checked: bool,
    pub report: Option<serde_json::Value>,
}

impl CachedReport {
    pub fn cursor(&self) -> Cursor { Cursor(self.ledger_index, self.transaction_index, self.proof_tx.clone()) }
}

pub(crate) const FILE_NAME: &str = "index.sqlite3";

impl Db {
    pub fn open(config: &Config) -> Result<Self> {
        std::fs::create_dir_all(&config.node.data_dir)?;
        let db = Self::open_path(&config.node.data_dir.join(FILE_NAME))?;
        {
            let mut conn = db.lock()?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mode = match config.node.sync_mode {
                crate::config::SyncMode::Ledger => "ledger",
                crate::config::SyncMode::Account => "account",
            };
            let expected = format!("{}:{}:{}", config.network.network_id as u32, config.network.inception_ledger, mode);
            let actual: Option<String> = tx.query_row("SELECT value FROM settings WHERE key = 'scope_v1'", [], |row| row.get(0)).optional()?;
            if actual.as_ref().is_some_and(|value| value != &expected) {
                return Err(Error::Config("cache scope changed; rebuild the cache before starting"));
            }
            tx.execute("INSERT OR IGNORE INTO settings VALUES ('scope_v1', ?1)", [expected])?;
            for address in &config.node.accounts {
                tx.execute("INSERT OR IGNORE INTO accounts VALUES (?1)", [address])?;
            }
            let count: usize = tx.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))?;
            if count > MAX_ACCOUNTS { return Err(Error::Config("too many cached accounts")); }
            tx.commit()?;
        }
        Ok(db)
    }

    pub fn open_path(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match tx.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))? {
            0 => tx.execute_batch(concat!(include_str!("schema.sql"), "PRAGMA user_version = 1;"))?,
            1 => (),
            _ => return Err(Error::Config("unsupported cache schema version")),
        }
        tx.commit()?;
        Ok(Self(Mutex::new(conn)))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.0.lock().map_err(|_| Error::Invariant("cache lock is unavailable"))
    }

    pub fn clear(&self) -> Result<()> {
        self.lock()?.execute_batch("BEGIN IMMEDIATE; DELETE FROM records; DELETE FROM indexed_ranges;
            DELETE FROM checkpoints; DELETE FROM accounts; DELETE FROM peers; DELETE FROM settings; COMMIT;")?;
        Ok(())
    }

    /// Remove every cached row of `data_dir`, even when the cache scope changed.
    pub fn wipe(data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        Self::open_path(&data_dir.join(FILE_NAME))?.clear()
    }

    /// Operational notes (for example the last sync error) shown by status.
    pub fn set_note(&self, key: &str, value: Option<&str>) -> Result<()> {
        let conn = self.lock()?;
        match value {
            Some(value) => conn.execute("INSERT INTO settings VALUES ('note:'||?1, ?2)
                ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?,
            None => conn.execute("DELETE FROM settings WHERE key='note:'||?1", [key])?,
        };
        Ok(())
    }

    pub fn note(&self, key: &str) -> Result<Option<String>> {
        Ok(self.lock()?.query_row("SELECT value FROM settings WHERE key='note:'||?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn put_tip(&self, index: u32) -> Result<()> {
        self.lock()?.execute("INSERT INTO settings VALUES ('latest_validated', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE CAST(settings.value AS INTEGER) < ?1", [index])?;
        Ok(())
    }

    pub fn tip(&self) -> Result<Option<u32>> {
        Ok(self.lock()?.query_row("SELECT CAST(value AS INTEGER) FROM settings WHERE key='latest_validated'", [], |r| r.get(0)).optional()?)
    }

    pub fn accounts(&self) -> Result<Vec<String>> {
        let conn = self.lock()?;
        let mut q = conn.prepare_cached("SELECT address FROM accounts ORDER BY address")?;
        Ok(q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn discover(&self, account: &str) -> Result<()> {
        self.lock()?.execute("INSERT OR IGNORE INTO accounts SELECT ?1 WHERE (SELECT COUNT(*) FROM accounts) < ?2", params![account, MAX_ACCOUNTS])?;
        Ok(())
    }

    pub fn missing(&self, scope: &str, from: u32, through: u32) -> Result<Option<u32>> {
        if from > through { return Ok(None); }
        let conn = self.lock()?;
        let covered: Option<u32> = conn.query_row("SELECT last FROM indexed_ranges WHERE scope=?1 AND first<=?2 AND last>=?2",
            params![scope, from], |r| r.get(0)).optional()?;
        Ok(match covered { Some(end) if end >= through => None, Some(end) => Some(end + 1), None => Some(from) })
    }

    pub fn next_covered(&self, scope: &str, after: u32) -> Result<Option<u32>> {
        Ok(self.lock()?.query_row("SELECT first FROM indexed_ranges WHERE scope=?1 AND first>?2 ORDER BY first LIMIT 1",
            params![scope, after], |r| r.get(0)).optional()?)
    }

    pub fn ranges(&self, scope: &str, after: u32, limit: usize) -> Result<Vec<Range>> {
        let conn = self.lock()?;
        let mut q = conn.prepare_cached("SELECT first,last FROM indexed_ranges WHERE scope=?1 AND first>?2 ORDER BY first LIMIT ?3")?;
        Ok(q.query_map(params![scope, after, limit], |r| Ok(Range { from: r.get(0)?, through: r.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn scope_bounds(&self, scope: &str) -> Result<(Option<u32>, Option<u32>)> {
        Ok(self.lock()?.query_row("SELECT MIN(first),MAX(last) FROM indexed_ranges WHERE scope=?1", [scope], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    pub fn last_indexed(&self) -> Result<Option<u32>> {
        Ok(self.lock()?.query_row("SELECT MAX(last) FROM indexed_ranges", [], |r| r.get(0))?)
    }

    pub fn hash(&self, ledger: u32) -> Result<Option<Hash32>> {
        checkpoint(&self.lock()?, ledger, false)
    }

    pub fn insert_ledger(&self, header: &LedgerHeader, records: &[Record]) -> Result<()> {
        if !header.validated || header.ledger_index == 0 || records.iter().any(|r| r.ledger != header.ledger_index) {
            return Err(Error::Invariant("expected records from one validated ledger"));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let i = header.ledger_index;
        if checkpoint(&tx, i, false)?.is_some_and(|hash| hash != header.hash)
            || checkpoint(&tx, i, true)?.is_some_and(|hash| hash != header.parent_hash)
            || checkpoint(&tx, i - 1, false)?.is_some_and(|hash| hash != header.parent_hash)
            || i.checked_add(1).map(|next| checkpoint(&tx, next, true)).transpose()?.flatten().is_some_and(|hash| hash != header.hash)
        {
            return Err(Error::Chain(i));
        }
        tx.execute("INSERT OR IGNORE INTO checkpoints VALUES (?1, ?2, ?3)", params![i, &header.hash[..], &header.parent_hash[..]])?;
        insert_records(&tx, records)?;
        cover(&tx, "", Range { from: i, through: i })?;
        tx.commit()?;
        Ok(())
    }

    /// Partial account pages may be indexed, but only the terminal page claims coverage.
    pub fn insert_account_page(&self, scope: &str, range: Range, records: &[Record], complete: bool) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_records(&tx, records)?;
        if complete { cover(&tx, scope, range)?; }
        tx.commit()?;
        Ok(())
    }

    pub fn lookup(&self, what: Lookup<'_>, cursor: &Cursor, limit: usize) -> Result<Vec<Reference>> {
        let (filter, value) = match what {
            Lookup::Photo(value) => ("r.kind='galaxy/v1/capture' AND r.reference=?1", value),
            Lookup::Capture(value) => ("r.kind='galaxy/v1/capture' AND r.id=?1", value),
            Lookup::Proof(value) => ("r.kind='galaxy/v1/proof' AND r.reference=?1", value),
            Lookup::Device(value) => ("(r.kind='galaxy/v1/device' AND r.hash=?1) OR (r.kind='galaxy/v1/revoke' AND r.registration=?1) OR (r.kind='galaxy/v1/device-chunk' AND (r.bundle,r.account) IN (SELECT bundle,account FROM records WHERE kind='galaxy/v1/device' AND rejected IS NULL AND hash=?1))", value),
            Lookup::Circuit(value) => ("(r.kind='galaxy/v1/circuit' AND r.id=?1) OR (r.kind='galaxy/v1/circuit-chunk' AND (r.bundle,r.account) IN (SELECT bundle,account FROM records WHERE kind='galaxy/v1/circuit' AND rejected IS NULL AND id=?1))", value),
        };
        let sql = format!("SELECT r.hash,r.ledger,r.tx_index,r.account,r.kind,r.id,r.reference,r.registration,r.bundle,r.counter,
            CASE WHEN r.kind='galaxy/v1/capture' THEN (SELECT p.hash FROM records p WHERE p.kind='galaxy/v1/proof'
            AND p.rejected IS NULL AND p.reference=r.hash AND p.account=r.account ORDER BY p.ledger,p.tx_index,p.hash LIMIT 1) END
            FROM records r WHERE r.rejected IS NULL AND ({filter}) AND (r.ledger,r.tx_index,r.hash)>(?2,?3,?4)
            ORDER BY r.ledger,r.tx_index,r.hash LIMIT ?5");
        let conn = self.lock()?;
        let mut q = conn.prepare_cached(&sql)?;
        Ok(q.query_map(params![value, cursor.0, cursor.1, cursor.2, limit], |r| Ok(Reference {
            tx: r.get(0)?, ledger_index: r.get(1)?, transaction_index: r.get(2)?, account: r.get(3)?, memo_type: r.get(4)?,
            id: r.get(5)?, reference: r.get(6)?, registration_tx: r.get(7)?, bundle_id: r.get(8)?, counter_hint: r.get(9)?, proof_tx: r.get(10)?,
        }))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn reports(&self, capture_id: &str, cursor: &Cursor, limit: usize) -> Result<Vec<CachedReport>> {
        let conn = self.lock()?;
        let mut q = conn.prepare_cached("SELECT p.hash,c.hash,p.ledger,p.tx_index,j.report_version,j.checked_at,j.document
            FROM records c JOIN records p ON p.reference=c.hash JOIN reports j ON j.proof=p.hash
            WHERE c.kind='galaxy/v1/capture' AND c.rejected IS NULL AND c.id=?1 AND p.kind='galaxy/v1/proof' AND p.rejected IS NULL
            AND (p.ledger,p.tx_index,p.hash)>(?2,?3,?4) ORDER BY p.ledger,p.tx_index,p.hash LIMIT ?5")?;
        let rows = q.query_map(params![capture_id, cursor.0, cursor.1, cursor.2, limit], |r| {
            Ok((CachedReport {proof_tx:r.get(0)?, manifest_tx:r.get(1)?, ledger_index:r.get(2)?, transaction_index:r.get(3)?,
                report_version:r.get(4)?, evaluated_at_unix:r.get(5)?, image_binding_checked:false, report:None}, r.get::<_,Option<String>>(6)?))
        })?;
        rows.map(|row| { let (mut report, bytes) = row?; report.report = bytes.map(|b| serde_json::from_str(&b)).transpose()?; Ok(report) }).collect()
    }

    pub fn counts(&self) -> Result<(u64, u64, u64, u64)> {
        Ok(self.lock()?.query_row("SELECT (SELECT COUNT(*) FROM checkpoints),
            (SELECT COUNT(*) FROM records WHERE rejected IS NULL), (SELECT COUNT(*) FROM records WHERE rejected IS NOT NULL),
            (SELECT COUNT(*) FROM reports WHERE due IS NOT NULL)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
    }

    pub fn job(&self, now: u64) -> Result<Option<(String,u64)>> {
        Ok(self.lock()?.query_row("SELECT proof,generation FROM reports WHERE due<=?1 ORDER BY due,proof LIMIT 1", [now],
            |r| Ok((r.get(0)?,r.get(1)?))).optional()?)
    }

    pub fn store_report(&self, proof: &str, generation: u64, now: u64, version: &str, document: &str, next: Option<u64>) -> Result<()> {
        self.lock()?.execute("UPDATE reports SET due=?3,checked_at=?4,report_version=?5,document=?6 WHERE proof=?1 AND generation=?2",
            params![proof,generation,next,now,version,document])?;
        Ok(())
    }

    pub fn refresh_reports(&self, build: &str) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("INSERT INTO settings VALUES ('report_build',?1) ON CONFLICT (key) DO UPDATE SET value=excluded.value WHERE value<>?1", [build])?;
        if changed != 0 {
            tx.execute("UPDATE reports SET due=0,generation=generation+1,checked_at=NULL,report_version=NULL,document=NULL", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn announcements(&self, now: u64) -> Result<Vec<(String, Hash32)>> {
        let conn = self.lock()?;
        let mut q = conn.prepare_cached("SELECT url,id FROM announcements WHERE expires>?1 AND expires<=?2 ORDER BY expires DESC,hash LIMIT ?3")?;
        Ok(q.query_map(params![now, now + crate::discovery::MAX_TTL, MAX_PEERS], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn peer_documents(&self, now: u64) -> Result<Vec<String>> {
        let conn = self.lock()?;
        let mut q = conn.prepare_cached("SELECT document FROM peers WHERE expires>?1 ORDER BY id LIMIT ?2")?;
        Ok(q.query_map(params![now, MAX_PEERS], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn replace_peers(&self, peers: &[crate::discovery::Descriptor]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM peers", [])?;
        let mut insert = tx.prepare_cached("INSERT INTO peers VALUES (?1,?2,?3)")?;
        for peer in peers {
            insert.execute(params![peer.node_id(), peer.expires_at(), serde_json::to_string(peer)?])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }
}

fn checkpoint(conn: &Connection, ledger: u32, parent: bool) -> Result<Option<Hash32>> {
    let sql = if parent { "SELECT parent_hash FROM checkpoints WHERE ledger=?1" } else { "SELECT hash FROM checkpoints WHERE ledger=?1" };
    Ok(conn.query_row(sql, [ledger], |r| r.get(0)).optional()?)
}

fn cover(conn: &Connection, scope: &str, range: Range) -> Result<()> {
    let (first, last): (Option<u32>, Option<u32>) = conn.query_row("SELECT MIN(first),MAX(last) FROM indexed_ranges WHERE scope=?1 AND first<=?2 AND last>=?3",
        params![scope, i64::from(range.through)+1, range.from.saturating_sub(1)], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if let (Some(first), Some(last)) = (first, last) {
        if first <= range.from && last >= range.through {
            // Min/max can span distinct ranges with a gap; only skip when one row covers the input.
            let containing: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM indexed_ranges WHERE scope=?1 AND first<=?2 AND last>=?3)",
                params![scope,range.from,range.through], |r| r.get(0))?;
            if containing { return Ok(()); }
        }
        conn.execute("DELETE FROM indexed_ranges WHERE scope=?1 AND first BETWEEN ?2 AND ?3", params![scope,first,last])?;
    }
    conn.execute("INSERT INTO indexed_ranges VALUES (?1,?2,?3)", params![scope,first.unwrap_or(range.from).min(range.from),last.unwrap_or(range.through).max(range.through)])?;
    Ok(())
}

fn insert_records(conn: &Connection, records: &[Record]) -> Result<()> {
    let mut insert = conn.prepare_cached("INSERT INTO records VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(hash) DO NOTHING")?;
    for r in records {
        if insert.execute(params![r.hash,r.ledger,r.index,r.kind,r.rejected,r.account,r.id,r.reference,r.registration,r.bundle,r.counter])? == 0 {
            let (ledger,index) = conn.query_row("SELECT ledger,tx_index FROM records WHERE hash=?1", [&r.hash], position)?;
            if (ledger,index) != (r.ledger,r.index) { return Err(Error::Chain(r.ledger)); }
            continue;
        }
        if r.rejected.is_some() { continue; }
        if let Some((url, expires, id)) = &r.announcement {
            // XRPL stores u64 expiry. Values SQLite cannot represent are never usable advertisements.
            if let Ok(expires) = i64::try_from(*expires) {
                conn.execute("INSERT INTO announcements VALUES (?1,?2,?3,?4)", params![r.hash,url,expires,&id[..]])?;
            }
        }
        if r.kind == PROOF {
            conn.execute("INSERT INTO reports(proof,due) VALUES (?1,0)", [&r.hash])?;
        } else if r.kind == REVOKE {
            conn.execute("UPDATE reports SET due=0,generation=generation+1,checked_at=NULL,report_version=NULL,document=NULL
                WHERE proof IN (SELECT p.hash FROM records c JOIN records p ON p.reference=c.hash
                WHERE c.kind='galaxy/v1/capture' AND c.rejected IS NULL AND c.registration=?1
                AND p.kind='galaxy/v1/proof' AND p.rejected IS NULL)", [&r.registration])?;
        }
    }
    Ok(())
}

fn position(r: &Row<'_>) -> rusqlite::Result<(u32,u32)> { Ok((r.get(0)?,r.get(1)?)) }
