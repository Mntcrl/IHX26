//! `galaxy` command-line tool: verification and protocol utilities on top of
//! galaxy-verifier (spec §13). Exit codes: 0 VERIFIED, 10 PENDING, 20 INVALID,
//! 30 INCONCLUSIVE, 40 NOT_FOUND, 50 UNAVAILABLE, 60 UNSUPPORTED, 2 usage,
//! 1 internal error.

use clap::{Args, Parser, Subcommand};
use galaxy_image::ImageError;
use galaxy_protocol::{network::NetworkConfig, policy::Policy, Hash32};
use galaxy_verifier::{
    inspect_circuit, parse_lookup_response, photo_identity,
    recorded::{Recorder, Recording},
    verify_by_photo_id, verify_photo, verify_proof_only, verify_proof_transaction, CircuitInfo, LookupError,
    LookupResult, LookupSource, Refs, Report, RpcTrust, State, Status, TrustReport, VerifyContext,
    DEFAULT_LOOKUP_PAGE_SIZE, MAX_LOOKUP_NODES, MAX_LOOKUP_PAGES, MAX_LOOKUP_RESPONSE_BYTES,
};
use galaxy_xrpl::{ledger::RpcLedger, native::RpcClient, LedgerSource};
use serde_json::{json, Value};
use std::{future::Future, path::PathBuf, process::ExitCode, time::Duration};
use url::Url;

/// Built-in network configuration (the repository's xrpl-testnet.toml).
const DEFAULT_CONFIG: &str = include_str!("../../../deployments/networks/xrpl-testnet.toml");
const NODE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Parser)]
#[command(name = "galaxy", version, about = "XRPL Galaxy verifier and protocol tools")]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Global {
    /// Network configuration TOML (trusted circuits, app IDs, environments).
    /// Default: the built-in xrpl-testnet configuration.
    #[arg(long, global = true, value_name = "PATH")]
    network_config: Option<PathBuf>,
    /// XRPL JSON-RPC endpoint, repeatable, tried in order. Default: the config's
    /// trusted_local_rippled (if set), then its json_rpc.
    #[arg(long = "rpc", global = true, value_name = "URL")]
    rpc: Vec<String>,
    /// Replay recorded XRPL answers from FILE instead of contacting XRPL.
    #[arg(long, global = true, value_name = "FILE", conflicts_with_all = ["rpc", "record"])]
    recording: Option<PathBuf>,
    /// Save every XRPL answer of this run to FILE (for offline replay with --recording).
    #[arg(long, global = true, value_name = "FILE")]
    record: Option<PathBuf>,
    /// Print machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
    /// Time limit for network work, in seconds.
    #[arg(long, global = true, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
}

#[derive(Subcommand)]
enum Command {
    /// Verify a photo (direct XMP references, manual references or photo-ID lookup).
    Verify(VerifyArgs),
    /// Ask lookup nodes which CAPTURE transactions they index for a photo ID (untrusted hints, nothing is verified).
    Lookup {
        #[arg(value_parser = parse_hash, value_name = "PHOTO_ID")]
        photo_id: Hash32,
        /// Lookup node base URL, repeatable (also GALAXY_NODES, comma-separated).
        #[arg(long = "node", value_name = "URL", env = "GALAXY_NODES", value_delimiter = ',')]
        nodes: Vec<String>,
    },
    /// Verify a CAPTURE and its PROOF without an image (image binding not checked).
    Inspect {
        #[arg(value_parser = parse_hash, value_name = "MANIFEST_TX")]
        manifest_tx: Hash32,
    },
    /// PROOF transaction commands.
    Proof {
        #[command(subcommand)]
        command: ProofCommand,
    },
    /// Circuit commands.
    Circuit {
        #[command(subcommand)]
        command: CircuitCommand,
    },
    /// Policy commands.
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },
    /// Photo commands (local, no network).
    Photo {
        #[command(subcommand)]
        command: PhotoCommand,
    },
    /// Verifier node commands (galaxy-node library).
    #[cfg(feature = "node")]
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
}

