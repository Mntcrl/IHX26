use std::{net::SocketAddr, path::{Path, PathBuf}};

use galaxy_protocol::network::NetworkConfig;
use serde::Deserialize;
use url::Url;

use crate::{discovery, Error, Result};

pub(crate) const MAX_PEERS: usize = 64;
pub(crate) const MAX_ACCOUNTS: usize = 64;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SyncMode {
    #[default]
    Ledger,
    Account,
}

#[cfg(feature = "test-faults")]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Fault {
    WrongReferences,
    OmitCaptures,
    FailLookup,
}

/// Load with [`Config::load`]; relative paths resolve against the TOML file.
#[derive(Clone)]
pub struct Config {
    pub(crate) node: Settings,
    pub(crate) network: NetworkConfig,
    pub(crate) public_url: Url,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub network_config: PathBuf,
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub public_url: String,
    #[serde(default)]
    pub sync_mode: SyncMode,
    pub end_ledger: Option<u32>,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub discover_accounts: bool,
    #[serde(default)]
    pub rpc_endpoints: Vec<String>,
    #[serde(default)]
    pub websocket_endpoints: Vec<String>,
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,
    #[serde(default)]
    pub manual_peers: Vec<String>,
    #[serde(default)]
    pub allow_private_peers: bool,
    #[serde(default = "enabled")]
    pub discovery: bool,
    #[serde(default)]
    pub xrpl_announcements: bool,
    #[serde(default = "enabled")]
    pub live: bool,
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
    #[serde(default)]
    pub limits: Limits,
    #[cfg(feature = "test-faults")]
    pub test_fault: Option<Fault>,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Limits {
    pub requests_per_minute: u32,
    pub sync_requests_per_second: u32,
    pub ledgers_per_pass: u32,
    pub poll_interval_secs: u64,
    pub discovery_interval_secs: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { requests_per_minute: 180, sync_requests_per_second: 4, ledgers_per_pass: 128,
               poll_interval_secs: 5, discovery_interval_secs: 60 }
    }
}

fn enabled() -> bool { true }
fn default_log_filter() -> String { "galaxy_node=info".into() }

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut node: Settings = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| Error::Config("invalid node TOML"))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        node.data_dir = base.join(node.data_dir);
        node.network_config = base.join(node.network_config);
        let network = NetworkConfig::load(&node.network_config)
            .map_err(|err| Error::NetworkConfig(err.to_string()))?;
        let public_url = discovery::peer_url(&node.public_url, node.allow_private_peers)?;
        let lim = &node.limits;
        if network.inception_ledger == 0
            || node.end_ledger.is_some_and(|end| end < network.inception_ledger)
            || node.accounts.len() > MAX_ACCOUNTS
            || node.rpc_endpoints.len() > 8 || node.websocket_endpoints.len() > 8
            || node.bootstrap_peers.len().saturating_add(node.manual_peers.len()) > MAX_PEERS
            || !(1..=36_000).contains(&lim.requests_per_minute)
            || !(1..=50).contains(&lim.sync_requests_per_second)
            || !(1..=4096).contains(&lim.ledgers_per_pass)
            || !(1..=3600).contains(&lim.poll_interval_secs)
            || !(5..=3600).contains(&lim.discovery_interval_secs)
        {
            return Err(Error::Config("node configuration exceeds bounds"));
        }
        if node.sync_mode == SyncMode::Ledger && (!node.accounts.is_empty() || node.discover_accounts) {
            return Err(Error::Config("accounts and discover_accounts require account sync mode"));
        }
        if node.sync_mode == SyncMode::Account && node.accounts.is_empty() && !node.discover_accounts {
            return Err(Error::Config("account sync requires accounts or discover_accounts"));
        }
        for account in &node.accounts { galaxy_xrpl::keys::decode_address(account)?; }
        for url in node.bootstrap_peers.iter().chain(&node.manual_peers) {
            discovery::peer_url(url, node.allow_private_peers)?;
        }
        Ok(Self { node, network, public_url })
    }

    pub fn log_filter(&self) -> &str { &self.node.log_filter }

    pub(crate) fn rpc_urls(&self) -> Vec<&str> {
        self.network.trusted_local_rippled.iter().map(String::as_str)
            .chain(self.node.rpc_endpoints.iter().map(String::as_str))
            .chain([self.network.json_rpc.as_str()]).collect()
    }

    pub(crate) fn ws_urls(&self) -> impl Iterator<Item = &str> {
        self.node.websocket_endpoints.iter().map(String::as_str)
            .chain([self.network.websocket.as_str()])
    }
}
