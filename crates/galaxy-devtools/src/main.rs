#![cfg(unix)]

mod capture;
mod record;
mod registry;
mod storage;

use std::{path::PathBuf, time::Duration};

use clap::{Parser, Subcommand, ValueEnum};
use galaxy_image::ImageProfile;
use galaxy_protocol::{network::NetworkConfig, Hash32};
use galaxy_xrpl::{
    dev_wallet::{faucet, wallet_load, wallet_new, wallet_root},
    native::RpcClient,
    submission::{balance, FileTxStore, Submitter},
    LedgerSource,
};
use serde_json::{json, Value};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(crate) const DEVELOPMENT: &str = "development";
const NETWORK: u32 = 1;

#[derive(Parser)]
#[command(about = "XRPL Galaxy development tools — not a certification path")]
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
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    RegisterCircuit {
        #[arg(default_value = "circuits/artifacts/v1m/circuit.gxcb")]
        bundle: PathBuf,
    },
    RegisterDevDevice {
        #[arg(long, default_value = "dev-device")]
        device: String,
    },
    Capture(Box<capture::CaptureArgs>),
    Record(record::RecordArgs),
}

#[derive(Subcommand)]
enum WalletAction {
    New,
    ShowAddress,
    Fund,
    Balance,
}

#[derive(Clone, Copy, Eq, PartialEq, ValueEnum)]
pub(crate) enum Profile {
    #[value(name = "png-v1")]
    Png,
    #[value(name = "jpeg-v1")]
    Jpeg,
}

impl From<Profile> for ImageProfile {
    fn from(profile: Profile) -> Self {
        match profile {
            Profile::Png => Self::PngV1,
            Profile::Jpeg => Self::JpegV1,
        }
    }
}

fn error(message: &str) -> Box<dyn std::error::Error> {
    std::io::Error::other(message).into()
}

fn hash(s: &str) -> std::result::Result<Hash32, &'static str> {
    let mut value = [0; 32];
    hex::decode_to_slice(s, &mut value).map_err(|_| "expected exactly 32 bytes in hex")?;
    Ok(value)
}

fn output(fields: Value) {
    let mut fields = fields.as_object().expect("command output object").clone();
    fields.insert("mode".into(), DEVELOPMENT.into());
    println!("{}", Value::Object(fields));
}

async fn delay() {
    tokio::time::sleep(Duration::from_secs(1)).await;
}

async fn ledger(rpc: &RpcClient, index: u32) -> Result<galaxy_xrpl::ledger::LedgerHeader> {
    let header = rpc.ledger_header(index).await?;
    if !header.validated || header.ledger_index != index {
        return Err(error("RPC did not return the requested validated ledger"));
    }
    Ok(header)
}

fn load_network(path: &std::path::Path) -> Result<NetworkConfig> {
    let config = NetworkConfig::load(path)?;
    if config.network_id.as_u32() != NETWORK {
        return Err(error("galaxy-dev is restricted to XRPL Testnet (network ID 1)"));
    }
    Ok(config)
}

fn rpc(config: &NetworkConfig) -> Result<RpcClient> {
    let endpoints = config.trusted_local_rippled.iter().chain(std::iter::once(&config.json_rpc));
    Ok(RpcClient::new(endpoints, NETWORK, false)?)
}

async fn run(args: Args) -> Result<()> {
    let name = &args.wallet;
    match args.command {
        Command::Wallet { action: WalletAction::New } => {
            output(json!({"address": wallet_new(name)?.classic_address()}));
        }
        Command::Wallet { action: WalletAction::ShowAddress } => {
            output(json!({"address": wallet_load(name)?.classic_address()}));
        }
        Command::Wallet { action: WalletAction::Fund } => {
            load_network(&args.config)?;
            let address = wallet_load(name)?.classic_address();
            faucet(&address).await?;
            output(json!({"address": address, "faucet_requested": true, "note": "Check the validated balance before submitting transactions."}));
        }
        Command::Wallet { action: WalletAction::Balance } => {
            let config = load_network(&args.config)?;
            let wallet = wallet_load(name)?;
            let drops = balance(&rpc(&config)?, &wallet.account_id()).await?;
            output(json!({"address": wallet.classic_address(), "balance_drops_before_reserve": drops}));
        }
        Command::Record(record) => {
            let config = load_network(&args.config)?;
            record::run(&config, record).await?;
        }
        command => {
            let config = load_network(&args.config)?;
            let rpc = rpc(&config)?;
            let wallet = wallet_load(name)?; // also validates the wallet name
            let wallet_root = wallet_root()?;
            let root = wallet_root.parent().ok_or_else(|| error("invalid wallet base directory"))?;
            let journal = storage::PrivateDir::open(&root.join("development").join(name))?;
            let _wallet_lock = journal.lock()?;
            let store = FileTxStore::new(root.join("tx-store").join(name))?;
            let submitter = Submitter::new(&rpc, &store, NETWORK, false, config.max_fee_drops)?;
            let session = Session { config: &config, rpc: &rpc, submitter, wallet: &wallet, journal: &journal, wallet_root: &wallet_root };
            match command {
                Command::RegisterCircuit { bundle } => registry::circuit(&session, &bundle).await?,
                Command::RegisterDevDevice { device } => registry::device(&session, &device).await?,
                Command::Capture(args) => capture::run(&session, *args).await?,
                _ => unreachable!("wallet and recorder handled separately"),
            }
        }
    }
    Ok(())
}

pub(crate) struct Session<'a> {
    config: &'a NetworkConfig,
    rpc: &'a RpcClient,
    submitter: Submitter<'a, RpcClient, FileTxStore>,
    wallet: &'a galaxy_xrpl::keys::Wallet,
    journal: &'a storage::PrivateDir,
    wallet_root: &'a std::path::Path,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run(Args::parse()).await {
        eprintln!("galaxy-dev (development): {err}");
        std::process::exit(1);
    }
}
