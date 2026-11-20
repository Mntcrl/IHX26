//! Native XRPL HTTP JSON-RPC and WebSocket transports. Both check the remote
//! NetworkID before handing requests or notifications to callers. RPC data is
//! still trusted for validation status, metadata and history coverage.

use std::{collections::VecDeque, time::Duration};

use futures_util::{SinkExt, StreamExt};
use reqwest::{redirect::Policy, Url};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::{sleep_until, timeout, Instant},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};

use crate::{
    ledger::{self, AccountTxPage, LedgerBackfill, LedgerHeader, LedgerSource, LedgerTx, RpcTransport, ServerInfo},
    AccountId, Error, Hash32,
};

const API_VERSION: u32 = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const PREFLIGHT_CAP: usize = 64 * 1024;
const HTTP_RESPONSE_CAP: usize = 8 * 1024 * 1024;
const BACKFILL_RESPONSE_CAP: usize = 32 * 1024 * 1024;
const WS_MESSAGE_CAP: usize = 4 * 1024 * 1024;
const MAX_PRE_ACK_NOTICES: usize = 256;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct RpcClient {
    http: reqwest::Client,
    urls: Vec<Url>,
    expected_network_id: u32,
}

impl RpcClient {
    /// HTTPS is required except for an explicitly configured loopback rippled
    /// over HTTP. Requests never follow redirects. Try endpoints in order, and
    /// refuse NetworkID 0 unless `allow_mainnet` is explicitly true.
    pub fn new(
        urls: impl IntoIterator<Item = impl AsRef<str>>,
        expected_network_id: u32,
        allow_mainnet: bool,
    ) -> Result<Self, Error> {
        if expected_network_id == 0 && !allow_mainnet {
            return Err(Error::Network);
        }
        let urls = parse_urls(urls, false)?;
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| Error::Rpc("could not create XRPL HTTP client".into()))?;
        Ok(Self { http, urls, expected_network_id })
    }

    /// Fetch a validated ledger and all its binary transactions in one request.
    /// Callers must check the parent hash against their stored checkpoint.
    pub async fn ledger_transactions(&self, index: u32) -> Result<LedgerBackfill, Error> {
        ledger::ledger_transactions(self, index).await
    }

    /// Connect over WSS, validate the network over that same socket, and
    /// subscribe to `ledger` and `transactions`. No notifications are returned
    /// before the subscription is acknowledged.
    pub async fn subscribe(
        &self,
        urls: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Subscription, Error> {
        let mut subscription = Subscription {
            urls: parse_urls(urls, true)?,
            expected_network_id: self.expected_network_id,
            next_endpoint: 0,
            socket: None,
            pending: VecDeque::new(),
            reconnect_at: None,
        };
        subscription.connect().await?;
        Ok(subscription)
    }

    async fn call(&self, url: &Url, method: &str, params: &Value, cap: usize) -> Result<Value, Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            method: &'a str,
            params: [&'a Value; 1],
        }
        let mut response = self.http.post(url.clone())
            .json(&Request { method, params: [params] })
            .send().await.map_err(|_| Error::Rpc("XRPL HTTP request failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Rpc(format!("XRPL HTTP status {}", response.status().as_u16())));
        }
        let hint = response.content_length().unwrap_or(0);
        if hint > cap as u64 {
            return Err(Error::Invalid("oversize XRPL HTTP response"));
        }
        let mut bytes = Vec::with_capacity(hint as usize);
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Rpc("XRPL HTTP body read failed".into()))? {
            if chunk.len() > cap - bytes.len() {
                return Err(Error::Invalid("oversize XRPL HTTP response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        ledger::take_http_result(serde_json::from_slice(&bytes)?)
    }
}

impl RpcTransport for RpcClient {
    async fn request(&self, method: &str, mut params: Value) -> Result<Value, Error> {
        params.as_object_mut()
            .ok_or(Error::Invalid("XRPL request parameters must be an object"))?
            .entry("api_version").or_insert_with(|| API_VERSION.into());
        let preflight = json!({"api_version": API_VERSION});
        let cap = if method == "ledger" && params.get("transactions").and_then(Value::as_bool) == Some(true) {
            BACKFILL_RESPONSE_CAP
        } else {
            HTTP_RESPONSE_CAP
        };
        let mut last_error = Error::Network;
        for url in &self.urls {
            // Do not reuse a previous NetworkID on a DNS, proxy, or server
            // reconfiguration. Check the target for each operation.
            let probe = self.call(url, "server_info", &preflight, PREFLIGHT_CAP).await;
            let probe = match probe.and_then(|result| {
                check_network(&result, self.expected_network_id)?;
                Ok(result)
            }) {
                Ok(result) => result,
                Err(err) => { last_error = err; continue; }
            };
            if method == "server_info" && params == preflight {
                return Ok(probe);
            }
            match self.call(url, method, &params, cap).await {
                Ok(result) => return Ok(result), // including XRPL command errors
                Err(err) if is_read(method) => last_error = err,
                // A submit may have reached the server; never retry a write
                // here. Recovery belongs to the submission state machine.
                Err(err) => return Err(err),
            }
        }
        Err(last_error)
    }
}

impl LedgerSource for RpcClient {
    async fn tx(&self, hash: &Hash32) -> Result<Option<LedgerTx>, Error> {
        ledger::tx(self, hash).await
    }

    async fn ledger_header(&self, index: u32) -> Result<LedgerHeader, Error> {
        ledger::ledger_header(self, index).await
    }

    async fn account_tx(&self, account: &AccountId, min: u32, max: u32, marker: Option<Value>, limit: u16) -> Result<AccountTxPage, Error> {
        ledger::account_tx(self, account, min, max, marker, limit).await
    }

    async fn server_info(&self) -> Result<ServerInfo, Error> {
        ledger::server_info(self).await
    }
}

fn check_network(result: &Value, expected: u32) -> Result<(), Error> {
    if ledger::server_info_fields(result)?.0 != expected {
        return Err(Error::Network);
    }
    Ok(())
}

fn is_read(method: &str) -> bool {
    matches!(method, "server_info" | "ledger" | "ledger_current" | "ledger_closed" | "tx" | "account_tx" | "account_info" | "fee")
}

fn parse_urls(urls: impl IntoIterator<Item = impl AsRef<str>>, websocket: bool) -> Result<Vec<Url>, Error> {
    let mut parsed = Vec::new();
    for input in urls {
        let url = Url::parse(input.as_ref()).map_err(|_| Error::Invalid("invalid XRPL endpoint URL"))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
        let valid_scheme = if websocket { url.scheme() == "wss" } else {
            url.scheme() == "https" || (url.scheme() == "http" && loopback)
        };
        if !valid_scheme || !url.has_host() || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(Error::Invalid("invalid XRPL endpoint URL"));
        }
        parsed.push(url);
    }
    if parsed.is_empty() {
        return Err(Error::Invalid("no XRPL endpoint URLs"));
    }
    Ok(parsed)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionEvent {
    /// First event after every successful (re)subscription. Backfill from the
    /// last durable checkpoint through this tip before trusting live continuity.
    Connected { ledger_index: u32, ledger_hash: Hash32 },
    LedgerClosed(LedgerClosed),
    /// A notification, not a verified transaction blob. Fetch by hash or
    /// backfill the ledger before indexing it.
    Transaction(TransactionNotice),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerClosed {
    pub ledger_index: u32,
    pub ledger_hash: Hash32,
    /// Unix seconds.
    pub close_time: u64,
    pub transaction_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionNotice {
    pub hash: Hash32,
    pub ledger_hash: Hash32,
    pub ledger_index: u32,
    pub transaction_index: u32,
    pub transaction_result: String,
    /// Untrusted discovery hint; verify the binary transaction before use.
    pub galaxy_memo_hint: bool,
}

pub struct Subscription {
    urls: Vec<Url>,
    expected_network_id: u32,
    next_endpoint: usize,
    socket: Option<Socket>,
    pending: VecDeque<SubscriptionEvent>,
    reconnect_at: Option<Instant>,
}

impl Subscription {
    /// An idle socket, disconnect or malformed message returns `Err`. The next
    /// call reconnects after a short delay (and rechecks NetworkID). No stream
    /// event is silently synthesized or replayed across a disconnect.
    pub async fn next_event(&mut self) -> Result<SubscriptionEvent, Error> {
        if self.socket.is_none() {
            self.connect().await?;
        }
        if let Some(event) = self.pending.pop_front() {
            return Ok(event);
        }
        let socket = self.socket.as_mut().expect("connected socket");
        let expected = self.expected_network_id;
        let event = timeout(STREAM_IDLE_TIMEOUT, async {
            loop {
                let message = read_ws(socket).await?;
                if let Some(event) = parse_notice(&message, expected)? {
                    return Ok(event);
                }
            }
        }).await.unwrap_or_else(|_| Err(Error::Rpc("XRPL WebSocket idle timeout".into())));
        if event.is_err() {
            self.socket = None;
            self.reconnect_at = Some(Instant::now() + RECONNECT_DELAY);
        }
        event
    }

    async fn connect(&mut self) -> Result<(), Error> {
        if let Some(deadline) = self.reconnect_at {
            sleep_until(deadline).await;
        }
        let mut last_error = Error::Network;
        for _ in 0..self.urls.len() {
            let endpoint = self.next_endpoint;
            self.next_endpoint = (endpoint + 1) % self.urls.len();
            match open_subscription(self.urls[endpoint].as_str(), self.expected_network_id).await {
                Ok((socket, pending)) => {
                    self.socket = Some(socket);
                    self.pending = pending;
                    self.reconnect_at = None;
                    return Ok(());
                }
                Err(err) => last_error = err,
            }
        }
        self.reconnect_at = Some(Instant::now() + RECONNECT_DELAY);
        Err(last_error)
    }
}

async fn open_subscription(url: &str, expected: u32) -> Result<(Socket, VecDeque<SubscriptionEvent>), Error> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(WS_MESSAGE_CAP))
        .max_frame_size(Some(WS_MESSAGE_CAP));
    let (mut socket, _) = timeout(CONNECT_TIMEOUT, connect_async_with_config(url, Some(config), false))
        .await.map_err(|_| Error::Rpc("XRPL WebSocket connect timeout".into()))?
        .map_err(|_| Error::Rpc("XRPL WebSocket connect failed".into()))?;
    let pending = timeout(REQUEST_TIMEOUT, subscribe_socket(&mut socket, expected))
        .await.map_err(|_| Error::Rpc("XRPL WebSocket handshake timeout".into()))??;
    Ok((socket, pending))
}

async fn subscribe_socket<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    expected: u32,
) -> Result<VecDeque<SubscriptionEvent>, Error> {
    send_ws(socket, json!({"command": "server_info", "id": 1, "api_version": API_VERSION})).await?;
    let probe = read_ws(socket).await?;
    check_network(ws_result(&probe, 1)?, expected)?;

    send_ws(socket, json!({"command": "subscribe", "id": 2, "api_version": API_VERSION,
                           "streams": ["ledger", "transactions"]})).await?;
    let mut pending = VecDeque::new();
    loop {
        let message = read_ws(socket).await?;
        if message.get("type").and_then(Value::as_str) == Some("response") {
            let result = ws_result(&message, 2)?;
            check_notice_network(result, expected)?;
            pending.push_front(SubscriptionEvent::Connected {
                ledger_index: ledger::u32_field(result, "ledger_index")?,
                ledger_hash: ledger::hash_field(result, "ledger_hash")?,
            });
            return Ok(pending);
        }
        if let Some(notice) = parse_notice(&message, expected)? {
            if pending.len() == MAX_PRE_ACK_NOTICES {
                return Err(Error::Invalid("too many XRPL notices before subscription acknowledgment"));
            }
            pending.push_back(notice);
        }
    }
}

fn ws_result(message: &Value, id: u64) -> Result<&Value, Error> {
    if message.get("id").and_then(Value::as_u64) != Some(id)
        || message.get("type").and_then(Value::as_str) != Some("response")
    {
        return Err(Error::Invalid("unexpected XRPL WebSocket response"));
    }
    ledger::check_result(message)?;
    if message.get("status").and_then(Value::as_str) != Some("success") {
        return Err(Error::Invalid("missing XRPL WebSocket success status"));
    }
    let result = &message["result"];
    ledger::check_result(result)?;
    Ok(result)
}

async fn send_ws<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    value: Value,
) -> Result<(), Error> {
    socket.send(Message::Text(serde_json::to_string(&value)?.into())).await
        .map_err(|_| Error::Rpc("XRPL WebSocket send failed".into()))
}

async fn read_ws<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> Result<Value, Error> {
    loop {
        let message = socket.next().await.ok_or_else(|| Error::Rpc("XRPL WebSocket disconnected".into()))?
            .map_err(|_| Error::Rpc("XRPL WebSocket receive failed".into()))?;
        match message {
            Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
            Message::Ping(_) => socket.flush().await.map_err(|_| Error::Rpc("XRPL WebSocket pong failed".into()))?,
            Message::Pong(_) => {},
            Message::Close(_) => return Err(Error::Rpc("XRPL WebSocket disconnected".into())),
            _ => return Err(Error::Invalid("unexpected XRPL WebSocket frame")),
        }
    }
}

fn check_notice_network(message: &Value, expected: u32) -> Result<(), Error> {
    // Older servers do not include NetworkID in notices, but the socket has
    // already passed server_info preflight.
    if message.get("network_id").is_some() && ledger::u32_field(message, "network_id")? != expected {
        return Err(Error::Network);
    }
    Ok(())
}

fn parse_notice(message: &Value, expected: u32) -> Result<Option<SubscriptionEvent>, Error> {
    check_notice_network(message, expected)?;
    match ledger::string_field(message, "type")? {
        "ledgerClosed" => Ok(Some(SubscriptionEvent::LedgerClosed(LedgerClosed {
            ledger_index: ledger::u32_field(message, "ledger_index")?,
            ledger_hash: ledger::hash_field(message, "ledger_hash")?,
            close_time: ledger::unix_time(ledger::u32_field(message, "ledger_time")?),
            transaction_count: ledger::u32_field(message, "txn_count")?,
        }))),
        "transaction" => {
            match message.get("validated") {
                Some(Value::Bool(true)) => {},
                None | Some(Value::Bool(false)) => return Ok(None),
                _ => return Err(Error::Invalid("validated")),
            }
            let meta = &message["meta"];
            let result = ledger::string_field(meta, "TransactionResult")?;
            if ledger::string_field(message, "engine_result")? != result {
                return Err(Error::Invalid("inconsistent XRPL WebSocket transaction result"));
            }
            let tx = message.get("tx_json").or_else(|| message.get("transaction"));
            let hash_source = if message.get("hash").is_some() {
                message
            } else {
                tx.ok_or(Error::Invalid("hash"))?
            };
            Ok(Some(SubscriptionEvent::Transaction(TransactionNotice {
                hash: ledger::hash_field(hash_source, "hash")?,
                ledger_hash: ledger::hash_field(message, "ledger_hash")?,
                ledger_index: ledger::u32_field(message, "ledger_index")?,
                transaction_index: ledger::u32_field(meta, "TransactionIndex")?,
                transaction_result: result.into(),
                galaxy_memo_hint: tx.is_some_and(has_galaxy_memo_hint),
            })))
        }
        "response" => Err(Error::Invalid("unsolicited XRPL WebSocket response")),
        _ => Ok(None),
    }
}

fn has_galaxy_memo_hint(tx: &Value) -> bool {
    const PREFIX: &[u8] = b"67616C6178792F76312F"; // Hex for `galaxy/v1/`.
    tx.get("Memos").and_then(Value::as_array).is_some_and(|memos| {
        memos.iter().any(|memo| {
            let Some(memo_type) = memo["Memo"]["MemoType"].as_str() else {
                return false;
            };
            let bytes = memo_type.as_bytes();
            bytes.get(..PREFIX.len()).is_some_and(|head| head.eq_ignore_ascii_case(PREFIX))
                && bytes.len() > PREFIX.len()
                && bytes.len().is_multiple_of(2)
                && bytes[PREFIX.len()..].iter().all(u8::is_ascii_hexdigit)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
        task::JoinHandle,
    };

    fn server_info(network_id: u32) -> Value {
        json!({"info": {"network_id": network_id, "complete_ledgers": "2-80",
                       "validated_ledger": {"seq": 80}}})
    }

    fn http_ok(result: Value) -> String {
        let body = json!({"result": result}).to_string();
        format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    async fn http_fixture(responses: Vec<(&'static str, String)>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for (method, response) in responses {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                let mut size = None;
                loop {
                    line.clear();
                    assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            size = Some(value.trim().parse::<usize>().unwrap());
                        }
                    }
                }
                let mut bytes = vec![0; size.unwrap()];
                stream.read_exact(&mut bytes).await.unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request["method"], method);
                assert_eq!(request["params"][0]["api_version"], 2);
                stream.get_mut().write_all(response.as_bytes()).await.unwrap();
            }
        });
        (url, task)
    }

    #[test]
    fn endpoint_protocol_and_mainnet_opt_in() {
        assert!(RpcClient::new(["https://example.org"], 0, true).is_ok());
        assert!(matches!(RpcClient::new(["https://example.org"], 0, false), Err(Error::Network)));
        assert!(RpcClient::new(["http://example.org"], 1, false).is_err());
        assert!(RpcClient::new(["https://user:password@example.org"], 1, false).is_err());
        assert!(parse_urls(["ws://127.0.0.1/"], true).is_err());
        assert!(parse_urls(["wss://example.org/"], true).is_ok());
    }

    #[tokio::test]
    async fn http_checks_each_endpoint_and_preserves_bare_rpc_errors() {
        let (wrong, wrong_task) = http_fixture(vec![("server_info", http_ok(server_info(0)))]).await;
        let missing = json!({"status": "error", "error": "txnNotFound", "searched_all": true});
        let (right, right_task) = http_fixture(vec![
            ("server_info", http_ok(server_info(1))),
            ("tx", http_ok(missing.clone())),
        ]).await;
        let client = RpcClient::new([wrong, right], 1, false).unwrap();
        assert_eq!(client.request("tx", json!({})).await.unwrap(), missing);
        wrong_task.await.unwrap();
        right_task.await.unwrap();
        assert!(check_network(&json!({"info": {"validated_ledger": {"seq": 1}}}), 0).is_err());
    }

    #[tokio::test]
    async fn http_does_not_follow_redirects_or_replay_ambiguous_writes() {
        let unused_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!("http://{}/", unused_listener.local_addr().unwrap());
        let redirect = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n");
        let (first, task) = http_fixture(vec![
            ("server_info", http_ok(server_info(1))),
            ("submit", redirect),
        ]).await;
        let (backup, backup_task) = http_fixture(vec![
            ("server_info", http_ok(server_info(1))),
            ("submit", http_ok(json!({"engine_result":"tesSUCCESS"}))),
        ]).await;
        let client = RpcClient::new([first, backup], 1, false).unwrap();
        // A redirect is an HTTP failure, not a command response. Bound the
        // test so an erroneous redirect cannot hang on the unused listener.
        let result = timeout(Duration::from_secs(2), client.request("submit", json!({}))).await.unwrap();
        assert!(matches!(result, Err(Error::Rpc(message)) if message == "XRPL HTTP status 307"));
        task.await.unwrap();
        backup_task.abort();
    }

    #[tokio::test]
    async fn http_enforces_byte_cap_with_and_without_content_length() {
        let chunks = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                      9\r\n123456789\r\n9\r\n123456789\r\n0\r\n\r\n";
        let (url, task) = http_fixture(vec![
            ("tx", http_ok(json!({"large": "already over the cap"}))),
            ("tx", chunks.into()),
        ]).await;
        let client = RpcClient::new([url], 1, false).unwrap();
        for _ in 0..2 {
            let result = client.call(&client.urls[0], "tx", &json!({"api_version": 2}), 16).await;
            assert!(matches!(result, Err(Error::Invalid("oversize XRPL HTTP response"))));
        }
        task.await.unwrap();
    }

    fn ws_reply(id: u32, result: Value) -> Value {
        json!({"id": id, "status": "success", "type": "response", "result": result})
    }

    #[tokio::test]
    async fn websocket_preflight_buffers_notices_and_reconnects_from_new_tip() {
        let wrong = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let right = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let urls = [&wrong, &right].map(|listener| {
            Url::parse(&format!("ws://{}/", listener.local_addr().unwrap())).unwrap()
        });
        let wrong_task = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = wrong.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                assert_eq!(read_ws(&mut socket).await.unwrap()["command"], "server_info");
                send_ws(&mut socket, ws_reply(1, server_info(0))).await.unwrap();
                assert!(matches!(socket.next().await, None | Some(Err(_))));
            }
        });
        let right_task = tokio::spawn(async move {
            for index in [80, 82] {
                let (stream, _) = right.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                assert_eq!(read_ws(&mut socket).await.unwrap()["command"], "server_info");
                send_ws(&mut socket, ws_reply(1, server_info(1))).await.unwrap();
                let request = read_ws(&mut socket).await.unwrap();
                assert_eq!(request["command"], "subscribe");
                assert_eq!(request["streams"], json!(["ledger", "transactions"]));
                send_ws(&mut socket, json!({
                    "type": "ledgerClosed", "network_id": 1, "ledger_index": index + 1,
                    "ledger_hash": "01".repeat(32), "ledger_time": 2000, "txn_count": 1,
                })).await.unwrap();
                send_ws(&mut socket, ws_reply(2, json!({
                    "network_id": 1, "ledger_index": index, "ledger_hash": "02".repeat(32),
                }))).await.unwrap();
                socket.close(None).await.unwrap();
            }
        });

        // Only the local test fixture bypasses URL validation; the production
        // constructor rejects all ws:// endpoints.
        let mut subscription = Subscription {
            urls: urls.into(),
            expected_network_id: 1,
            next_endpoint: 0,
            socket: None,
            pending: VecDeque::new(),
            reconnect_at: None,
        };
        for ledger_index in [80, 82] {
            assert_eq!(subscription.next_event().await.unwrap(), SubscriptionEvent::Connected {
                ledger_index, ledger_hash: [2; 32],
            });
            assert_eq!(subscription.next_event().await.unwrap(), SubscriptionEvent::LedgerClosed(
                LedgerClosed { ledger_index: ledger_index + 1, ledger_hash: [1; 32],
                               close_time: ledger::unix_time(2000), transaction_count: 1 }
            ));
            assert!(subscription.next_event().await.is_err());
        }
        wrong_task.await.unwrap();
        right_task.await.unwrap();
    }

    #[test]
    fn websocket_validated_transaction_notices_only() {
        let mut message = json!({
            "type": "transaction", "validated": true, "ledger_index": 80,
            "ledger_hash": "03".repeat(32), "hash": "04".repeat(32),
            "engine_result": "tecCLAIM", "meta": {"TransactionResult": "tecCLAIM", "TransactionIndex": 5},
        });
        assert_eq!(parse_notice(&message, 1).unwrap(), Some(SubscriptionEvent::Transaction(TransactionNotice {
            hash: [4; 32], ledger_hash: [3; 32], ledger_index: 80, transaction_index: 5,
            transaction_result: "tecCLAIM".into(),
            galaxy_memo_hint: false,
        })));
        message["engine_result"] = json!("tesSUCCESS");
        assert!(parse_notice(&message, 1).is_err());
        message["validated"] = json!(false);
        assert!(parse_notice(&message, 1).unwrap().is_none());
    }

    #[test]
    fn websocket_galaxy_memo_hint_supports_v2_and_legacy_transaction_json() {
        let mut message = json!({
            "type": "transaction", "validated": true, "ledger_index": 80,
            "ledger_hash": "03".repeat(32), "hash": "04".repeat(32),
            "engine_result": "tesSUCCESS", "meta": {"TransactionResult": "tesSUCCESS", "TransactionIndex": 5},
            "tx_json": {"Memos": [{"Memo": {"MemoType": hex::encode("galaxy/v1/capture")}}]},
        });
        let Some(SubscriptionEvent::Transaction(notice)) = parse_notice(&message, 1).unwrap() else {
            panic!("missing transaction notice");
        };
        assert!(notice.galaxy_memo_hint);

        let mut tx = message.as_object_mut().unwrap().remove("tx_json").unwrap();
        tx["hash"] = message.as_object_mut().unwrap().remove("hash").unwrap();
        message["transaction"] = tx;
        let Some(SubscriptionEvent::Transaction(notice)) = parse_notice(&message, 1).unwrap() else {
            panic!("missing legacy transaction notice");
        };
        assert!(notice.galaxy_memo_hint);
        assert_eq!(notice.hash, [4; 32]);
        for unrelated in ["galaxy/v1/capture", "67616C6178792F76312Fxyz", "67616c6178792f76312f", "67616C6178792F76322F31"] {
            message["transaction"]["Memos"][0]["Memo"]["MemoType"] = json!(unrelated);
            let Some(SubscriptionEvent::Transaction(notice)) = parse_notice(&message, 1).unwrap() else {
                panic!("missing transaction notice");
            };
            assert!(!notice.galaxy_memo_hint);
        }
    }

    #[tokio::test]
    #[ignore = "requires public XRPL Testnet access"]
    async fn testnet_ws_tip_matches_http_backfill() {
        let client = RpcClient::new(["https://s.altnet.rippletest.net:51234/"], 1, false).unwrap();
        let mut subscription = client.subscribe(["wss://s.altnet.rippletest.net:51233/"]).await.unwrap();
        let SubscriptionEvent::Connected { ledger_index, ledger_hash } = subscription.next_event().await.unwrap() else {
            panic!("missing initial validated ledger tip");
        };
        let backfill = client.ledger_transactions(ledger_index).await.unwrap();
        assert_eq!(backfill.header.hash, ledger_hash);
        assert!(backfill.header.validated);

        // Live streams: the next closed ledger and its transaction notices must
        // match an HTTP backfill of that ledger.
        let mut closed: Option<LedgerClosed> = None;
        let mut notices = Vec::new();
        for _ in 0..10_000 {
            match subscription.next_event().await.unwrap() {
                SubscriptionEvent::LedgerClosed(event) if closed.is_none() => {
                    assert!(event.ledger_index > ledger_index);
                    closed = Some(event);
                }
                SubscriptionEvent::LedgerClosed(_) => break,
                SubscriptionEvent::Transaction(notice) => notices.push(notice),
                SubscriptionEvent::Connected { .. } => panic!("unexpected reconnect during the live test"),
            }
        }
        let closed = closed.expect("no ledgerClosed event");
        let backfill = client.ledger_transactions(closed.ledger_index).await.unwrap();
        assert_eq!(backfill.header.hash, closed.ledger_hash);
        assert_eq!(backfill.header.close_time, closed.close_time);
        assert_eq!(backfill.transactions.len(), closed.transaction_count as usize);
        let in_ledger: Vec<_> = notices.iter().filter(|n| n.ledger_index == closed.ledger_index).collect();
        for notice in &in_ledger {
            assert_eq!(notice.ledger_hash, closed.ledger_hash);
            let tx = backfill.transactions.iter().find(|tx| tx.hash == notice.hash).expect("notice not in backfill");
            assert_eq!((tx.transaction_index, tx.transaction_result.as_str()),
                (notice.transaction_index, notice.transaction_result.as_str()));
        }
        println!("ledger {} txs {} notices {}", closed.ledger_index, closed.transaction_count, in_ledger.len());
    }
}
