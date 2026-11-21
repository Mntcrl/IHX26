//! Offline replay of recorded XRPL JSON-RPC answers (tests, parity runs,
//! `galaxy --recording`) and capture of live answers (`galaxy --record`). A
//! recording is a JSON array of `{"method": str, "params": {..}, "result": {..}}`
//! entries holding the exact parameter object galaxy-xrpl sends and the bare
//! XRPL `result`. Requests match on method and params with the `api_version`
//! key ignored; the first match wins. An unmatched request is a transport error,
//! so missing data surfaces as UNAVAILABLE, never as an absence claim.
//! AccountRoot anchors use the same format with method `account_info`; they
//! are required when an offline attempt walks the AccountTxnID chain.

use galaxy_xrpl::{
    ledger::{RpcLedger, RpcTransport},
    Error,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Mutex, PoisonError};

/// Upper bound for a recording file (bytes).
pub const MAX_RECORDING_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Exchange {
    pub method: String,
    pub params: Value,
    pub result: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub exchanges: Vec<Exchange>,
}

pub type RecordedLedgerSource = RpcLedger<Recording>;

#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("recording exceeds {MAX_RECORDING_BYTES} bytes")]
    TooLarge,
    #[error("invalid recording JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl Recording {
    pub fn from_json(bytes: &[u8]) -> Result<Self, RecordingError> {
        if bytes.len() > MAX_RECORDING_BYTES {
            return Err(RecordingError::TooLarge);
        }
        Ok(Self { exchanges: serde_json::from_slice(bytes)? })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.exchanges).expect("JSON values serialize")
    }

    /// Append an exchange unless an identical request is already recorded
    /// (replay returns the first match, so later duplicates are unreachable).
    pub fn push(&mut self, method: &str, params: Value, result: Value) {
        let params = strip(params);
        if self.find(method, &params).is_none() {
            self.exchanges.push(Exchange { method: method.into(), params, result });
        }
    }

    pub fn into_ledger(self) -> RecordedLedgerSource {
        RpcLedger::new(self)
    }

    fn find(&self, method: &str, params: &Value) -> Option<&Value> {
        let wanted = params.as_object()?;
        let without_version = |values: &serde_json::Map<String, Value>| {
            values.len() - usize::from(values.contains_key("api_version"))
        };
        self.exchanges
            .iter()
            .find(|exchange| exchange.method == method && exchange.params.as_object().is_some_and(|stored| {
                without_version(stored) == without_version(wanted)
                    && stored.iter().filter(|(key, _)| *key != "api_version")
                        .all(|(key, value)| wanted.get(key) == Some(value))
            }))
            .map(|exchange| &exchange.result)
    }
}

fn strip(mut params: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.remove("api_version");
    }
    params
}

impl RpcTransport for Recording {
    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.find(method, &params)
            .cloned()
            .ok_or_else(|| Error::Rpc(format!("no recorded answer for {method}")))
    }
}

/// Wraps a live transport and records every successful exchange for later replay.
pub struct Recorder<T> {
    inner: T,
    log: Mutex<Recording>,
}

impl<T> Recorder<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, log: Mutex::new(Recording::default()) }
    }

    pub fn recording(&self) -> Recording {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Implemented on a shared reference so the caller keeps the recorder and can
/// read [`Recorder::recording`] after the run.
impl<T: RpcTransport> RpcTransport for &Recorder<T> {
    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let result = self.inner.request(method, params.clone()).await?;
        self.log.lock().unwrap_or_else(PoisonError::into_inner).push(method, params, result.clone());
        Ok(result)
    }
}
