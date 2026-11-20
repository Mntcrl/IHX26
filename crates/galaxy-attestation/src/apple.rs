use ciborium::value::Value;
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use x509_cert::{der::Decode, Certificate};

use crate::{certificate, sha256, AttestError, AttestationEnv, Hash32};

const ATTESTATION_LIMIT: usize = 1024 * 1024;
const ASSERTION_LIMIT: usize = 600;
const APPLE_FMT: &str = "apple-appattest";
const ATTESTED_CREDENTIAL_DATA: u8 = 0x40;
const EXTENSION_DATA: u8 = 0x80;
const AUTH_HEADER_LEN: usize = 37;
const AAGUID_DEVELOPMENT: &[u8; 16] = b"appattestdevelop";
const AAGUID_PRODUCTION: &[u8; 16] = b"appattest\0\0\0\0\0\0\0";

struct Attestation {
    leaf_der: Vec<u8>,
    issuer_der: Vec<u8>,
    auth_data: Vec<u8>,
}

impl Attestation {
    fn decode(input: &[u8]) -> Result<Self, AttestError> {
        if input.len() > ATTESTATION_LIMIT {
            return Err(AttestError::Size);
        }
        let mut input = input;
        let [fmt, statement, auth_data] =
            named_fields(read_cbor(&mut input)?, ["fmt", "attStmt", "authData"])?;
        if !input.is_empty() {
            return Err(AttestError::Cbor);
        }
        if fmt.as_ref().and_then(Value::as_text) != Some(APPLE_FMT) {
            return Err(AttestError::AttestationFormat);
        }
        let [x5c, receipt] = named_fields(statement.ok_or(AttestError::Cbor)?, ["x5c", "receipt"])?;
        let Some(Value::Array(chain)) = x5c else {
            return Err(AttestError::Cbor);
        };
        let [leaf, issuer] =
            <[_; 2]>::try_from(chain).map_err(|_| AttestError::CertificateChain)?;
        if let Some(receipt) = receipt {
            bytes(Some(receipt))?;
        }
        Ok(Self {
            leaf_der: bytes(Some(leaf))?,
            issuer_der: bytes(Some(issuer))?,
            auth_data: bytes(auth_data)?,
        })
    }
}

/// Re-encode the App Attest object as deterministic, receipt-free CBOR for XRPL.
/// This does not verify the attestation and never changes the certificate or authenticator bytes.
pub fn strip_receipt(input: &[u8]) -> Result<Vec<u8>, AttestError> {
    let att = Attestation::decode(input)?;
    // RFC 8949 deterministic order for these keys (length, then bytes).
    let value = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text(APPLE_FMT.into())),
        (
            Value::Text("attStmt".into()),
            Value::Map(vec![(
                Value::Text("x5c".into()),
                Value::Array(vec![
                    Value::Bytes(att.leaf_der),
                    Value::Bytes(att.issuer_der),
                ]),
            )]),
        ),
        (Value::Text("authData".into()), Value::Bytes(att.auth_data)),
    ]);
    let mut encoded = Vec::new();
    ciborium::into_writer(&value, &mut encoded).map_err(|_| AttestError::Cbor)?;
    Ok(encoded)
}

pub(crate) fn verify_attestation(
    input: &[u8],
    app_id: &str,
    env: AttestationEnv,
    key_id: &Hash32,
    client_data_hash: &Hash32,
    unix_seconds: u64,
) -> Result<VerifyingKey, AttestError> {
    let att = Attestation::decode(input)?;
    let auth = &att.auth_data;
    let leaf =
        Certificate::from_der(&att.leaf_der).map_err(|_| AttestError::CertificateEncoding)?;
    certificate::verify_chain(&leaf, &att.leaf_der, &att.issuer_der, unix_seconds)?;
    if certificate::credential_nonce(&leaf)? != &sha256(&[auth, client_data_hash]) {
        return Err(AttestError::NonceMismatch);
    }
    let point = certificate::credential_point(&leaf)?;
    if &sha256(&[point]) != key_id {
        return Err(AttestError::KeyId);
    }
    check_attested_auth_data(auth, app_id, env, key_id, point)?;
    VerifyingKey::from_sec1_bytes(point).map_err(|_| AttestError::PublicKey)
}

