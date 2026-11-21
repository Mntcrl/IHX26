use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{
        rejection::{PathRejection, QueryRejection},
        ConnectInfo, Path, Query, Request, State as Extract,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use crate::{
    client::MAX_BODY,
    config::SyncMode,
    db::{Cursor, Db, Lookup, Range},
    discovery::Identity,
    unix_now, Config, Error, Result, API_VERSION, PROTOCOL_VERSION, SOFTWARE_VERSION,
};

const MAX_RANGES: usize = 2048;
const ACCOUNT_RANGES: usize = 32;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const MAX_CLIENTS: usize = 4096;
const MAX_CONCURRENT_REQUESTS: usize = 64;
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct State {
    config: Arc<Config>,
    db: Arc<Db>,
    identity: Arc<Identity>,
    rate: Arc<Mutex<Rate>>,
    capacity: Arc<Semaphore>,
}

impl State {
    pub fn new(config: Arc<Config>, db: Arc<Db>, identity: Arc<Identity>) -> Self {
        Self {
            config,
            db,
            identity,
            rate: Arc::new(Mutex::new(Rate::default())),
            capacity: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        }
    }
}

#[derive(Default)]
pub(crate) struct Rate {
    clients: HashMap<IpAddr, ClientBudget>,
    recency: BTreeSet<(u64, IpAddr)>,
    sequence: u64,
    global: Option<Budget>,
}

struct ClientBudget {
    budget: Budget,
    last_seen: u64,
}

struct Budget {
    since: Instant,
    used: u32,
}

impl Budget {
    fn new(now: Instant) -> Self {
        Self { since: now, used: 0 }
    }

    fn available(&mut self, now: Instant, limit: u32) -> bool {
        if now.duration_since(self.since) >= WINDOW {
            *self = Self::new(now);
        }
        self.used < limit
    }
}

impl Rate {
    pub(crate) fn allow(&mut self, ip: IpAddr, now: Instant, client: u32, global: u32) -> bool {
        let global_budget = self.global.get_or_insert_with(|| Budget::new(now));
        if !global_budget.available(now, global) {
            return false;
        }
        let ip = match ip.to_canonical() {
            IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(v6.to_bits() & (u128::MAX << 64))),
            v4 => v4,
        };
        self.sequence += 1;
        let entry = self.clients.entry(ip).or_insert_with(|| ClientBudget {
            budget: Budget::new(now),
            last_seen: 0,
        });
        let previous = std::mem::replace(&mut entry.last_seen, self.sequence);
        self.recency.insert((entry.last_seen, ip));
        if previous != 0 {
            self.recency.remove(&(previous, ip));
        }
        let allowed = entry.budget.available(now, client);
        if allowed {
            entry.budget.used += 1;
            global_budget.used += 1;
        }
        if self.clients.len() > MAX_CLIENTS {
            let (_, oldest) = self.recency.pop_first().expect("client recency is populated");
            self.clients.remove(&oldest);
        }
        allowed
    }
}

fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted_proxies: &[IpAddr]) -> IpAddr {
    let peer = peer.to_canonical();
    if trusted_proxies.iter().any(|proxy| proxy.to_canonical() == peer) {
        let forwarded = headers.get_all("x-forwarded-for");
        let mut values = forwarded.iter();
        if let Some(value) = values.next() {
            if values.next().is_none() {
                if let Some(ip) = value.to_str().ok().and_then(|s| s.trim().parse().ok()) {
                    return ip;
                }
            }
        }
    }
    peer
}

pub(crate) fn router(state: State) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/node", get(descriptor))
        .route("/.well-known/galaxy-node.json", get(descriptor))
        .route("/v1/peers", get(peers))
        .route("/v1/{resource}/{key}", get(resource))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_policy,
        ))
        .with_state(state)
}

