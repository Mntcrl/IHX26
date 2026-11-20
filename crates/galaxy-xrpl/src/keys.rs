//! XRPL Ed25519 family seeds and address derivation (never send seeds to an RPC).

use crate::{AccountId, Error, Hash32};
use ed25519_dalek::{Signer, SigningKey};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

const ED_VERSION: [u8; 3] = [0x01, 0xe1, 0x4b];

fn checksum(bytes: &[u8]) -> [u8; 4] {
    Sha256::digest(Sha256::digest(bytes))[..4].try_into().unwrap()
}

fn base58(bytes: &[u8]) -> String {
    bs58::encode(bytes)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .into_string()
}

pub fn decode_address(input: &str) -> Result<AccountId, Error> {
    let error = || Error::Invalid("invalid XRPL classic address");
    if input.len() > 35 {
        return Err(error());
    }
    let mut bytes = [0; 25];
    let n = bs58::decode(input)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .onto(&mut bytes[..])
        .map_err(|_| error())?;
    if n != bytes.len() || bytes[0] != 0 || bytes[21..] != checksum(&bytes[..21]) {
        return Err(error());
    }
    Ok(bytes[1..21].try_into().unwrap())
}

pub fn classic_address(account: &AccountId) -> String {
    let mut bytes = [0; 25];
    bytes[1..21].copy_from_slice(account);
    let sum = checksum(&bytes[..21]);
    bytes[21..].copy_from_slice(&sum);
    base58(&bytes)
}

pub fn account_id(public_key: &[u8; 33]) -> AccountId {
    Ripemd160::digest(Sha256::digest(public_key)).into()
}

pub fn tx_hash(blob: &[u8]) -> Hash32 {
    let mut hasher = Sha512::new();
    hasher.update(b"TXN\0");
    hasher.update(blob);
    hasher.finalize()[..32].try_into().unwrap()
}

pub fn decode_family_seed(input: &str) -> Result<Zeroizing<[u8; 16]>, Error> {
    let error = || Error::Invalid("invalid XRPL Ed25519 family seed");
    if input.len() > 32 || !input.starts_with("sEd") {
        return Err(error());
    }
    let mut bytes = Zeroizing::new([0; 23]);
    let n = bs58::decode(input)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .onto(&mut bytes[..])
        .map_err(|_| error())?;
    if n != bytes.len() || bytes[..3] != ED_VERSION || bytes[19..] != checksum(&bytes[..19]) {
        return Err(error());
    }
    Ok(Zeroizing::new(bytes[3..19].try_into().unwrap()))
}

pub fn encode_family_seed(entropy: &[u8; 16]) -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0; 23]);
    bytes[..3].copy_from_slice(&ED_VERSION);
    bytes[3..19].copy_from_slice(entropy);
    let sum = checksum(&bytes[..19]);
    bytes[19..].copy_from_slice(&sum);
    Zeroizing::new(base58(&bytes[..]))
}

pub struct Wallet(SigningKey);

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wallet([redacted])")
    }
}

impl Wallet {
    pub fn from_family_seed(seed: &str) -> Result<Self, Error> {
        let entropy = decode_family_seed(seed)?;
        Ok(Self::from_entropy(&entropy))
    }

    /// XRPL Ed25519 private seed = SHA-512Half(family seed entropy).
    pub fn from_entropy(entropy: &[u8; 16]) -> Self {
        let mut digest = Sha512::digest(entropy);
        let secret: Zeroizing<[u8; 32]> = Zeroizing::new(digest[..32].try_into().unwrap());
        use zeroize::Zeroize;
        digest[..].zeroize();
        Self(SigningKey::from_bytes(&secret))
    }

    pub fn public_key(&self) -> [u8; 33] {
        let mut out = [0xed; 33];
        out[1..].copy_from_slice(self.0.verifying_key().as_bytes());
        out
    }

    pub fn account_id(&self) -> AccountId {
        account_id(&self.public_key())
    }

    pub fn classic_address(&self) -> String {
        classic_address(&self.account_id())
    }

    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base58check() {
        assert_eq!(classic_address(&[0; 20]), "rrrrrrrrrrrrrrrrrrrrrhoLvTp");
        assert_eq!(decode_address("rrrrrrrrrrrrrrrrrrrrrhoLvTp").unwrap(), [0; 20]);
        assert!(decode_address("rrrrrrrrrrrrrrrrrrrrrhoLvTr").is_err());
        let entropy = [0xa5; 16];
        let encoded = encode_family_seed(&entropy);
        assert_eq!(*decode_family_seed(&encoded).unwrap(), entropy);
        assert!(decode_family_seed(&encoded[..encoded.len() - 1]).is_err());
        assert_eq!(format!("{:?}", Wallet::from_entropy(&entropy)), "Wallet([redacted])");
    }
}
