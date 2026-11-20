use der::asn1::{AnyRef, ObjectIdentifier, OctetStringRef};
use der::{oid::AssociatedOid, Decode, Document, Reader, Sequence, SliceReader};
use p384::ecdsa::signature::hazmat::PrehashVerifier;
use sha2::{Digest, Sha256, Sha384};
use std::{sync::LazyLock, time::Duration};
use x509_cert::certificate::Version;
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage};
use x509_cert::Certificate;

use crate::{sha256, AttestError};

const ROOT_PEM: &str = include_str!("Apple_App_Attestation_Root_CA.pem");
const ROOT_SHA256: [u8; 32] = [
    0x1c, 0xb9, 0x82, 0x3b, 0xa2, 0x8b, 0xa6, 0xad, 0x2d, 0x33, 0xa0, 0x06, 0x94, 0x1d, 0xe2, 0xae,
    0x4f, 0x51, 0x3e, 0xf1, 0xd4, 0xe8, 0x31, 0xb9, 0xf7, 0xe0, 0xfa, 0x7b, 0x62, 0x42, 0xc9, 0x32,
];
const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const P384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const ECDSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
const ECDSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
const APPLE_NONCE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113635.100.8.2");

pub(crate) fn verify_chain(
    leaf: &Certificate,
    leaf_der: &[u8],
    issuer_der: &[u8],
    unix_seconds: u64,
) -> Result<(), AttestError> {
    static ROOT: LazyLock<Result<(Certificate, p384::ecdsa::VerifyingKey), AttestError>> =
        LazyLock::new(load_root);
    let (root, root_key) = ROOT.as_ref().map_err(|e| *e)?;
    let issuer = Certificate::from_der(issuer_der).map_err(|_| AttestError::CertificateEncoding)?;
    for cert in [root, &issuer, leaf] {
        check_validity(cert, unix_seconds)?;
    }
    check_profile(&issuer, true)?;
    check_profile(leaf, false)?;
    verify_child(&issuer, issuer_der, root, root_key, ECDSA_SHA384)?;
    let issuer_key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_point(&issuer, P384)?)
        .map_err(|_| AttestError::CertificateProfile)?;
    // The credential is P-256, but its issuing CA signs it with P-384 and SHA-256.
    verify_child(leaf, leaf_der, &issuer, &issuer_key, ECDSA_SHA256)
}

fn load_root() -> Result<(Certificate, p384::ecdsa::VerifyingKey), AttestError> {
    let (label, root_der) = Document::from_pem(ROOT_PEM).map_err(|_| AttestError::RootPin)?;
    if label != "CERTIFICATE" || sha256(&[root_der.as_bytes()]) != ROOT_SHA256 {
        return Err(AttestError::RootPin);
    }
    let root = Certificate::from_der(root_der.as_bytes()).map_err(|_| AttestError::RootPin)?;
    let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_point(&root, P384)?)
        .map_err(|_| AttestError::RootPin)?;
    Ok((root, key))
}

fn check_validity(cert: &Certificate, unix_seconds: u64) -> Result<(), AttestError> {
    let validity = cert.tbs_certificate().validity();
    let at = Duration::from_secs(unix_seconds);
    if (validity.not_before.to_unix_duration()..=validity.not_after.to_unix_duration())
        .contains(&at)
    {
        Ok(())
    } else {
        Err(AttestError::CertificateTime)
    }
}

fn check_profile(cert: &Certificate, ca: bool) -> Result<(), AttestError> {
    let tbs = cert.tbs_certificate();
    let invalid = AttestError::CertificateProfile;
    if tbs.version() != Version::V3 {
        return Err(invalid);
    }
    let extensions = tbs.extensions().ok_or(invalid)?;
    for (i, ext) in extensions.iter().enumerate() {
        if (ext.critical && !matches!(ext.extn_id, BasicConstraints::OID | KeyUsage::OID))
            || extensions[..i]
                .iter()
                .any(|prev| prev.extn_id == ext.extn_id)
        {
            return Err(invalid);
        }
    }
    let (_, constraints) = tbs
        .get_extension::<BasicConstraints>()
        .map_err(|_| invalid)?
        .ok_or(invalid)?;
    let (_, usage) = tbs
        .get_extension::<KeyUsage>()
        .map_err(|_| invalid)?
        .ok_or(invalid)?;
    if constraints.ca != ca
        || usage.key_cert_sign() != ca
        || (!ca && (!usage.digital_signature() || constraints.path_len_constraint.is_some()))
    {
        return Err(invalid);
    }
    Ok(())
}

