//! JavaScript bindings. All networking happens in JavaScript transports; this module only
//! adapts their JSON strings to the shared Rust ledger and lookup traits.

use super::inspect;
use galaxy_protocol::{network::NetworkConfig, Hash32};
use galaxy_verifier::{
    verify_by_photo_id, verify_manifest, verify_photo, LedgerRange, LookupError, LookupResult,
    LookupSource, Refs, VerifyContext,
};
use galaxy_xrpl::{
    ledger::{RpcLedger, RpcTransport},
    Error as LedgerError,
};
use js_sys::{Promise, JSON};
use serde::Deserialize;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const MAX_RPC_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODE_BYTES: usize = 1024 * 1024;
const MAX_ENDPOINTS: usize = 8;

#[wasm_bindgen(typescript_custom_section)]
const API: &str = r#"
export interface ImageInspection {
    container: 'png' | 'jpeg' | null;
    profile: 'png-v1' | 'jpeg-v1' | null;
    photo_id: string | null;
    rgb_root: string | null;
    xmp: {
        protocol: string; network: string; image_profile: string;
        photo_id: string; capture_id: string; manifest_tx: string; proof_tx: string;
    } | null;
    xmp_error: string | null;
    errors: { state: 'INVALID' | 'UNSUPPORTED'; detail: string }[];
}
export type VerificationOptions = {
    network_config: string;
    rpc_endpoints: string[];
} & (
    { mode: 'direct' }
    | { mode: 'manual'; manifest_tx: string }
    | { mode: 'lookup'; nodes: string[]; node_lookup_consent: true }
);
/** Input is the XRPL command's parameter object as JSON; output is the bare XRPL `result` as JSON. */
export interface JsLedgerSource {
    tx(params: string): Promise<string>;
    ledger_header(params: string): Promise<string>;
    account_tx(params: string): Promise<string>;
    server_info(params: string): Promise<string>;
}
/** Result JSON: {candidates: [{manifest_tx, proof_tx|null}], ranges: [{min, max}], incomplete}. */
export interface JsNodeClient {
    lookup(endpoint: string, photoId: string, networkId: number): Promise<string>;
}
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "ImageInspection")]
    pub type JsInspection;
    #[wasm_bindgen(typescript_type = "VerificationOptions")]
    pub type JsOptions;
    #[wasm_bindgen(typescript_type = "JsLedgerSource")]
    pub type JsLedger;
    #[wasm_bindgen(typescript_type = "JsNodeClient")]
    pub type JsNode;

    #[wasm_bindgen(method, catch, js_name = tx)]
    fn tx_js(this: &JsLedger, params: &str) -> Result<Promise, JsValue>;
    #[wasm_bindgen(method, catch, js_name = ledger_header)]
    fn ledger_header_js(this: &JsLedger, params: &str) -> Result<Promise, JsValue>;
    #[wasm_bindgen(method, catch, js_name = account_tx)]
    fn account_tx_js(this: &JsLedger, params: &str) -> Result<Promise, JsValue>;
    #[wasm_bindgen(method, catch, js_name = server_info)]
    fn server_info_js(this: &JsLedger, params: &str) -> Result<Promise, JsValue>;
    #[wasm_bindgen(method, catch, js_name = lookup)]
    fn lookup_js(
        this: &JsNode,
        endpoint: &str,
        photo_id: &str,
        network_id: u32,
    ) -> Result<Promise, JsValue>;
}

/// Decode the image locally: container, profile, `photo_id`, `rgb_root` and untrusted XMP.
#[wasm_bindgen]
pub fn inspect_image(bytes: &[u8]) -> Result<JsInspection, JsValue> {
    let text = serde_json::to_string(&inspect(bytes)).map_err(js_error)?;
    Ok(JSON::parse(&text)?.unchecked_into())
}

