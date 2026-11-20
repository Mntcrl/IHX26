//! Local CLI Testnet wallets. Mobile callers should keep seeds in Keychain.

use crate::keys::{decode_address, encode_family_seed, Wallet};
use crate::Error;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

pub fn wallet_root() -> Result<PathBuf, Error> {
    let home = std::env::var_os("HOME").ok_or(Error::Invalid("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config/galaxy/wallets"))
}

fn wallet_path(name: &str) -> Result<PathBuf, Error> {
    if !(1..=64).contains(&name.len()) || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)) {
        return Err(Error::Invalid("wallet name must be 1–64 letters, digits, '_' or '-'"));
    }
    Ok(wallet_root()?.join(format!("{name}.json")))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WalletFile<'a> { seed: &'a str }

pub fn wallet_new(name: &str) -> Result<Wallet, Error> {
    let path = wallet_path(name)?;
    let dir = path.parent().unwrap();
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let mut entropy = Zeroizing::new([0; 16]);
    getrandom::fill(&mut entropy[..]).map_err(|_| Error::Invalid("operating system RNG unavailable"))?;
    let seed = encode_family_seed(&entropy);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    serde_json::to_writer(&mut file, &WalletFile { seed: &seed })?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(dir)?.sync_all()?;
    Ok(Wallet::from_entropy(&entropy))
}

pub fn wallet_load(name: &str) -> Result<Wallet, Error> {
    let file = File::open(wallet_path(name)?)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(Error::Invalid("wallet file must be a regular file with mode 0600"));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(257).read_to_end(&mut bytes)?;
    if bytes.len() > 256 { return Err(Error::Invalid("wallet file is oversized")); }
    // A JSON error can carry input fragments. Return a redacted error instead.
    let record: WalletFile<'_> = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("invalid wallet file"))?;
    Wallet::from_family_seed(record.seed)
}

/// Request funds for a Testnet address. Success is HTTP acceptance, not a
/// validated payment. Retrieve a validated balance separately.
pub async fn faucet(address: &str) -> Result<(), Error> {
    decode_address(address)?;
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20)).build().map_err(|_| Error::Rpc("could not build faucet HTTPS client".into()))?;
    // Some faucet versions include secrets in the reply; never print or store the body.
    let response = client.post("https://faucet.altnet.rippletest.net/accounts").json(&json!({"destination":address}))
        .send().await.map_err(|_| Error::Rpc("Testnet faucet transport failure".into()))?;
    if !response.status().is_success() {
        return Err(Error::Rpc(format!("Testnet faucet HTTP {}", response.status().as_u16())));
    }
    Ok(())
}
