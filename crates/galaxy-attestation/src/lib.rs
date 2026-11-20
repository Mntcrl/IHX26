//! Galaxy's Apple App Attest verifier and separately labelled development credential.

mod apple;
mod certificate;

use galaxy_protocol::ids::{
    assertion_client_data_hash, dev_assertion_message, dev_key_id, registration_client_data_hash,
};
pub use galaxy_protocol::wire::AttestationEnv;
use galaxy_protocol::wire::{CaptureCore, DeviceBundle, DeviceKind, DeviceManifest};
use galaxy_protocol::{AccountId, Hash32};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use apple::{assertion_counter_hint, strip_receipt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AttestationClass {
    AppleDevelopment,
    AppleProduction,
    /// Valid dev signatures establish key possession, never `VERIFIED` status.
    DevUnattested,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum CredentialPublicKey {
    AppleP256(p256::ecdsa::VerifyingKey),
    DevEd25519(ed25519_dalek::VerifyingKey),
}

/// Policy for Apple credentials. An empty allowlist accepts no Apple credential.
/// The separately named dev class is recognized independently for `UNSUPPORTED` reporting.
#[derive(Clone, Debug, Default)]
pub struct AcceptedAttestation {
    pub app_ids: Vec<String>,
    pub environments: Vec<AttestationEnv>,
}

/// Registration result, constructed only by [`verify_registration`].
#[derive(Clone, Debug)]
pub struct VerifiedCredential {
    key_id: Hash32,
    public_key: CredentialPublicKey,
    env: AttestationEnv,
    class: AttestationClass,
    app_id: String,
    account: AccountId,
    network: u32,
}

impl VerifiedCredential {
    pub fn key_id(&self) -> &Hash32 {
        &self.key_id
    }

    pub fn public_key(&self) -> &CredentialPublicKey {
        &self.public_key
    }

    pub fn env(&self) -> AttestationEnv {
        self.env
    }

    pub fn class(&self) -> AttestationClass {
        self.class
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn account(&self) -> &AccountId {
        &self.account
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssertionInfo {
    /// Apple’s authenticated sign counter; `None` for dev-unattested assertions.
    pub counter: Option<u32>,
}

/// An authenticated App Attest assertion's ledger position and counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CounterRecord {
    pub ledger_index: u32,
    pub tx_index: u32,
    pub counter: u32,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum AttestError {
    #[error("attestation or assertion exceeds the wire size limit")]
    Size,
    #[error("malformed or unexpected CBOR")]
    Cbor,
    #[error("unexpected attestation format")]
    AttestationFormat,
    #[error("DEVICE manifest and bundle bindings differ")]
    ManifestBinding,
    #[error("invalid capture core")]
    CaptureCore,
    #[error("capture account or network does not match the credential")]
    CaptureBinding,
    #[error("Apple trust anchor does not match the compiled fingerprint")]
    RootPin,
    #[error("invalid X.509 certificate encoding")]
    CertificateEncoding,
    #[error("certificate is not valid at the DEVICE manifest ledger close time")]
    CertificateTime,
    #[error("certificate does not match the Apple X.509 profile")]
    CertificateProfile,
    #[error("certificate chain does not lead to the pinned Apple root")]
    CertificateChain,
    #[error("missing or invalid Apple nonce extension")]
    NonceEncoding,
    #[error("attestation nonce does not match the registration challenge")]
    NonceMismatch,
    #[error("app identifier is not accepted or does not match signed data")]
    AppId,
    #[error("attestation environment is not accepted or does not match signed data")]
    Environment,
    #[error("key identifier does not match the attested credential")]
    KeyId,
    #[error("invalid or inconsistent public key")]
    PublicKey,
    #[error("invalid authenticator data length")]
    AuthenticatorData,
    #[error("invalid App Attest authenticator flags")]
    Flags,
    #[error("invalid Apple authenticator extensions")]
    Extensions,
    #[error("invalid sign counter for the operation")]
    Counter,
    #[error("assertion replays a ledger position or does not advance the counter")]
    CounterReplay,
    #[error("unexpected assertion signature encoding")]
    SignatureEncoding,
    #[error("assertion signature is invalid")]
    Signature,
}

impl AttestError {
    /// Stable machine-readable code for report checks and test vectors.
    pub fn code(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Cbor => "cbor",
            Self::AttestationFormat => "attestation_format",
            Self::ManifestBinding => "manifest_binding",
            Self::CaptureCore => "capture_core",
            Self::CaptureBinding => "capture_binding",
            Self::RootPin => "root_pin",
            Self::CertificateEncoding => "certificate_encoding",
            Self::CertificateTime => "certificate_time",
            Self::CertificateProfile => "certificate_profile",
            Self::CertificateChain => "certificate_chain",
            Self::NonceEncoding => "nonce_encoding",
            Self::NonceMismatch => "nonce_mismatch",
            Self::AppId => "app_id",
            Self::Environment => "environment",
            Self::KeyId => "key_id",
            Self::PublicKey => "public_key",
            Self::AuthenticatorData => "authenticator_data",
            Self::Flags => "flags",
            Self::Extensions => "extensions",
            Self::Counter => "counter",
            Self::CounterReplay => "counter_replay",
            Self::SignatureEncoding => "signature_encoding",
            Self::Signature => "signature",
        }
    }
}

/// Verify Galaxy DEVICE registration against the Apple root, or identify an
/// explicitly dev-unattested Ed25519 key.
///
/// `manifest_ledger_close_time` is **Unix seconds** (convert XRPL's epoch first).
/// The caller checks validated ledgers, the registration window, chunk hash and
/// length, and XRPL transaction account binding before calling this function.
pub fn verify_registration(
    bundle: &DeviceBundle,
    manifest: &DeviceManifest,
    manifest_ledger_close_time: u64,
    accepted: &AcceptedAttestation,
) -> Result<VerifiedCredential, AttestError> {
    if bundle.network != manifest.network
        || bundle.kind != manifest.kind
        || bundle.env != manifest.env
        || bundle.key_id != manifest.key_id
        || bundle.account_id != manifest.account_id
        || bundle.reg_ledger_index != manifest.reg_ledger_index
        || bundle.reg_ledger_hash != manifest.reg_ledger_hash
    {
        return Err(AttestError::ManifestBinding);
    }
    let (public_key, class) = match bundle.kind {
        DeviceKind::AppleAppAttest => {
            let class = match bundle.env {
                AttestationEnv::Development => AttestationClass::AppleDevelopment,
                AttestationEnv::Production => AttestationClass::AppleProduction,
                AttestationEnv::NotApplicable => return Err(AttestError::Environment),
            };
            if !accepted.environments.contains(&bundle.env) {
                return Err(AttestError::Environment);
            }
            if !accepted.app_ids.contains(&bundle.app_id) {
                return Err(AttestError::AppId);
            }
            let client_data_hash = registration_client_data_hash(
                bundle.network,
                bundle.reg_ledger_index,
                &bundle.reg_ledger_hash,
                &bundle.reg_nonce,
                &bundle.key_id,
                &bundle.account_id,
            );
            let key = apple::verify_attestation(
                &bundle.attestation,
                &bundle.app_id,
                bundle.env,
                &bundle.key_id,
                &client_data_hash,
                manifest_ledger_close_time,
            )?;
            (CredentialPublicKey::AppleP256(key), class)
        }
        DeviceKind::DevUnattested => {
            if bundle.env != AttestationEnv::NotApplicable {
                return Err(AttestError::Environment);
            }
            let raw_key = bundle
                .attestation
                .as_slice()
                .try_into()
                .map_err(|_| AttestError::PublicKey)?;
            let key = ed25519_dalek::VerifyingKey::from_bytes(raw_key)
                .map_err(|_| AttestError::PublicKey)?;
            if key.is_weak() {
                return Err(AttestError::PublicKey);
            }
            if dev_key_id(raw_key) != bundle.key_id {
                return Err(AttestError::KeyId);
            }
            (
                CredentialPublicKey::DevEd25519(key),
                AttestationClass::DevUnattested,
            )
        }
    };
    Ok(VerifiedCredential {
        key_id: bundle.key_id,
        public_key,
        env: bundle.env,
        class,
        app_id: bundle.app_id.clone(),
        account: bundle.account_id,
        network: bundle.network,
    })
}

/// Verify the signature over the exact CAPTURE core and bind it to the
/// registration account and network. Apply [`verify_counter`] separately.
pub fn verify_capture_assertion(
    cred: &VerifiedCredential,
    core299: &[u8],
    assertion_bytes: &[u8],
) -> Result<AssertionInfo, AttestError> {
    let core = CaptureCore::decode(core299).map_err(|_| AttestError::CaptureCore)?;
    if core.account_id != cred.account || core.network != cred.network {
        return Err(AttestError::CaptureBinding);
    }
    let counter = match &cred.public_key {
        CredentialPublicKey::AppleP256(key) => {
            let client_data_hash = assertion_client_data_hash(core299);
            Some(apple::verify_assertion(
                assertion_bytes,
                &cred.app_id,
                &client_data_hash,
                key,
            )?)
        }
        CredentialPublicKey::DevEd25519(key) => {
            let Some((0xf0, signature)) = assertion_bytes.split_first() else {
                return Err(AttestError::SignatureEncoding);
            };
            let signature = ed25519_dalek::Signature::from_slice(signature)
                .map_err(|_| AttestError::SignatureEncoding)?;
            key.verify_strict(&dev_assertion_message(core299), &signature)
                .map_err(|_| AttestError::Signature)?;
            None
        }
    };
    Ok(AssertionInfo { counter })
}

/// Accept only a nonzero counter greater than every authenticated, strictly
/// earlier counter of this same registration. Input order is immaterial.
/// An already-seen exact ledger position is a replay; later positions are ignored.
pub fn verify_counter(
    candidate: CounterRecord,
    history: &[CounterRecord],
) -> Result<(), AttestError> {
    if candidate.counter == 0 {
        return Err(AttestError::Counter);
    }
    let position = (candidate.ledger_index, candidate.tx_index);
    for preceding in history {
        let other = (preceding.ledger_index, preceding.tx_index);
        if other == position || (other < position && preceding.counter >= candidate.counter) {
            return Err(AttestError::CounterReplay);
        }
    }
    Ok(())
}

fn sha256(parts: &[&[u8]]) -> Hash32 {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests;