#[derive(Args)]
struct VerifyArgs {
    /// PNG or JPEG file.
    image: PathBuf,
    /// CAPTURE transaction hash (manual mode); overrides references in the image.
    #[arg(long, value_parser = parse_hash, value_name = "HASH")]
    manifest_tx: Option<Hash32>,
    /// PROOF transaction hash hint for the given CAPTURE (evaluated and listed, never trusted).
    #[arg(long, value_parser = parse_hash, value_name = "HASH", requires = "manifest_tx")]
    proof_tx: Option<Hash32>,
    /// Use photo-ID lookup through the given nodes even if the image carries references.
    #[arg(long, conflicts_with = "manifest_tx")]
    lookup: bool,
    /// Lookup node base URL, repeatable (also GALAXY_NODES, comma-separated).
    #[arg(long = "node", value_name = "URL", env = "GALAXY_NODES", value_delimiter = ',')]
    nodes: Vec<String>,
    /// Never contact lookup nodes: the photo ID does not leave this machine.
    #[arg(long, conflicts_with = "lookup")]
    no_discovery: bool,
}

#[derive(Subcommand)]
enum ProofCommand {
    /// Verify one exact PROOF transaction and its CAPTURE (no image).
    Verify {
        #[arg(value_parser = parse_hash, value_name = "PROOF_TX")]
        proof_tx: Hash32,
    },
}

#[derive(Subcommand)]
enum CircuitCommand {
    /// Show an accepted circuit: descriptor, policy and digests, fetched and checked against the config.
    Inspect {
        #[arg(value_parser = parse_hash, value_name = "CIRCUIT_ID")]
        circuit_id: Hash32,
    },
}

#[derive(Subcommand)]
enum PolicyCommand {
    /// Show a policy from a JSON file (local) or by policy ID (via its accepted circuit).
    Inspect {
        #[arg(value_name = "POLICY_ID|FILE")]
        policy: String,
    },
}

#[derive(Subcommand)]
enum PhotoCommand {
    /// Print photo_id and rgb_root of an image (local, no network).
    Id { image: PathBuf },
}

#[cfg(feature = "node")]
#[derive(Subcommand)]
enum NodeCommand {
    /// Run a verifier node until interrupted.
    Run {
        #[arg(long, value_name = "PATH")]
        config: PathBuf,
    },
    /// Backfill to the validated tip once, verify queued proofs, print the status.
    Sync {
        #[arg(long, value_name = "PATH")]
        config: PathBuf,
    },
    /// Status of a local node cache (--config) or of a remote node (--url).
    Status {
        #[arg(long, value_name = "PATH", required_unless_present = "url", conflicts_with = "url")]
        config: Option<PathBuf>,
        #[arg(long, value_name = "URL")]
        url: Option<String>,
    },
    /// Known peers of a local node cache (--config) or of a remote node (--url).
    Peers {
        #[arg(long, value_name = "PATH", required_unless_present = "url", conflicts_with = "url")]
        config: Option<PathBuf>,
        #[arg(long, value_name = "URL")]
        url: Option<String>,
    },
}

enum Failure {
    Usage(String),
    Internal(String),
}

type Outcome = Result<u8, Failure>;

fn internal(err: impl std::fmt::Display) -> Failure {
    Failure::Internal(err.to_string())
}

