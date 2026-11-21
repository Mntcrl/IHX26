use std::sync::{atomic::AtomicBool, Arc};

use galaxy_crypto::field::fr_to_be;
use galaxy_evidence::{bundle, ingest, try_commit, Commitments, Salts};
use galaxy_protocol::{wire::CaptureCore, Hash32};

use crate::{guarded, GalaxyError, Result};

#[derive(Clone, Copy, uniffi::Enum)]
pub enum CaptureProfile {
    PngV1,
    JpegV1,
}

impl From<CaptureProfile> for galaxy_image::ImageProfile {
    fn from(value: CaptureProfile) -> Self {
        match value {
            CaptureProfile::PngV1 => Self::PngV1,
            CaptureProfile::JpegV1 => Self::JpegV1,
        }
    }
}

#[derive(uniffi::Record)]
pub struct CommitmentIds {
    pub photo_id: Vec<u8>,
    pub rgb_root: Vec<u8>,
    pub evidence_root: Vec<u8>,
}

/// Sensitive capture state. No sensor planes or salts are exposed.
#[derive(uniffi::Object)]
pub struct CaptureEvidence {
    pub(crate) bundle: bundle::EvidenceBundle,
    pub(crate) commitments: Commitments,
    pub(crate) photo_id: Hash32,
    pub(crate) statement_available: AtomicBool,
}

impl CaptureEvidence {
    pub(crate) fn from_bundle(bundle: bundle::EvidenceBundle, statement_available: bool) -> Result<Arc<Self>> {
        let commitments =
            try_commit(&bundle.evidence, &bundle.salts).map_err(|_| GalaxyError::Evidence)?;
        let photo_id = galaxy_image::photo_id(&bundle.evidence.image.rgb)
            .map_err(|_| GalaxyError::Evidence)?;
        Ok(Arc::new(Self {
            bundle,
            commitments,
            photo_id,
            statement_available: AtomicBool::new(statement_available),
        }))
    }

    pub(crate) fn profile(&self) -> galaxy_image::ImageProfile {
        self.bundle.evidence.image.profile
    }

    pub(crate) fn check_core(&self, core: &CaptureCore) -> Result<()> {
        if core.photo_id != self.photo_id
            || core.rgb_root != fr_to_be(&self.commitments.rgb_root)
            || core.evidence_root != fr_to_be(&self.commitments.evidence_root)
            || core.flags != u16::from(self.profile().code())
        {
            return Err(GalaxyError::CaptureBinding);
        }
        Ok(())
    }
}

#[uniffi::export]
pub fn ingest_gxrw(
    raw: Vec<u8>,
    profile: CaptureProfile,
    capture_state: Vec<u8>,
) -> Result<Arc<CaptureEvidence>> {
    guarded(|| {
        if capture_state.len() > bundle::MAX_CAPTURE_STATE_BYTES {
            return Err(GalaxyError::EvidenceBundle);
        }
        let evidence =
            ingest::from_gxrw(&raw, profile.into()).map_err(|_| GalaxyError::Evidence)?;
        drop(raw);
        let salts = Salts::random().map_err(|_| GalaxyError::Randomness)?;
        CaptureEvidence::from_bundle(bundle::EvidenceBundle {
            evidence,
            salts,
            capture_state,
        }, true)
    })
}

#[uniffi::export]
pub fn decode_evidence_bundle(bytes: Vec<u8>) -> Result<Arc<CaptureEvidence>> {
    guarded(|| {
        let bundle = bundle::decode(&bytes).map_err(|_| GalaxyError::EvidenceBundle)?;
        drop(bytes);
        // Restored evidence may be evaluated/exported only against the journaled statement.
        CaptureEvidence::from_bundle(bundle, false)
    })
}

#[uniffi::export]
impl CaptureEvidence {
    /// Invariant-size, 32-byte identifiers. The roots are canonical BN254 scalars.
    pub fn commitment_ids(&self) -> CommitmentIds {
        CommitmentIds {
            photo_id: self.photo_id.to_vec(),
            rgb_root: fr_to_be(&self.commitments.rgb_root).to_vec(),
            evidence_root: fr_to_be(&self.commitments.evidence_root).to_vec(),
        }
    }

    /// The exact PNG or JPEG whose reference-decoded pixels are committed. No XMP yet.
    pub fn canonical_image(&self) -> Vec<u8> {
        self.bundle.evidence.image.encoded.clone()
    }

    /// Sensitive GXEV bytes. Save under complete file protection, excluded from backup.
    pub fn encode_bundle(&self) -> Result<Vec<u8>> {
        guarded(|| bundle::encode(&self.bundle).map_err(|_| GalaxyError::EvidenceBundle))
    }

    pub fn capture_state(&self) -> Vec<u8> {
        self.bundle.capture_state.clone()
    }
}