#[derive(Deserialize)]
struct Options {
    network_config: String,
    rpc_endpoints: Vec<String>,
    #[serde(flatten)]
    mode: Mode,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum Mode {
    Direct,
    Manual { manifest_tx: String },
    Lookup { nodes: Vec<String>, node_lookup_consent: bool },
}

/// Run the galaxy-verifier pipeline and return the `galaxy-report/v1` JSON text.
/// Errors are configuration errors only; verification outcomes are report states.
#[wasm_bindgen]
pub async fn verify(
    bytes: Vec<u8>,
    options: JsOptions,
    ledger_source: JsLedger,
    node_client: Option<JsNode>,
) -> Result<String, JsValue> {
    let options = JSON::stringify(options.as_ref())?
        .as_string()
        .ok_or_else(|| js_error("verification options must be a JSON object"))?;
    let options: Options = serde_json::from_str(&options).map_err(js_error)?;
    if !(1..=MAX_ENDPOINTS).contains(&options.rpc_endpoints.len()) {
        return Err(js_error("configure between one and eight XRPL endpoints"));
    }
    let config = NetworkConfig::from_toml(&options.network_config).map_err(js_error)?;
    let ledger = RpcLedger::new(JsRpc(ledger_source));
    let mut ctx = VerifyContext::new(&ledger, &config);
    ctx.rpc_endpoints = options.rpc_endpoints;
    let report = match options.mode {
        Mode::Direct => verify_photo(&bytes, None, &ctx).await,
        Mode::Manual { manifest_tx } => {
            verify_manifest(Some(&bytes), hash32(&manifest_tx).map_err(js_error)?, &ctx).await
        }
        Mode::Lookup { nodes, node_lookup_consent } => {
            let client = node_client
                .as_ref()
                .filter(|_| node_lookup_consent)
                .ok_or_else(|| js_error("photo identifier lookup requires consent and a node client"))?;
            if !(1..=MAX_ENDPOINTS).contains(&nodes.len()) {
                return Err(js_error("configure between one and eight lookup nodes"));
            }
            let sources: Vec<_> = nodes.iter().map(|endpoint| JsLookup { endpoint, client }).collect();
            verify_by_photo_id(&bytes, &sources, &ctx).await
        }
    };
    serde_json::to_string(&report).map_err(js_error)
}

/// The JavaScript transport pins every endpoint to the configured network before use;
/// the pipeline checks `server_info` again.
struct JsRpc(JsLedger);

impl RpcTransport for JsRpc {
    async fn request(&self, method: &str, params: Value) -> Result<Value, LedgerError> {
        let params = serde_json::to_string(&params)?;
        let pending = match method {
            "tx" => self.0.tx_js(&params),
            "ledger" => self.0.ledger_header_js(&params),
            "account_tx" => self.0.account_tx_js(&params),
            "server_info" => self.0.server_info_js(&params),
            _ => return Err(LedgerError::Invalid("unexpected browser RPC method")),
        };
        let text = receive(pending, MAX_RPC_BYTES).await.map_err(LedgerError::Rpc)?;
        Ok(serde_json::from_str(&text)?)
    }
}

struct JsLookup<'a> {
    endpoint: &'a str,
    client: &'a JsNode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeResponse {
    candidates: Vec<NodeRef>,
    ranges: Vec<NodeRange>,
    incomplete: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeRef {
    manifest_tx: String,
    proof_tx: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeRange {
    min: u32,
    max: u32,
}

impl LookupSource for JsLookup<'_> {
    fn endpoint(&self) -> &str {
        self.endpoint
    }

    async fn lookup(&self, photo_id: &Hash32, network_id: u32) -> Result<LookupResult, LookupError> {
        let pending = self.client.lookup_js(self.endpoint, &hex::encode(photo_id), network_id);
        let text = receive(pending, MAX_NODE_BYTES).await.map_err(LookupError)?;
        let response: NodeResponse =
            serde_json::from_str(&text).map_err(|_| lookup_error("malformed node response"))?;
        let candidates = response
            .candidates
            .into_iter()
            .map(|candidate| {
                Ok(Refs {
                    manifest_tx: hash32(&candidate.manifest_tx).map_err(lookup_error)?,
                    proof_tx: candidate.proof_tx.as_deref().map(hash32).transpose().map_err(lookup_error)?,
                })
            })
            .collect::<Result<Vec<_>, LookupError>>()?;
        let ranges = response.ranges.into_iter().map(|range| LedgerRange { min: range.min, max: range.max }).collect();
        Ok(LookupResult { candidates, ranges, incomplete: response.incomplete })
    }
}

fn lookup_error(detail: &str) -> LookupError {
    LookupError(detail.into())
}

/// Await a JavaScript promise of a string, bounded in size before and after the copy
/// into linear memory. Rejections keep the JavaScript error message.
async fn receive(pending: Result<Promise, JsValue>, max_bytes: usize) -> Result<String, String> {
    let promise = pending.map_err(describe)?;
    let value = JsFuture::from(promise).await.map_err(describe)?;
    let text: js_sys::JsString = value.dyn_into().map_err(|_| "transport returned a non-string value".to_owned())?;
    // UTF-16 code units bound the UTF-8 byte length from below.
    if text.length() as usize > max_bytes {
        return Err("oversized response".into());
    }
    let text = String::from(text);
    if text.len() > max_bytes {
        return Err("oversized response".into());
    }
    Ok(text)
}

fn describe(error: JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "transport failed".into())
}

fn hash32(input: &str) -> Result<Hash32, &'static str> {
    let mut bytes = [0; 32];
    hex::decode_to_slice(input, &mut bytes).map_err(|_| "expected a 64-digit hexadecimal hash")?;
    Ok(bytes)
}

fn js_error(detail: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&detail.to_string()).into()
}