async fn request_policy(Extract(state): Extract<State>, request: Request, next: Next) -> Response {
    let allowed = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|addr| {
            let node = &state.config.node;
            let client = client_ip(addr.0.ip(), request.headers(), &node.trusted_proxies);
            state.rate.lock().is_ok_and(|mut rate| {
                rate.allow(
                    client,
                    Instant::now(),
                    node.limits.requests_per_minute,
                    node.limits.global_requests_per_minute,
                )
            })
        });
    let mut response = if allowed {
        next.run(request).await
    } else {
        let mut result = error(
            &state.config,
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "request limit reached",
        );
        result
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        result
    };
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Serialize)]
struct Coverage<'a> {
    protocol: &'static str,
    protocol_version: &'static str,
    software_version: &'static str,
    network: &'a str,
    network_id: u32,
    indexed_from: Option<u32>,
    indexed_through: Option<u32>,
    indexed_ranges: Vec<Range>,
    gaps: Vec<Range>,
    latest_validated_ledger: Option<u32>,
    incomplete: bool,
}

impl<'a> Coverage<'a> {
    fn empty(config: &'a Config) -> Self {
        Self {
            protocol: API_VERSION,
            protocol_version: PROTOCOL_VERSION,
            software_version: SOFTWARE_VERSION,
            network: &config.network.name,
            network_id: config.network.network_id.as_u32(),
            indexed_from: None,
            indexed_through: None,
            indexed_ranges: Vec::new(),
            gaps: Vec::new(),
            latest_validated_ledger: None,
            incomplete: true,
        }
    }

    fn load(config: &'a Config, db: &Db) -> Result<Self> {
        let mut out = Self::empty(config);
        // Read tip first; concurrent indexing can only improve coverage.
        let tip = db.tip()?;
        let ranges = db.ranges("", 0, MAX_RANGES + 1)?;
        if ranges.len() > MAX_RANGES {
            return Err(Error::Invariant("coverage exceeds response limit"));
        }
        out.indexed_from = ranges.first().map(|r| r.from);
        out.indexed_through = ranges.last().map(|r| r.through);
        out.latest_validated_ledger = tip.max(out.indexed_through);
        if let Some(tip) = out.latest_validated_ledger {
            out.gaps = gaps(&ranges, config.network.inception_ledger, tip);
            out.incomplete = config.node.sync_mode == SyncMode::Account
                || ranges.is_empty()
                || !out.gaps.is_empty();
        }
        out.indexed_ranges = ranges;
        Ok(out)
    }
}

fn gaps(ranges: &[Range], from: u32, through: u32) -> Vec<Range> {
    let mut start = from;
    let mut out = Vec::new();
    for range in ranges {
        if range.through < start {
            continue;
        }
        if range.from > through {
            break;
        }
        if start < range.from {
            out.push(Range {
                from: start,
                through: range.from - 1,
            });
        }
        if range.through >= through {
            return out;
        }
        start = range.through + 1;
    }
    if start <= through {
        out.push(Range {
            from: start,
            through,
        });
    }
    out
}

fn envelope(coverage: Coverage<'_>, extra: Value) -> Result<Value> {
    let mut value = serde_json::to_value(coverage)?;
    let Some(map) = value.as_object_mut() else {
        return Err(Error::Invariant("invalid response envelope"));
    };
    let Value::Object(extra) = extra else {
        return Err(Error::Invariant("invalid response data"));
    };
    map.extend(extra);
    Ok(value)
}

pub(crate) fn status_document(config: &Config, db: &Db) -> Result<Value> {
    let (checkpoints, records, rejected, queued) = db.counts()?;
    let mut accounts = Vec::new();
    for account in db.accounts()? {
        let mut ranges = db.ranges(&account, 0, ACCOUNT_RANGES + 1)?;
        let ranges_truncated = ranges.len() > ACCOUNT_RANGES;
        ranges.truncate(ACCOUNT_RANGES);
        accounts.push(json!({ "account": account, "indexed_ranges": ranges, "ranges_truncated": ranges_truncated }));
    }
    envelope(
        Coverage::load(config, db)?,
        json!({
            "sync_mode": match config.node.sync_mode { SyncMode::Ledger => "ledger", SyncMode::Account => "account" },
            "inception_ledger": config.network.inception_ledger, "configured_end_ledger": config.node.end_ledger,
            "latest_synchronized_ledger": db.last_indexed()?, "account_coverage": accounts,
            "checkpoints": checkpoints, "records": records, "rejected": rejected, "queued_reports": queued,
            "sync_issue": db.note("sync")?, "live_issue": db.note("live")?,
            "live_connections": db.counter("live_connections")?, "live_ledger_events": db.counter("live_ledger_events")?,
        }),
    )
}

