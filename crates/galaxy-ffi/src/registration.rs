use galaxy_protocol::{
    ids,
    wire::{
        AttestationEnv, ChunkMemo, DeviceBundle, DeviceKind, DeviceManifest, MemoKind,
        MAX_BUNDLE_LEN,
    },
    AccountId, Hash32,
};
use galaxy_xrpl::registry::CHUNK_BYTES;

use crate::{exact, guarded, network_id, GalaxyError, Memo, Result, ValidatedLedger};

const REGISTRATION_WINDOW: u32 = 100;

#[derive(Clone, uniffi::Record)]
pub struct RegistrationChallenge {
    pub network: u32,
    pub ledger: ValidatedLedger,
    pub nonce: Vec<u8>,
    pub key_id: Vec<u8>,
    pub account_id: Vec<u8>,
}

#[derive(Clone, Copy, uniffi::Enum)]
pub enum AppAttestEnvironment {
    Development,
    Production,
}

#[derive(uniffi::Record)]
pub struct RegistrationClientData {
    pub client_data_hash: Vec<u8>,
    pub deadline: u32,
}

#[derive(uniffi::Record)]
pub struct DevicePublication {
    pub bundle: Vec<u8>,
    pub bundle_id: Vec<u8>,
    /// In chunk index order. Each must independently validate with tesSUCCESS.
    pub chunks: Vec<Memo>,
    pub deadline: u32,
}

struct CheckedChallenge {
    ledger_hash: Hash32,
    nonce: Hash32,
    key_id: Hash32,
    account_id: AccountId,
}

impl RegistrationChallenge {
    fn checked(&self) -> Result<CheckedChallenge> {
        network_id(self.network)?;
        Ok(CheckedChallenge {
            ledger_hash: self.ledger.checked_hash()?,
            nonce: exact(&self.nonce)?,
            key_id: exact(&self.key_id)?,
            account_id: exact(&self.account_id)?,
        })
    }
}

fn registration_deadline(index: u32) -> Result<u32> {
    index
        .checked_add(REGISTRATION_WINDOW)
        .filter(|_| index != 0)
        .ok_or(GalaxyError::LedgerWindow)
}

#[uniffi::export]
pub fn device_registration_client_data(
    input: RegistrationChallenge,
) -> Result<RegistrationClientData> {
    guarded(|| {
        let checked = input.checked()?;
        let hash = ids::registration_client_data_hash(
            input.network,
            input.ledger.index,
            &checked.ledger_hash,
            &checked.nonce,
            &checked.key_id,
            &checked.account_id,
        );
        Ok(RegistrationClientData {
            client_data_hash: hash.to_vec(),
            deadline: registration_deadline(input.ledger.index)?,
        })
    })
}

#[uniffi::export]
pub fn prepare_device_publication(
    input: RegistrationChallenge,
    app_id: String,
    environment: AppAttestEnvironment,
    attestation: Vec<u8>,
) -> Result<DevicePublication> {
    guarded(|| {
        let checked = input.checked()?;
        let deadline = registration_deadline(input.ledger.index)?;
        if app_id.len() > usize::from(u16::MAX) || attestation.len() > MAX_BUNDLE_LEN {
            return Err(GalaxyError::Attestation);
        }
        let attestation = galaxy_attestation::strip_receipt(&attestation)
            .map_err(|_| GalaxyError::Attestation)?;
        let device = DeviceBundle {
            kind: DeviceKind::AppleAppAttest,
            env: match environment {
                AppAttestEnvironment::Development => AttestationEnv::Development,
                AppAttestEnvironment::Production => AttestationEnv::Production,
            },
            app_id,
            key_id: checked.key_id,
            network: input.network,
            reg_ledger_index: input.ledger.index,
            reg_ledger_hash: checked.ledger_hash,
            reg_nonce: checked.nonce,
            account_id: checked.account_id,
            attestation,
        };
        let bundle = device.encode();
        DeviceBundle::decode(&bundle).map_err(|_| GalaxyError::Attestation)?;
        let bundle_id = ids::bundle_id(&bundle);
        let total = chunk_total(&bundle)?;
        let chunks = bundle
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(index, payload)| {
                Memo::new(
                    MemoKind::DeviceChunk,
                    ChunkMemo {
                        bundle_id,
                        index: index as u16,
                        total,
                        payload: payload.to_vec(),
                    }
                    .encode(),
                )
            })
            .collect();
        Ok(DevicePublication {
            bundle,
            bundle_id: bundle_id.to_vec(),
            chunks,
            deadline,
        })
    })
}

fn chunk_total(bundle: &[u8]) -> Result<u16> {
    u16::try_from(bundle.len().div_ceil(CHUNK_BYTES)).map_err(|_| GalaxyError::Attestation)
}

/// `chunk_ledgers` are the validated tesSUCCESS inclusion indices, in chunk order.
#[uniffi::export]
pub fn build_device_manifest(bundle: Vec<u8>, chunk_ledgers: Vec<u32>) -> Result<Memo> {
    guarded(|| {
        let device = DeviceBundle::decode(&bundle).map_err(|_| GalaxyError::Attestation)?;
        if device.kind != DeviceKind::AppleAppAttest {
            return Err(GalaxyError::Attestation);
        }
        let chunk_total = chunk_total(&bundle)?;
        let (Some(&first), Some(&last)) = (chunk_ledgers.first(), chunk_ledgers.last()) else {
            return Err(GalaxyError::LedgerWindow);
        };
        if chunk_ledgers.len() != usize::from(chunk_total)
            || first <= device.reg_ledger_index
            || last >= registration_deadline(device.reg_ledger_index)?
            || !chunk_ledgers.is_sorted()
        {
            return Err(GalaxyError::LedgerWindow);
        }
        let manifest = DeviceManifest {
            network: device.network,
            bundle_id: ids::bundle_id(&bundle),
            bundle_len: bundle.len() as u32,
            chunk_total,
            first_chunk_ledger: first,
            last_chunk_ledger: last,
            kind: device.kind,
            env: device.env,
            key_id: device.key_id,
            account_id: device.account_id,
            reg_ledger_index: device.reg_ledger_index,
            reg_ledger_hash: device.reg_ledger_hash,
        };
        Ok(Memo::new(MemoKind::Device, manifest.encode()))
    })
}