fn check_attested_auth_data(
    auth: &[u8],
    app_id: &str,
    env: AttestationEnv,
    key_id: &Hash32,
    point: &[u8; 65],
) -> Result<(), AttestError> {
    let (flags, counter) = auth_header(auth)?;
    verify_rp_id(auth, app_id)?;
    if flags & ATTESTED_CREDENTIAL_DATA == 0 {
        return Err(AttestError::Flags);
    }
    if counter != 0 {
        return Err(AttestError::Counter);
    }
    let body = auth
        .get(AUTH_HEADER_LEN..87)
        .ok_or(AttestError::AuthenticatorData)?;
    let aaguid = match env {
        AttestationEnv::Development => AAGUID_DEVELOPMENT,
        AttestationEnv::Production => AAGUID_PRODUCTION,
        AttestationEnv::NotApplicable => return Err(AttestError::Environment),
    };
    if &body[..16] != aaguid {
        return Err(AttestError::Environment);
    }
    if body[16..18] != [0, 32] || &body[18..] != key_id {
        return Err(AttestError::KeyId);
    }
    let mut cose = &auth[87..];
    check_cose_key(&mut cose, point)?;
    check_extensions(cose, flags)
}

fn check_cose_key(input: &mut &[u8], point: &[u8; 65]) -> Result<(), AttestError> {
    let invalid = AttestError::PublicKey;
    let Value::Map(fields) = read_cbor(input).map_err(|_| invalid)? else {
        return Err(invalid);
    };
    let mut seen = 0;
    for (label, value) in fields {
        let bit = match label.as_integer().map(i128::from) {
            Some(1) if value.as_integer().map(i128::from) == Some(2) => 1, // EC2
            Some(3) if value.as_integer().map(i128::from) == Some(-7) => 2, // ES256
            Some(-1) if value.as_integer().map(i128::from) == Some(1) => 4, // P-256
            Some(-2) if value.as_bytes().map(Vec::as_slice) == Some(&point[1..33]) => 8,
            Some(-3) if value.as_bytes().map(Vec::as_slice) == Some(&point[33..]) => 16,
            _ => return Err(invalid),
        };
        if seen & bit != 0 {
            return Err(invalid);
        }
        seen |= bit;
    }
    if seen != 31 {
        return Err(invalid);
    }
    Ok(())
}

pub(crate) fn verify_assertion(
    input: &[u8],
    app_id: &str,
    client_data_hash: &Hash32,
    key: &VerifyingKey,
) -> Result<u32, AttestError> {
    let assertion = decode_assertion(input)?;
    verify_rp_id(&assertion.auth_data, app_id)?;
    let nonce = sha256(&[&assertion.auth_data, client_data_hash]);
    // ES256 hashes nonce as the *message*, as confirmed against device assertions.
    key.verify(&nonce, &assertion.signature)
        .map_err(|_| AttestError::Signature)?;
    Ok(assertion.counter)
}

/// Extract a structurally valid Apple counter without authenticating it.
/// This is an indexing hint only; never use it as replay-policy input.
pub fn assertion_counter_hint(input: &[u8]) -> Result<u32, AttestError> {
    decode_assertion(input).map(|assertion| assertion.counter)
}

struct ParsedAssertion {
    signature: Signature,
    auth_data: Vec<u8>,
    counter: u32,
}

fn decode_assertion(mut input: &[u8]) -> Result<ParsedAssertion, AttestError> {
    if input.len() > ASSERTION_LIMIT {
        return Err(AttestError::Size);
    }
    let [signature, auth] =
        named_fields(read_cbor(&mut input)?, ["signature", "authenticatorData"])?;
    if !input.is_empty() {
        return Err(AttestError::Cbor);
    }
    let auth_data = bytes(auth)?;
    let (flags, counter) = auth_header(&auth_data)?;
    if counter == 0 {
        return Err(AttestError::Counter);
    }
    // App Attest uses a simplified authData layout for assertions: the legacy
    // AT bit may be set even though no attested-credential record follows.
    check_extensions(&auth_data[AUTH_HEADER_LEN..], flags)?;
    let signature =
        Signature::from_der(&bytes(signature)?).map_err(|_| AttestError::SignatureEncoding)?;
    Ok(ParsedAssertion {
        signature,
        auth_data,
        counter,
    })
}