fn verify_child(
    cert: &Certificate,
    cert_der: &[u8],
    issuer: &Certificate,
    issuer_key: &p384::ecdsa::VerifyingKey,
    oid: ObjectIdentifier,
) -> Result<(), AttestError> {
    if cert.tbs_certificate().issuer() != issuer.tbs_certificate().subject() {
        return Err(AttestError::CertificateChain);
    }
    let algorithm = cert.signature_algorithm();
    if algorithm != cert.tbs_certificate().signature()
        || algorithm.oid != oid
        || algorithm.parameters.is_some()
    {
        return Err(AttestError::CertificateProfile);
    }
    let signature = cert
        .signature()
        .as_bytes()
        .ok_or(AttestError::CertificateEncoding)?;
    let signature = p384::ecdsa::Signature::from_der(signature)
        .map_err(|_| AttestError::CertificateEncoding)?;
    let tbs = tbs_bytes(cert_der).map_err(|_| AttestError::CertificateEncoding)?;
    let result = match oid {
        ECDSA_SHA256 => issuer_key.verify_prehash(&Sha256::digest(tbs), &signature),
        ECDSA_SHA384 => issuer_key.verify_prehash(&Sha384::digest(tbs), &signature),
        _ => return Err(AttestError::CertificateProfile),
    };
    result.map_err(|_| AttestError::CertificateChain)
}

// The whole certificate has already been decoded as DER. Verify over the first
// encoded SEQUENCE, including its header, not a reconstruction of the signed bytes.
fn tbs_bytes(cert_der: &[u8]) -> Result<&[u8], der::Error> {
    let body = AnyRef::from_der(cert_der)?.value();
    let mut reader = SliceReader::new(body)?;
    AnyRef::decode(&mut reader)?;
    Ok(&body[..usize::try_from(reader.position())?])
}

fn public_point(cert: &Certificate, curve: ObjectIdentifier) -> Result<&[u8], AttestError> {
    let spki = cert.tbs_certificate().subject_public_key_info();
    let invalid = AttestError::CertificateProfile;
    let named_curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|params| params.decode_as::<ObjectIdentifier>().ok());
    if spki.algorithm.oid != EC_PUBLIC_KEY || named_curve != Some(curve) {
        return Err(invalid);
    }
    spki.subject_public_key.as_bytes().ok_or(invalid)
}

pub(crate) fn credential_point(leaf: &Certificate) -> Result<&[u8; 65], AttestError> {
    let point: &[u8; 65] = public_point(leaf, P256)?
        .try_into()
        .map_err(|_| AttestError::CertificateProfile)?;
    if point[0] != 4 {
        return Err(AttestError::CertificateProfile);
    }
    Ok(point)
}

#[derive(Sequence)]
struct AppleNonce<'a> {
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT")]
    nonce: &'a OctetStringRef,
}

pub(crate) fn credential_nonce(leaf: &Certificate) -> Result<&[u8; 32], AttestError> {
    let invalid = AttestError::NonceEncoding;
    let ext = leaf
        .tbs_certificate()
        .extensions()
        .and_then(|exts| exts.iter().find(|ext| ext.extn_id == APPLE_NONCE))
        .ok_or(invalid)?;
    decode_nonce(ext.extn_value.as_bytes())
}

fn decode_nonce(input: &[u8]) -> Result<&[u8; 32], AttestError> {
    let invalid = AttestError::NonceEncoding;
    AppleNonce::from_der(input)
        .map_err(|_| invalid)?
        .nonce
        .as_bytes()
        .try_into()
        .map_err(|_| invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_apple_root_matches_published_fingerprint() {
        assert!(load_root().is_ok());
    }

    #[test]
    fn apple_nonce_requires_the_exact_der_envelope() {
        let mut data = [0; 38];
        data[..6].copy_from_slice(&[0x30, 0x24, 0xa1, 0x22, 0x04, 0x20]);
        assert_eq!(decode_nonce(&data), Ok(&[0; 32]));
        assert_eq!(decode_nonce(&data[..37]), Err(AttestError::NonceEncoding));
        assert_eq!(
            decode_nonce(&[data.as_slice(), &[0]].concat()),
            Err(AttestError::NonceEncoding)
        );
        data[2] = 0xa2;
        assert_eq!(decode_nonce(&data), Err(AttestError::NonceEncoding));
        assert_eq!(decode_nonce(&data[4..]), Err(AttestError::NonceEncoding));
    }
}