pub(crate) fn peers_document(config: &Config, db: &Db) -> Result<Value> {
    let mut document = envelope(
        Coverage::load(config, db)?,
        json!({"peers": [], "peers_truncated": false}),
    )?;
    let mut length = serde_json::to_vec(&document)?.len();
    let peers = document["peers"]
        .as_array_mut()
        .ok_or(Error::Invariant("invalid peers array"))?;
    for json in db.peer_documents(unix_now() as i64)? {
        // The database stores each descriptor as compact JSON.
        length += json.len() + usize::from(!peers.is_empty());
        if length > MAX_BODY {
            document["peers_truncated"] = true.into();
            break;
        }
        peers.push(serde_json::from_str::<Value>(&json)?);
    }
    Ok(document)
}

async fn health(Extract(state): Extract<State>) -> Response {
    respond(state, |state| {
        Ok((
            StatusCode::OK,
            envelope(
                Coverage::load(&state.config, &state.db)?,
                json!({"status":"ok"}),
            )?,
        ))
    })
    .await
}

async fn status(Extract(state): Extract<State>) -> Response {
    respond(state, |state| {
        Ok((StatusCode::OK, status_document(&state.config, &state.db)?))
    })
    .await
}

async fn peers(Extract(state): Extract<State>) -> Response {
    respond(state, |state| {
        Ok((StatusCode::OK, peers_document(&state.config, &state.db)?))
    })
    .await
}

async fn descriptor(Extract(state): Extract<State>) -> Response {
    respond(state, |state| {
        Ok((
            StatusCode::OK,
            serde_json::to_value(state.identity.descriptor(&state.config, &state.db)?)?,
        ))
    })
    .await
}

async fn not_found(Extract(state): Extract<State>) -> Response {
    error(
        &state.config,
        StatusCode::NOT_FOUND,
        "unknown_route",
        "unknown node API route",
    )
}

async fn method_not_allowed(Extract(state): Extract<State>) -> Response {
    let mut response = error(
        &state.config,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "this endpoint supports GET and HEAD",
    );
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    network_id: Option<u32>,
    cursor: Option<String>,
    limit: Option<usize>,
}

fn encode_cursor(cursor: Cursor) -> String {
    format!("v1.{:08x}.{:08x}.{}", cursor.0, cursor.1, cursor.2)
}

fn parse_cursor(text: Option<&str>) -> Option<Cursor> {
    let Some(text) = text else {
        return Some(Cursor(0, 0, String::new()));
    };
    let mut parts = text.split('.');
    if parts.next()? != "v1" {
        return None;
    }
    let (ledger, index, tx) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    if ledger.len() != 8
        || index.len() != 8
        || !ledger
            .bytes()
            .chain(index.bytes())
            .all(|b| b.is_ascii_hexdigit())
        || !valid_hash(tx)
        || tx.bytes().any(|b| b.is_ascii_lowercase())
    {
        return None;
    }
    let ledger = u32::from_str_radix(ledger, 16).ok()?;
    if ledger == 0 {
        return None;
    }
    Some(Cursor(
        ledger,
        u32::from_str_radix(index, 16).ok()?,
        tx.into(),
    ))
}

fn valid_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Resource {
    Photos,
    Captures,
    Proofs,
    Credentials,
    Circuits,
    Reports,
}

