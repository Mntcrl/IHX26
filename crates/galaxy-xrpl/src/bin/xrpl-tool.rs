use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand, ValueEnum};
use galaxy_protocol::network::NetworkConfig;
use galaxy_xrpl::dev_wallet::{faucet, wallet_load, wallet_new, wallet_root};
use galaxy_xrpl::keys::Wallet;
use galaxy_xrpl::ledger::{LedgerTx, RpcTransport};
use galaxy_xrpl::native::RpcClient;
use galaxy_xrpl::registry::{fetch_bundle_in, publish_bundle_in, BundleKind, Namespace, MAX_BUNDLE_BYTES};
use galaxy_xrpl::submission::{balance, classify_result, FileTxStore, PendingTx, Submitter, TxState, TxStore};
use galaxy_xrpl::transaction::{sign_transaction, AccountSet, Memo};
use galaxy_xrpl::{codec, Error, Hash32, LedgerSource};
use serde_json::{json, Value};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(about = "Self-custodied Galaxy XRPL tools")]
struct Args {
    #[arg(long, global = true, default_value = "deployments/networks/xrpl-testnet.toml")]
    config: PathBuf,
    #[arg(long, global = true, default_value = "testnet")]
    wallet: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Wallet { #[command(subcommand)] command: WalletCommand },
    Tx { #[command(subcommand)] command: TxCommand },
    /// Publish chunk txs then the manifest tx. `--namespace test` writes galaxy/test/* memo types.
    PublishBundle { kind: CliKind, file: PathBuf, #[arg(long, value_enum)] namespace: CliNamespace },
    FetchBundle {
        manifest_tx: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "v1")]
        namespace: CliNamespace,
    },
    /// Serialize the v1 envelopes. Live exercises send only galaxy/test/* on Testnet.
    Measure {
        #[arg(long, conflicts_with = "interrupt")]
        live: bool,
        /// Persist the signed blob and exit before submit. Resume with --live and the same run ID.
        #[arg(long)]
        interrupt: bool,
        #[arg(long, default_value = "phase1")]
        run_id: String,
    },
}

#[derive(Subcommand)]
enum WalletCommand { New, ShowAddress, Fund, Balance }
#[derive(Subcommand)]
enum TxCommand { Inspect { hash: String } }
#[derive(Clone, Copy, ValueEnum)]
enum CliKind { Device, Circuit }
#[derive(Clone, Copy, ValueEnum)]
enum CliNamespace { V1, Test }

impl From<CliNamespace> for Namespace {
    fn from(value: CliNamespace) -> Self {
        match value { CliNamespace::V1 => Self::V1, CliNamespace::Test => Self::Test }
    }
}

fn user_name(s: &str) -> Result<(), Error> {
    if !(1..=64).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)) {
        return Err(Error::Invalid("wallet and run names must be 1–64 letters, digits, '_' or '-'"));
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<NetworkConfig, Error> {
    NetworkConfig::load(path).map_err(|err| Error::Rpc(format!("load network configuration: {err}")))
}

fn rpc_client(config: &NetworkConfig) -> Result<RpcClient, Error> {
    let urls = config.trusted_local_rippled.iter().chain(std::iter::once(&config.json_rpc)).map(String::as_str);
    RpcClient::new(urls, config.network_id.as_u32(), config.allow_mainnet)
}

fn store(name: &str) -> Result<FileTxStore, Error> {
    user_name(name)?;
    let path = wallet_root()?.parent().unwrap().join("tx-store").join(name);
    FileTxStore::new(path)
}

fn hash(s: &str) -> Result<Hash32, Error> {
    let mut out = [0; 32];
    hex::decode_to_slice(s, &mut out).map_err(|_| Error::Invalid("expected a 32-byte hex transaction hash"))?;
    Ok(out)
}

fn show_json(value: &Value) -> Result<(), Error> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

async fn delay() { tokio::time::sleep(Duration::from_millis(900)).await; }

fn measured_memos(mut decoded: Value, blob_len: usize) -> Result<usize, Error> {
    decoded.as_object_mut().ok_or(Error::Invalid("encoded tx is not an object"))?.remove("Memos");
    Ok(blob_len - codec::encode(&decoded)?.len())
}

fn measure_offline() -> Result<(), Error> {
    let wallet = Wallet::from_entropy(&[0; 16]);
    for (label, kind, len) in [
        ("CAPTURE/max-assertion", "galaxy/v1/capture", 901),
        ("CAPTURE/450-byte-assertion", "galaxy/v1/capture", 751),
        // Real App Attest assertions observed on device: 141/142 bytes.
        ("CAPTURE/142-byte-assertion", "galaxy/v1/capture", 443),
        // Dev-unattested: 0xF0 ‖ ed25519 signature[64].
        ("CAPTURE/dev-unattested", "galaxy/v1/capture", 366),
        ("PROOF", "galaxy/v1/proof", 361),
    ] {
        let payload = vec![0; len];
        let memo = Memo { kind, data: &payload };
        let signed = AccountSet { account: wallet.account_id(), sequence: 1, fee_drops: 10,
            last_ledger_sequence: 50, network_id: 1, memo }.sign(&wallet)?;
        let memos_bytes = measured_memos(codec::decode(&signed.blob)?, signed.blob.len())?;
        show_json(&json!({"size_only":label,"blob_bytes":signed.blob.len(),"memos_bytes":memos_bytes,"memo_data_bytes":len}))?;
    }
    Ok(())
}

fn final_tx(state: TxState) -> Result<LedgerTx, Error> {
    match state {
        TxState::Validated(tx) => Ok(tx),
        TxState::Expired => Err(Error::Invalid("transaction expired")),
        TxState::Pending => Err(Error::Invalid("transaction did not finalize")),
    }
}

async fn measure_live(args: &Args, config: &NetworkConfig, run_id: &str, interrupt: bool) -> Result<(), Error> {
    if config.network_id.as_u32() != 1 { return Err(Error::Network); }
    user_name(run_id)?;
    let rpc = rpc_client(config)?;
    let wallet = wallet_load(&args.wallet)?;
    let store = store(&args.wallet)?;
    let sm = Submitter::new(&rpc, &store, 1, false, config.max_fee_drops)?;
    let purpose = format!("measure/{run_id}/interrupted");
    let memo = Memo { kind: "galaxy/test/interrupted", data: run_id.as_bytes() };
    if interrupt {
        let p = sm.prepare(&purpose, &wallet, memo, None).await?;
        return show_json(&json!({"interrupted_before_submit":hex::encode_upper(p.hash),"last_ledger":p.last_ledger}));
    }
    if let Some(saved) = store.load(&purpose)? {
        let p = sm.prepare(&purpose, &wallet, memo, None).await?;
        if p != saved { return Err(Error::Invalid("recovery failed to reuse original blob")); }
        let pending_at_restart = matches!(sm.state(&p).await?, TxState::Pending);
        let tx = final_tx(sm.run(&p, delay).await?)?;
        if tx.transaction_result != "tesSUCCESS" { return Err(Error::Invalid("recovered transaction did not succeed")); }
        show_json(&json!({"recovered":hex::encode_upper(tx.hash),"pending_at_restart":pending_at_restart,
            "validated_ledger":tx.ledger_index,"result":tx.transaction_result}))?;
    }
    for (index, len) in [1, 32, 65, 191, 192, 193, 361, 751, 839, 901].into_iter().enumerate() {
        let data = vec![index as u8; len];
        let memo = Memo { kind: "galaxy/test/measure", data: &data };
        let purpose = format!("measure/{run_id}/{index}");
        let p = sm.prepare(&purpose, &wallet, memo, None).await?;
        let started = Instant::now();
        let submit = sm.submit_saved(&p).await?;
        let duplicate = if index == 0 { Some(sm.submit_saved(&p).await?.code) } else { None };
        let tx = final_tx(sm.poll(&p, delay).await?)?;
        let latency = started.elapsed().as_millis();
        if tx.transaction_result != "tesSUCCESS" { return Err(Error::Invalid("measurement transaction failed")); }
        let tx_json = codec::decode(&tx.tx_blob)?;
        let fee = tx_json["Fee"].as_str().and_then(|s| s.parse::<u64>().ok()).ok_or(Error::Invalid("invalid measured fee"))?;
        let memos_bytes = measured_memos(tx_json, tx.tx_blob.len())?;
        show_json(&json!({"hash":hex::encode_upper(tx.hash),"ledger":tx.ledger_index,
            "blob_bytes":tx.tx_blob.len(),"memos_bytes":memos_bytes,"memo_data_bytes":len,
            "fee_drops":fee,"submit_to_validated_ms":latency,"engine_result":submit.code,"duplicate_result":duplicate}))?;
    }
    let purpose = format!("measure/{run_id}/expiry");
    let expiry_memo = Memo { kind: "galaxy/test/expiry", data: run_id.as_bytes() };
    let head = sm.validated_ledger().await?;
    let expiry = sm.prepare(&purpose, &wallet, expiry_memo, Some(head + 3)).await?;
    if !matches!(sm.poll(&expiry, delay).await?, TxState::Expired) { return Err(Error::Invalid("unsent expiry transaction unexpectedly validated")); }
    // Only the Testnet experiment intentionally bypasses the deadline guard to
    // observe the server's response to this old signed blob.
    let expired = rpc.request("submit", json!({"tx_blob":hex::encode_upper(&expiry.blob)})).await?;
    show_json(&json!({"expired_test":hex::encode_upper(expiry.hash),"last_ledger":expiry.last_ledger,
        "window_searched":true,"engine_result":expired["engine_result"]}))?;
    if expired["engine_result"] != "tefMAX_LEDGER" { return Err(Error::Invalid("expected tefMAX_LEDGER for expired blob")); }
    measure_tec(&rpc, &sm, &store, &wallet, config.max_fee_drops, run_id).await
}

async fn measure_tec(rpc: &RpcClient, sm: &Submitter<'_, RpcClient, FileTxStore>, store: &FileTxStore, wallet: &Wallet,
    cap: u64, run_id: &str) -> Result<(), Error>
{
    let purpose = format!("measure/{run_id}/tec");
    let pending = if let Some(p) = store.load(&purpose)? { p } else {
        let head = sm.validated_ledger().await?;
        let address = wallet.classic_address();
        let info = rpc.request("account_info", json!({"account":address,"ledger_index":head})).await?;
        if info["validated"] != true || info["ledger_index"] != head || info["account_data"]["Account"] != address {
            return Err(Error::Invalid("validated account unavailable for tec experiment"));
        }
        let seq = info["account_data"]["Sequence"].as_u64().ok_or(Error::Invalid("missing validated sequence"))?;
        let fee = rpc.request("fee", json!({})).await?;
        let fee = fee["drops"]["open_ledger_fee"].as_str().ok_or(Error::Invalid("missing tec experiment fee"))?;
        if !fee.parse::<u64>().ok().is_some_and(|n| n > 0 && n <= cap) { return Err(Error::Invalid("tec experiment fee too high")); }
        let mut entropy = Zeroizing::new([0; 16]);
        getrandom::fill(&mut entropy[..]).map_err(|_| Error::Invalid("operating system RNG unavailable"))?;
        let destination = Wallet::from_entropy(&entropy).classic_address();
        let memo = Memo { kind:"galaxy/test/tec", data:run_id.as_bytes() };
        let last = head + 20;
        let tx = sign_transaction(json!({"TransactionType":"Payment","Account":address,"Destination":destination,"Amount":"1",
            "Fee":fee,"Sequence":seq,"LastLedgerSequence":last,"Memos":memo.to_json()?}), wallet)?;
        let p = PendingTx { purpose, hash: tx.hash, blob: tx.blob, network_id:1, first_possible_ledger:head+1, last_ledger:last };
        if !store.compare_and_swap(None, &p)? { return Err(Error::Invalid("concurrent tec measurement")); }
        p
    };
    let state = sm.run(&pending, delay).await?;
    let success = state.succeeded();
    let tx = final_tx(state)?;
    show_json(&json!({"tec_test":hex::encode_upper(tx.hash),"validated_ledger":tx.ledger_index,"result":tx.transaction_result,
        "class":format!("{:?}",classify_result(&tx.transaction_result)),"success":success}))?;
    if tx.transaction_result != "tecNO_DST_INSUF_XRP" || success { return Err(Error::Invalid("expected a validated tecNO_DST_INSUF_XRP")); }
    Ok(())
}

async fn run(args: Args) -> Result<(), Error> {
    match &args.command {
        Command::Wallet { command: WalletCommand::New } => println!("{}", wallet_new(&args.wallet)?.classic_address()),
        Command::Wallet { command: WalletCommand::ShowAddress } => println!("{}", wallet_load(&args.wallet)?.classic_address()),
        Command::Wallet { command: WalletCommand::Fund } => {
            let cfg = read_config(&args.config)?;
            if cfg.network_id.as_u32() != 1 { return Err(Error::Network); }
            let wallet = wallet_load(&args.wallet)?;
            faucet(&wallet.classic_address()).await?;
            println!("Faucet request accepted for {}; check validated balance.", wallet.classic_address());
        }
        Command::Wallet { command: WalletCommand::Balance } => {
            let rpc = rpc_client(&read_config(&args.config)?)?;
            let drops = balance(&rpc, &wallet_load(&args.wallet)?.account_id()).await?;
            println!("{drops} drops (before reserve)");
        }
        Command::Tx { command: TxCommand::Inspect { hash: txid } } => {
            let rpc = rpc_client(&read_config(&args.config)?)?;
            let tx = rpc.tx(&hash(txid)?).await?.ok_or(Error::Invalid("transaction not found"))?;
            show_json(&json!({"hash":hex::encode_upper(tx.hash),"validated":tx.validated,"ledger_index":tx.ledger_index,
                "close_time_unix":tx.close_time,"tx":codec::decode(&tx.tx_blob)?,"meta":codec::decode(&tx.meta_blob)?}))?;
        }
        Command::PublishBundle { kind, file, namespace } => {
            let config = read_config(&args.config)?;
            let rpc = rpc_client(&config)?;
            let store = store(&args.wallet)?;
            let wallet = wallet_load(&args.wallet)?;
            let sm = Submitter::new(&rpc, &store, config.network_id.as_u32(), config.allow_mainnet, config.max_fee_drops)?;
            let mut bytes = Vec::new();
            File::open(file)?.take((MAX_BUNDLE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
            let kind = match kind { CliKind::Device => BundleKind::Device, CliKind::Circuit => BundleKind::Circuit };
            let result = publish_bundle_in(&sm, &wallet, (*namespace).into(), kind, &bytes, delay).await?;
            show_json(&json!({"manifest_tx":hex::encode_upper(result.manifest_tx),
                "chunk_txs":result.chunk_txs.into_iter().map(hex::encode_upper).collect::<Vec<_>>()}))?;
        }
        Command::FetchBundle { manifest_tx, output, namespace } => {
            let config = read_config(&args.config)?;
            let rpc = rpc_client(&config)?;
            let bundle = fetch_bundle_in(&rpc, &hash(manifest_tx)?, config.network_id.as_u32(), config.allow_mainnet,
                (*namespace).into()).await?;
            if let Some(path) = output { fs::write(path, bundle.bytes)?; }
            else { std::io::stdout().lock().write_all(&bundle.bytes)?; }
        }
        Command::Measure { live, interrupt, run_id } => {
            if !interrupt { measure_offline()?; }
            if *live || *interrupt { measure_live(&args, &read_config(&args.config)?, run_id, *interrupt).await?; }
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run(Args::parse()).await {
        eprintln!("xrpl-tool: {err}");
        std::process::exit(1);
    }
}