fn parse_hash(text: &str) -> Result<Hash32, String> {
    let mut hash = [0u8; 32];
    hex::decode_to_slice(text, &mut hash).map_err(|_| "expected 64 hexadecimal characters".to_owned())?;
    Ok(hash)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start the async runtime: {err}");
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code),
        Err(Failure::Usage(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
        Err(Failure::Internal(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Outcome {
    let Cli { global, command } = cli;
    match command {
        Command::Photo { command: PhotoCommand::Id { image } } => return photo_id(&read_image(&image)?, global.json),
        Command::Policy { command: PolicyCommand::Inspect { policy } } if std::path::Path::new(&policy).is_file() => {
            return policy_file(&PathBuf::from(policy), global.json);
        }
        Command::Lookup { photo_id, nodes } => return lookup(&photo_id, &nodes, &load_config(&global)?, global.json).await,
        #[cfg(feature = "node")]
        Command::Node { command } => return node(command).await,
        _ => {}
    }
    let config = load_config(&global)?;
    if let Some(path) = &global.recording {
        let bytes = std::fs::read(path).map_err(|err| internal(format!("cannot read {}: {err}", path.display())))?;
        let ledger = Recording::from_json(&bytes).map_err(internal)?.into_ledger();
        let endpoints = vec![format!("recording:{}", path.display())];
        return execute(command, &global, &config, &ledger, endpoints, RpcTrust::Remote).await;
    }
    let endpoints = if global.rpc.is_empty() {
        config.trusted_local_rippled.iter().chain([&config.json_rpc]).cloned().collect()
    } else {
        global.rpc.clone()
    };
    let trust = if endpoints.iter().all(|url| is_local(url, &config)) { RpcTrust::Local } else { RpcTrust::Remote };
    let client = RpcClient::new(&endpoints, config.network_id.as_u32(), config.allow_mainnet)
        .map_err(|err| Failure::Usage(format!("XRPL endpoint rejected: {err}")))?;
    let recorder = Recorder::new(client);
    let ledger = RpcLedger::new(&recorder);
    let outcome = execute(command, &global, &config, &ledger, endpoints, trust).await;
    if let Some(path) = &global.record {
        std::fs::write(path, recorder.recording().to_json())
            .map_err(|err| internal(format!("cannot write {}: {err}", path.display())))?;
    }
    outcome
}

fn load_config(global: &Global) -> Result<NetworkConfig, Failure> {
    match &global.network_config {
        Some(path) => NetworkConfig::load(path).map_err(|err| internal(format!("network config {}: {err}", path.display()))),
        None => NetworkConfig::from_toml(DEFAULT_CONFIG).map_err(|err| internal(format!("built-in network config: {err}"))),
    }
}

/// A loopback endpoint or the operator's configured local rippled.
fn is_local(endpoint: &str, config: &NetworkConfig) -> bool {
    config.trusted_local_rippled.as_deref() == Some(endpoint)
        || Url::parse(endpoint).is_ok_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")))
}

fn read_image(path: &PathBuf) -> Result<Vec<u8>, Failure> {
    let size = std::fs::metadata(path).map_err(|err| internal(format!("cannot read {}: {err}", path.display())))?.len();
    if size > galaxy_image::MAX_IMAGE_BYTES as u64 {
        return Err(Failure::Usage(format!("{} exceeds the {} byte image limit", path.display(), galaxy_image::MAX_IMAGE_BYTES)));
    }
    std::fs::read(path).map_err(|err| internal(format!("cannot read {}: {err}", path.display())))
}

async fn execute<L: LedgerSource>(
    command: Command, global: &Global, config: &NetworkConfig, ledger: &L, endpoints: Vec<String>, trust: RpcTrust,
) -> Outcome {
    let mut ctx = VerifyContext::new(ledger, config);
    ctx.rpc_endpoints = endpoints;
    ctx.rpc_trust = trust;
    let limit = Duration::from_secs(global.timeout);
    let report = match command {
        Command::Verify(args) => {
            let image = read_image(&args.image)?;
            let nodes = if args.no_discovery { Vec::new() } else { http_nodes(&args.nodes)? };
            if args.lookup && nodes.is_empty() {
                return Err(Failure::Usage("--lookup needs at least one --node URL (or GALAXY_NODES)".into()));
            }
            bounded(&ctx, "verify", limit, async {
                if let Some(manifest_tx) = args.manifest_tx {
                    return verify_photo(&image, Some(Refs { manifest_tx, proof_tx: args.proof_tx }), &ctx).await;
                }
                if args.lookup {
                    return verify_by_photo_id(&image, &nodes, &ctx).await;
                }
                let report = verify_photo(&image, None, &ctx).await;
                // No references in the image: discover captures through the configured nodes.
                let unreferenced = report.state == State::NotFound
                    && report.check("refs.metadata").is_some_and(|check| check.status == Status::NotApplicable);
                if unreferenced && !nodes.is_empty() { verify_by_photo_id(&image, &nodes, &ctx).await } else { report }
            })
            .await
        }
        Command::Inspect { manifest_tx } => bounded(&ctx, "proof-only", limit, verify_proof_only(manifest_tx, &ctx)).await,
        Command::Proof { command: ProofCommand::Verify { proof_tx } } => {
            bounded(&ctx, "proof-only", limit, verify_proof_transaction(proof_tx, &ctx)).await
        }
        Command::Circuit { command: CircuitCommand::Inspect { circuit_id } } => {
            return match timed(limit, inspect_circuit(circuit_id, &ctx)).await {
                Some(Ok(info)) => print_circuit(&info, None, global.json),
                Some(Err(report)) => Ok(emit(&report, global.json)),
                None => Ok(emit(&timeout_report(&ctx, "circuit-inspect", limit), global.json)),
            };
        }
        Command::Policy { command: PolicyCommand::Inspect { policy } } => {
            let policy_id = parse_hash(&policy).map_err(|_| {
                Failure::Usage(format!("{policy} is neither a readable policy file nor a 64-hex policy ID"))
            })?;
            let Some(entry) = config.accepted_circuits.iter().find(|entry| entry.policy_id == policy_id) else {
                eprintln!("UNSUPPORTED: policy {policy} is not bound to any accepted circuit of {}", config.name);
                return Ok(State::Unsupported.exit_code());
            };
            return match timed(limit, inspect_circuit(entry.circuit_id, &ctx)).await {
                Some(Ok(info)) => print_circuit(&info, Some(&policy_id), global.json),
                Some(Err(report)) => Ok(emit(&report, global.json)),
                None => Ok(emit(&timeout_report(&ctx, "policy-inspect", limit), global.json)),
            };
        }
        Command::Photo { .. } | Command::Lookup { .. } => unreachable!("handled without a ledger"),
        #[cfg(feature = "node")]
        Command::Node { .. } => unreachable!("handled without a ledger"),
    };
    Ok(emit(&report, global.json))
}

async fn timed<T>(limit: Duration, work: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(limit, work).await.ok()
}

fn timeout_report<L: LedgerSource>(ctx: &VerifyContext<'_, L>, mode: &'static str, limit: Duration) -> Report {
    let trust = TrustReport { rpc_endpoints: ctx.rpc_endpoints.clone(), rpc_trust: ctx.rpc_trust, ..TrustReport::default() };
    Report::unavailable(ctx.config.name.clone(), mode, trust, format!(
        "Time limit of {} s reached before verification completed; no state was established.", limit.as_secs(),
    ))
}

async fn bounded<L: LedgerSource>(
    ctx: &VerifyContext<'_, L>, mode: &'static str, limit: Duration, work: impl Future<Output = Report>,
) -> Report {
    match timed(limit, work).await {
        Some(report) => report,
        None => timeout_report(ctx, mode, limit),
    }
}

fn emit(report: &Report, json: bool) -> u8 {
    if json {
        println!("{}", serde_json::to_string_pretty(report).expect("reports serialize"));
    } else {
        print!("{report}");
    }
    report.exit_code()
}

fn print_value(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).expect("JSON values serialize"));
}

fn print_circuit(info: &CircuitInfo, policy_id: Option<&Hash32>, json: bool) -> Outcome {
    if json {
        let value = match policy_id {
            Some(id) => json!({"policy_id": hex::encode(id), "circuit_id": info.circuit_id, "source": info.source, "policy": info.policy}),
            None => serde_json::to_value(info).map_err(internal)?,
        };
        print_value(&value);
    } else {
        println!("Circuit: {} ({})", info.circuit_id, info.descriptor["name"].as_str().unwrap_or("unnamed"));
        println!("Policy: {} ({})", info.policy.name, info.policy_id);
        println!("VK digest: {}", info.vk_digest);
        println!("Registration: {} ({})", info.registration_tx, info.source);
        println!("Ceremony: {}", info.descriptor["ceremony"].as_str().unwrap_or("unknown"));
        println!("Windows: w_cap {}, sel_offset {}, w_proof {}", info.policy.windows.w_cap, info.policy.windows.sel_offset, info.policy.windows.w_proof);
        if policy_id.is_some() {
            print_value(&serde_json::to_value(&info.policy).map_err(internal)?);
        }
    }
    Ok(0)
}

fn policy_file(path: &PathBuf, json: bool) -> Outcome {
    let policy = Policy::load(path).map_err(|err| internal(format!("policy {}: {err}", path.display())))?;
    let id = policy.id().map_err(|err| internal(format!("policy {}: {err}", path.display())))?;
    if json {
        print_value(&json!({"policy_id": hex::encode(id), "source": "file", "policy": policy}));
    } else {
        println!("Policy: {} ({})", policy.name, hex::encode(id));
        println!("Windows: w_cap {}, sel_offset {}, w_proof {}", policy.windows.w_cap, policy.windows.sel_offset, policy.windows.w_proof);
        print_value(&serde_json::to_value(&policy).map_err(internal)?);
    }
    Ok(0)
}

fn photo_id(image: &[u8], json: bool) -> Outcome {
    match photo_identity(image) {
        Ok(identity) => {
            let profile = serde_json::to_value(identity.profile).map_err(internal)?;
            if json {
                print_value(&json!({
                    "photo_id": hex::encode(identity.photo_id), "rgb_root": hex::encode(identity.rgb_root), "container_profile": profile,
                }));
            } else {
                println!("photo_id: {}\nrgb_root: {}\ncontainer profile: {}", hex::encode(identity.photo_id),
                    hex::encode(identity.rgb_root), profile.as_str().unwrap_or("unknown"));
            }
            Ok(0)
        }
        Err(err) => {
            eprintln!("{}: {err}", if matches!(err, ImageError::Unsupported(_)) { "UNSUPPORTED" } else { "INVALID" });
            Ok(match err {
                ImageError::Unsupported(_) => State::Unsupported.exit_code(),
                ImageError::Invalid(_) => State::Invalid.exit_code(),
            })
        }
    }
}

/// A lookup node reached over HTTPS (HTTP only on loopback); responses are untrusted hints.
struct HttpNode {
    base: Url,
    client: reqwest::Client,
}

fn http_nodes(urls: &[String]) -> Result<Vec<HttpNode>, Failure> {
    if urls.len() > MAX_LOOKUP_NODES {
        return Err(Failure::Usage(format!("at most {MAX_LOOKUP_NODES} lookup nodes")));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(NODE_TIMEOUT)
        .timeout(NODE_TIMEOUT)
        .build()
        .map_err(internal)?;
    urls.iter()
        .map(|text| {
            let mut base = Url::parse(text).map_err(|_| Failure::Usage(format!("invalid node URL {text}")))?;
            let loopback = matches!(base.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
            if !(base.scheme() == "https" || base.scheme() == "http" && loopback)
                || !base.username().is_empty() || base.password().is_some() || base.query().is_some() || base.fragment().is_some()
            {
                return Err(Failure::Usage(format!("node URL {text} must be https (http only on loopback), without credentials or query")));
            }
            if !base.path().ends_with('/') {
                base.set_path(&format!("{}/", base.path()));
            }
            Ok(HttpNode { base, client: client.clone() })
        })
        .collect()
}

impl LookupSource for HttpNode {
    fn endpoint(&self) -> &str {
        self.base.as_str()
    }

    async fn lookup(&self, photo_id: &Hash32, network_id: u32) -> Result<LookupResult, LookupError> {
        let fail = |detail: &str| LookupError(detail.to_owned());
        let mut base = self.base.join(&format!("v1/photos/{}", hex::encode(photo_id))).map_err(|_| fail("invalid node URL"))?;
        base.query_pairs_mut().append_pair("network_id", &network_id.to_string());
        let mut result = LookupResult::default();
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        for index in 0..MAX_LOOKUP_PAGES {
            let mut url = base.clone();
            if let Some(cursor) = &cursor { url.query_pairs_mut().append_pair("cursor", cursor); }
            let page = async {
                let mut response = self.client.get(url).send().await.map_err(|_| fail("node request failed"))?;
                if response.status() != reqwest::StatusCode::OK {
                    return Err(LookupError(format!("node answered HTTP {}", response.status().as_u16())));
                }
                if response.content_length().is_some_and(|n| n > MAX_LOOKUP_RESPONSE_BYTES as u64) {
                    return Err(fail("oversized node response"));
                }
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| fail("node response interrupted"))? {
                    if chunk.len() > MAX_LOOKUP_RESPONSE_BYTES - body.len() { return Err(fail("oversized node response")); }
                    body.extend_from_slice(&chunk);
                }
                parse_lookup_response(&body, photo_id, network_id, DEFAULT_LOOKUP_PAGE_SIZE)
            }.await;
            let page = match page {
                Ok(page) => page,
                Err(error) if index == 0 => return Err(error),
                Err(_) => break,
            };
            result.candidates.extend(page.result.candidates);
            result.ranges = page.result.ranges;
            result.incomplete |= page.result.incomplete;
            let Some(next) = page.next_cursor else { return Ok(result); };
            if !seen.insert(next.clone()) { break; }
            cursor = Some(next);
        }
        result.incomplete = true;
        Ok(result)
    }
}

async fn lookup(photo_id: &Hash32, urls: &[String], config: &NetworkConfig, json: bool) -> Outcome {
    let nodes = http_nodes(urls)?;
    if nodes.is_empty() {
        return Err(Failure::Usage("lookup needs at least one --node URL (or GALAXY_NODES)".into()));
    }
    let mut answered = false;
    let mut results = Vec::new();
    for node in &nodes {
        let entry = match node.lookup(photo_id, config.network_id.as_u32()).await {
            Ok(result) => {
                answered = true;
                json!({
                    "endpoint": node.endpoint(), "incomplete": result.incomplete,
                    "candidates": result.candidates.iter().map(|refs| json!({
                        "manifest_tx": hex::encode_upper(refs.manifest_tx), "proof_tx": refs.proof_tx.map(hex::encode_upper),
                    })).collect::<Vec<_>>(),
                    "ranges": result.ranges,
                })
            }
            Err(err) => json!({"endpoint": node.endpoint(), "error": err.0}),
        };
        results.push(entry);
    }
    let note = "Untrusted hints: nothing here is verified. Run `galaxy verify <image>` to verify a candidate from XRPL.";
    if json {
        print_value(&json!({"photo_id": hex::encode(photo_id), "network": config.name, "note": note, "nodes": results}));
    } else {
        println!("Photo ID: {}\n{note}", hex::encode(photo_id));
        for entry in &results {
            println!("Node {}:", entry["endpoint"].as_str().unwrap_or(""));
            if let Some(error) = entry["error"].as_str() {
                println!("  unavailable: {error}");
                continue;
            }
            for candidate in entry["candidates"].as_array().into_iter().flatten() {
                println!("  CAPTURE {} PROOF {}", candidate["manifest_tx"].as_str().unwrap_or(""),
                    candidate["proof_tx"].as_str().unwrap_or("–"));
            }
            for range in entry["ranges"].as_array().into_iter().flatten() {
                println!("  indexed ledgers {}..{}", range["min"], range["max"]);
            }
            if entry["incomplete"] == true {
                println!("  index reports gaps");
            }
        }
    }
    Ok(if answered { 0 } else { State::Unavailable.exit_code() })
}

#[cfg(feature = "node")]
async fn node(command: NodeCommand) -> Outcome {
    let load = |path: &PathBuf| galaxy_node::Config::load(path).map_err(internal);
    let value = match command {
        NodeCommand::Run { config } => {
            galaxy_node::run(load(&config)?).await.map_err(internal)?;
            return Ok(0);
        }
        NodeCommand::Sync { config } => galaxy_node::sync_once(load(&config)?).await,
        NodeCommand::Status { url: Some(url), .. } => galaxy_node::status(&url).await,
        NodeCommand::Status { config: Some(config), .. } => galaxy_node::local_status(load(&config)?),
        NodeCommand::Peers { url: Some(url), .. } => galaxy_node::peers(&url).await,
        NodeCommand::Peers { config: Some(config), .. } => galaxy_node::local_peers(load(&config)?),
        NodeCommand::Status { .. } | NodeCommand::Peers { .. } => {
            return Err(Failure::Usage("give --config or --url".into()));
        }
    }
    .map_err(internal)?;
    print_value(&value);
    Ok(0)
}