impl Resource {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "photos" => Self::Photos,
            "captures" => Self::Captures,
            "proofs" => Self::Proofs,
            "credentials" => Self::Credentials,
            "circuits" => Self::Circuits,
            "reports" => Self::Reports,
            _ => return None,
        })
    }

    fn key_name(self) -> &'static str {
        match self {
            Self::Photos => "photo_id",
            Self::Captures | Self::Reports => "capture_id",
            Self::Proofs => "manifest_tx",
            Self::Credentials => "registration_tx",
            Self::Circuits => "circuit_id",
        }
    }

    fn array_name(self) -> &'static str {
        match self {
            Self::Photos | Self::Captures => "captures",
            Self::Proofs => "proofs",
            Self::Credentials => "credentials",
            Self::Circuits => "circuits",
            Self::Reports => "reports",
        }
    }
}

async fn resource(
    Extract(state): Extract<State>,
    path: std::result::Result<Path<(String, String)>, PathRejection>,
    query: std::result::Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let fail = |code, message| error(&state.config, StatusCode::BAD_REQUEST, code, message);
    let Ok(Path((resource, key))) = path else {
        return fail("invalid_path", "invalid lookup path");
    };
    let Some(resource) = Resource::parse(&resource) else {
        return not_found(Extract(state)).await;
    };
    if !valid_hash(&key) {
        return fail("invalid_id", "expected exactly 64 hexadecimal digits");
    }
    let key = if matches!(resource, Resource::Proofs | Resource::Credentials) {
        key.to_ascii_uppercase()
    } else {
        key.to_ascii_lowercase()
    };
    let Ok(Query(query)) = query else {
        return fail("invalid_query", "invalid lookup query");
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return fail("invalid_limit", "limit must be between 1 and 100");
    }
    let Some(cursor) = parse_cursor(query.cursor.as_deref()) else {
        return fail("invalid_cursor", "invalid pagination cursor");
    };
    if query
        .network_id
        .is_some_and(|id| id != state.config.network.network_id.as_u32())
    {
        return fail("wrong_network", "node indexes a different XRPL network");
    }

    #[cfg(feature = "test-faults")]
    if resource == Resource::Photos
        && state.config.node.test_fault == Some(crate::config::Fault::FailLookup)
    {
        return respond(state, |state| Ok((StatusCode::SERVICE_UNAVAILABLE, envelope(Coverage::load(&state.config, &state.db)?,
            json!({"error":{"code":"lookup_unavailable","message":"lookup temporarily unavailable"}}))?))).await;
    }

    respond(state, move |state| {
        Ok((
            StatusCode::OK,
            lookup_document(&state, resource, &key, cursor, limit)?,
        ))
    })
    .await
}

fn lookup_document(
    state: &State,
    resource: Resource,
    key: &str,
    cursor: Cursor,
    limit: usize,
) -> Result<Value> {
    let coverage = Coverage::load(&state.config, &state.db)?;
    let (rows, more) = if resource == Resource::Reports {
        // Reports are larger than reference records. Byte-limit after this small
        // SQL page and retain the continuation if any row did not fit.
        let limit = limit.min(3);
        let mut rows = state.db.reports(key, &cursor, limit + 1)?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        (
            rows.into_iter()
                .map(|r| Ok((r.cursor(), serde_json::to_value(r)?)))
                .collect::<Result<Vec<_>>>()?,
            more,
        )
    } else {
        let lookup = match resource {
            Resource::Photos => Lookup::Photo(key),
            Resource::Captures => Lookup::Capture(key),
            Resource::Proofs => Lookup::Proof(key),
            Resource::Credentials => Lookup::Device(key),
            Resource::Circuits => Lookup::Circuit(key),
            Resource::Reports => unreachable!(),
        };
        let mut rows = state.db.lookup(lookup, &cursor, limit + 1)?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        (rows.into_iter().map(|r| {
            let cursor = r.cursor();
            let row = if matches!(resource, Resource::Photos | Resource::Captures) {
                json!({"capture_id": r.id, "photo_id": r.reference, "manifest_tx": r.tx, "ledger_index": r.ledger_index,
                    "transaction_index": r.transaction_index, "registration_tx": r.registration_tx, "proof_tx": r.proof_tx,
                    "assertion_counter_hint": r.counter_hint})
            } else { serde_json::to_value(r)? };
            Ok((cursor, row))
        }).collect::<Result<Vec<_>>>()?, more)
    };

    #[cfg(feature = "test-faults")]
    let (rows, more) = if resource == Resource::Photos {
        use crate::config::Fault;
        match state.config.node.test_fault {
            Some(Fault::OmitCaptures) => (Vec::new(), false),
            Some(Fault::WrongReferences) => (
                rows.into_iter()
                    .map(|(cursor, mut row)| {
                        row["manifest_tx"] = hex::encode_upper([255u8; 32]).into();
                        row["proof_tx"] = hex::encode_upper([254u8; 32]).into();
                        (cursor, row)
                    })
                    .collect(),
                more,
            ),
            _ => (rows, more),
        }
    } else {
        (rows, more)
    };

    let mut document = envelope(
        coverage,
        json!({ (resource.key_name()):key, (resource.array_name()):[], "next_cursor":null }),
    )?;
    let mut rows = rows;
    let mut more = more;
    loop {
        document["next_cursor"] = if more {
            Value::String(encode_cursor(
                rows.last()
                    .ok_or(Error::Invariant("empty page cannot continue"))?
                    .0
                    .clone(),
            ))
        } else {
            Value::Null
        };
        document[resource.array_name()] =
            Value::Array(rows.iter_mut().map(|(_, value)| value.take()).collect());
        if serde_json::to_vec(&document)?.len() <= MAX_BODY {
            return Ok(document);
        }
        let Some(items) = document[resource.array_name()].as_array_mut() else {
            return Err(Error::Invariant("missing page array"));
        };
        for (slot, value) in rows.iter_mut().zip(items) {
            slot.1 = value.take();
        }
        if rows.pop().is_none() {
            return Err(Error::Invariant("lookup exceeds response limit"));
        }
        more = true;
    }
}

