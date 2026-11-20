CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE checkpoints(
    ledger INTEGER PRIMARY KEY,
    hash BLOB NOT NULL CHECK(length(hash) = 32),
    parent_hash BLOB NOT NULL CHECK(length(parent_hash) = 32)
);
-- scope '' is global; classic addresses are outbound-only account scans.
CREATE TABLE indexed_ranges(
    scope TEXT NOT NULL, first INTEGER NOT NULL, last INTEGER NOT NULL,
    PRIMARY KEY(scope, first), CHECK(first > 0 AND first <= last)
) WITHOUT ROWID;
CREATE TABLE accounts(address TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE records(
    hash TEXT PRIMARY KEY NOT NULL, ledger INTEGER NOT NULL, tx_index INTEGER NOT NULL,
    kind TEXT NOT NULL, rejected TEXT, account TEXT, id TEXT, reference TEXT,
    registration TEXT, bundle TEXT, counter INTEGER,
    UNIQUE(ledger, tx_index)
);
CREATE INDEX by_id ON records(kind,id,ledger,tx_index,hash) WHERE rejected IS NULL;
CREATE INDEX by_reference ON records(kind,reference,ledger,tx_index,hash) WHERE rejected IS NULL;
CREATE INDEX by_registration ON records(registration,kind,ledger,tx_index,hash) WHERE rejected IS NULL;
CREATE INDEX by_bundle ON records(bundle,kind,ledger,tx_index,hash) WHERE rejected IS NULL;
CREATE TABLE announcements(
    hash TEXT PRIMARY KEY REFERENCES records(hash) ON DELETE CASCADE,
    url TEXT NOT NULL, expires INTEGER NOT NULL, id BLOB NOT NULL CHECK(length(id) = 32)
) WITHOUT ROWID;
CREATE INDEX announcements_by_expiry ON announcements(expires);
CREATE TABLE reports(
    proof TEXT PRIMARY KEY REFERENCES records(hash) ON DELETE CASCADE,
    generation INTEGER NOT NULL DEFAULT 0,
    due INTEGER,
    checked_at INTEGER,
    report_version TEXT,
    document TEXT
) WITHOUT ROWID;
CREATE INDEX report_queue ON reports(due,proof) WHERE due IS NOT NULL;
CREATE TABLE peers(
    id TEXT PRIMARY KEY, expires INTEGER NOT NULL, document TEXT NOT NULL
) WITHOUT ROWID;