fn auth_header(auth: &[u8]) -> Result<(u8, u32), AttestError> {
    let header: &[u8; AUTH_HEADER_LEN] =
        auth.first_chunk().ok_or(AttestError::AuthenticatorData)?;
    let [flags, a, b, c, d] = *header.last_chunk().ok_or(AttestError::AuthenticatorData)?;
    if flags & !(ATTESTED_CREDENTIAL_DATA | EXTENSION_DATA) != 0 {
        return Err(AttestError::Flags);
    }
    Ok((flags, u32::from_be_bytes([a, b, c, d])))
}

// The caller has already checked the 37-byte authenticator data header.
fn verify_rp_id(auth: &[u8], app_id: &str) -> Result<(), AttestError> {
    if auth[..32] == sha256(&[app_id.as_bytes()]) {
        Ok(())
    } else {
        Err(AttestError::AppId)
    }
}

fn check_extensions(mut input: &[u8], flags: u8) -> Result<(), AttestError> {
    if input.is_empty() {
        return if flags & EXTENSION_DATA == 0 {
            Ok(())
        } else {
            Err(AttestError::Extensions)
        };
    }
    // Apple's signed guide sample has a trailing extensions map with ED clear.
    // Only the documented Apple schema is allowed; no opaque trailing bytes.
    let invalid = AttestError::Extensions;
    let [category, version] = named_fields(
        read_cbor(&mut input).map_err(|_| invalid)?,
        ["apple_validation_category_01", "apple_bundle_version_01"],
    )
    .map_err(|_| invalid)?;
    let Some(Value::Bytes(category)) = category else {
        return Err(invalid);
    };
    let category = u32::from_le_bytes(category.try_into().map_err(|_| invalid)?);
    if !input.is_empty()
        || !matches!(category, 1..=6 | 10)
        || version
            .as_ref()
            .and_then(Value::as_text)
            .is_none_or(str::is_empty)
    {
        return Err(invalid);
    }
    Ok(())
}

fn read_cbor(input: &mut &[u8]) -> Result<Value, AttestError> {
    ciborium::de::from_reader_with_recursion_limit(input, 8).map_err(|_| AttestError::Cbor)
}

fn named_fields<const N: usize>(
    map: Value,
    names: [&str; N],
) -> Result<[Option<Value>; N], AttestError> {
    let Value::Map(map) = map else {
        return Err(AttestError::Cbor);
    };
    let mut fields = std::array::from_fn(|_| None);
    for (name, value) in map {
        let name = name.as_text().ok_or(AttestError::Cbor)?;
        let i = names
            .iter()
            .position(|&expected| name == expected)
            .ok_or(AttestError::Cbor)?;
        if fields[i].replace(value).is_some() {
            return Err(AttestError::Cbor);
        }
    }
    Ok(fields)
}

fn bytes(value: Option<Value>) -> Result<Vec<u8>, AttestError> {
    match value {
        Some(Value::Bytes(bytes)) => Ok(bytes),
        _ => Err(AttestError::Cbor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attestation_counter_must_be_zero() {
        let app_id = "EXAMPLE000.example.app";
        let mut header = [0u8; AUTH_HEADER_LEN];
        header[..32].copy_from_slice(&sha256(&[app_id.as_bytes()]));
        header[32] = ATTESTED_CREDENTIAL_DATA;
        header[36] = 1;
        assert_eq!(
            check_attested_auth_data(
                &header,
                app_id,
                AttestationEnv::Development,
                &[0; 32],
                &[0; 65],
            ),
            Err(AttestError::Counter)
        );
    }

    #[test]
    fn apple_documented_extensions_are_consumed_exactly() {
        // The current Apple validation guide includes this map with ED clear.
        let mut extensions = hex::decode(concat!(
            "a2776170706c655f62756e646c655f76657273696f6e5f30316131",
            "781c6170706c655f76616c69646174696f6e5f63617465676f72795f30314401000000",
        ))
        .unwrap();
        assert_eq!(
            check_extensions(&extensions, ATTESTED_CREDENTIAL_DATA),
            Ok(())
        );
        assert_eq!(check_extensions(&extensions, EXTENSION_DATA), Ok(()));
        assert_eq!(
            check_extensions(&[], EXTENSION_DATA),
            Err(AttestError::Extensions)
        );
        extensions.push(0);
        assert_eq!(
            check_extensions(&extensions, 0),
            Err(AttestError::Extensions)
        );
        extensions.pop();
        let category = extensions.len() - 4;
        extensions[category] = 7;
        assert_eq!(
            check_extensions(&extensions, 0),
            Err(AttestError::Extensions)
        );
    }
}