fn json_response(status: StatusCode, bytes: Vec<u8>) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

fn error(config: &Config, status: StatusCode, code: &str, message: &str) -> Response {
    let body = json!({"protocol":API_VERSION, "protocol_version":PROTOCOL_VERSION, "software_version":SOFTWARE_VERSION,
        "network":config.network.name, "network_id":config.network.network_id.as_u32(), "indexed_from":null,
        "indexed_through":null, "indexed_ranges":[], "gaps":[], "latest_validated_ledger":null, "incomplete":true,
        "error":{"code":code,"message":message}});
    json_response(status, body.to_string().into_bytes())
}

async fn respond(
    state: State,
    work: impl FnOnce(State) -> Result<(StatusCode, Value)> + Send + 'static,
) -> Response {
    let config = state.config.clone();
    let Ok(permit) = state.capacity.clone().try_acquire_owned() else {
        return error(
            &config,
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded",
            "too many concurrent requests",
        );
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (status, document) = work(state)?;
        Ok::<_, Error>((status, serde_json::to_vec(&document)?))
    })
    .await
    {
        Ok(Ok((status, bytes))) if bytes.len() <= MAX_BODY => json_response(status, bytes),
        _ => error(
            &config,
            StatusCode::SERVICE_UNAVAILABLE,
            "cache_unavailable",
            "node cache response unavailable",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip_and_bounds() {
        let cursor = Cursor(u32::MAX, 7, hex::encode_upper([0xabu8; 32]));
        assert_eq!(
            parse_cursor(Some(&encode_cursor(cursor.clone()))),
            Some(cursor)
        );
        assert_eq!(parse_cursor(None), Some(Cursor(0, 0, String::new())));
        assert_eq!(parse_cursor(Some("v1.00000000.00000000.invalid")), None);
    }

    #[test]
    fn openapi_documents_every_route() {
        let spec = include_str!("../openapi.yaml");
        for route in [
            "/health",
            "/v1/status",
            "/v1/node",
            "/v1/peers",
            "/.well-known/galaxy-node.json",
            "/v1/captures/{capture_id}",
            "/v1/photos/{photo_id}",
            "/v1/proofs/{manifest_tx}",
            "/v1/credentials/{registration_tx}",
            "/v1/circuits/{circuit_id}",
            "/v1/reports/{capture_id}",
        ] {
            assert!(spec.contains(&format!("  {route}:")), "{route}");
        }
    }
}
