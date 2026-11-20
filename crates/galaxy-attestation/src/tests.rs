use ciborium::value::Value;
use p256::ecdsa::{signature::hazmat::PrehashVerifier, Signature};
use serde::Deserialize;

use super::*;

mod vectors;

const PAIR: &str = include_str!("../../../test-vectors/attestation/public-ios-14.4-pair.json");
const PRODUCTION: &str = include_str!("../../../test-vectors/attestation/public-production.json");
const DEVICE: &str = include_str!("../../../test-vectors/attestation/device-development.json");

#[derive(Deserialize)]
struct Fixture {
    app_id: String,
    environment: String,
    verify_at_unix_seconds: u64,
    key_id_hex: String,
    registration_client_data_hex: String,
    attestation_cbor_hex: String,
    assertion_client_data_hex: Option<String>,
    assertion_cbor_hex: Option<String>,
    counter: Option<u32>,
}

impl Fixture {
    fn new(json: &str) -> Self {
        serde_json::from_str(json).unwrap()
    }

    fn env(&self) -> AttestationEnv {
        match self.environment.as_str() {
            "development" => AttestationEnv::Development,
            "production" => AttestationEnv::Production,
            other => panic!("invalid fixture environment: {other}"),
        }
    }

    fn key_id(&self) -> Hash32 {
        unhex(&self.key_id_hex).try_into().unwrap()
    }

    fn reg_hash(&self) -> Hash32 {
        sha256(&[&unhex(&self.registration_client_data_hex)])
    }

    fn att(&self) -> Vec<u8> {
        unhex(&self.attestation_cbor_hex)
    }

    fn verify(&self, att: &[u8]) -> Result<p256::ecdsa::VerifyingKey, AttestError> {
        apple::verify_attestation(
            att,
            &self.app_id,
            self.env(),
            &self.key_id(),
            &self.reg_hash(),
            self.verify_at_unix_seconds,
        )
    }
}

fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}

fn decode(bytes: &[u8]) -> Value {
    ciborium::from_reader(bytes).unwrap()
}

fn encode(value: &Value) -> Vec<u8> {
    let mut output = Vec::new();
    ciborium::into_writer(value, &mut output).unwrap();
    output
}

fn field<'a>(value: &'a mut Value, name: &str) -> &'a mut Value {
    &mut value
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some(name))
        .unwrap()
        .1
}

#[test]
fn real_development_attestation_and_assertion_share_key() {
    let f = Fixture::new(PAIR);
    let key = f.verify(&f.att()).unwrap();
    let assertion = unhex(f.assertion_cbor_hex.as_deref().unwrap());
    let hash = sha256(&[&unhex(f.assertion_client_data_hex.as_deref().unwrap())]);
    assert_eq!(
        apple::verify_assertion(&assertion, &f.app_id, &hash, &key),
        Ok(f.counter.unwrap())
    );
    assert_eq!(assertion_counter_hint(&assertion), Ok(f.counter.unwrap()));

    let mut different_counter = decode(&assertion);
    field(&mut different_counter, "authenticatorData")
        .as_bytes_mut()
        .unwrap()[36] = 2;
    let different_counter = encode(&different_counter);
    assert_eq!(assertion_counter_hint(&different_counter), Ok(2));
    assert_eq!(
        apple::verify_assertion(&different_counter, &f.app_id, &hash, &key),
        Err(AttestError::Signature)
    );

    let mut decoded = decode(&assertion);
    let nonce = sha256(&[
        field(&mut decoded, "authenticatorData").as_bytes().unwrap(),
        &hash,
    ]);
    let der = field(&mut decoded, "signature").as_bytes().unwrap();
    let sig = Signature::from_der(der).unwrap();
    assert!(
        key.verify_prehash(&nonce, &sig).is_err(),
        "nonce is not the ECDSA digest"
    );
    assert!(key.verify_prehash(&sha256(&[&nonce]), &sig).is_ok());
    assert_eq!(
        apple::verify_assertion(&assertion, &f.app_id, &[0; 32], &key),
        Err(AttestError::Signature)
    );
    assert_eq!(
        apple::verify_assertion(&assertion, "WRONG00000.example.app", &hash, &key),
        Err(AttestError::AppId)
    );
    *field(&mut decoded, "signature") = Value::Bytes(sig.to_bytes().to_vec());
    assert_eq!(
        apple::verify_assertion(&encode(&decoded), &f.app_id, &hash, &key),
        Err(AttestError::SignatureEncoding)
    );
    field(&mut decoded, "authenticatorData")
        .as_bytes_mut()
        .unwrap()[36] = 0;
    assert_eq!(
        apple::verify_assertion(&encode(&decoded), &f.app_id, &hash, &key),
        Err(AttestError::Counter)
    );
}

#[test]
fn real_production_attestation_depends_on_time_and_challenge() {
    let f = Fixture::new(PRODUCTION);
    let att = f.att();
    let verify = |app: &str, env, id: &Hash32, challenge: &Hash32, time| {
        apple::verify_attestation(&att, app, env, id, challenge, time)
    };
    let env = f.env();
    let id = f.key_id();
    let hash = f.reg_hash();
    let time = f.verify_at_unix_seconds;
    assert!(verify(&f.app_id, env, &id, &hash, time).is_ok());
    assert_eq!(
        verify(&f.app_id, env, &id, &hash, time - 2).unwrap_err(),
        AttestError::CertificateTime
    );
    assert_eq!(
        verify(&f.app_id, env, &id, &hash, time + 365 * 86_400).unwrap_err(),
        AttestError::CertificateTime
    );
    assert_eq!(
        verify(&f.app_id, env, &id, &[0x42; 32], time).unwrap_err(),
        AttestError::NonceMismatch
    );
    assert_eq!(
        verify("WRONG00000.example.app", env, &id, &hash, time).unwrap_err(),
        AttestError::AppId
    );
    assert_eq!(
        verify(&f.app_id, AttestationEnv::Development, &id, &hash, time).unwrap_err(),
        AttestError::Environment
    );
    assert_eq!(
        verify(&f.app_id, env, &[0x42; 32], &hash, time).unwrap_err(),
        AttestError::KeyId
    );
}

#[test]
fn wrong_trust_path_and_changed_authenticator_are_rejected() {
    let f = Fixture::new(PAIR);
    let mut att = decode(&f.att());
    let x5c = field(field(&mut att, "attStmt"), "x5c")
        .as_array_mut()
        .unwrap();
    // The intermediate still parses unchanged except for its root signature.
    *x5c[1].as_bytes_mut().unwrap().last_mut().unwrap() ^= 1;
    assert_eq!(
        f.verify(&encode(&att)).unwrap_err(),
        AttestError::CertificateChain
    );
    let mut att = decode(&f.att());
    field(&mut att, "authData").as_bytes_mut().unwrap()[36] = 1;
    assert_eq!(
        f.verify(&encode(&att)).unwrap_err(),
        AttestError::NonceMismatch
    );
}

#[test]
fn receipt_does_not_contribute_to_proof_and_is_stripped_canonically() {
    let f = Fixture::new(PAIR);
    let stripped = f.att();
    let mut att = decode(&stripped);
    field(&mut att, "attStmt").as_map_mut().unwrap().push((
        Value::Text("receipt".into()),
        // The field is not authenticated here; do not copy a real Apple risk receipt.
        Value::Bytes(b"opaque receipt bytes".to_vec()),
    ));
    att.as_map_mut().unwrap().reverse();
    let raw = encode(&att);
    assert_eq!(f.verify(&raw).unwrap(), f.verify(&stripped).unwrap());
    assert_eq!(strip_receipt(&raw).unwrap(), stripped);
    assert_eq!(strip_receipt(&stripped).unwrap(), stripped);
}

#[test]
fn duplicate_unknown_trailing_and_oversized_cbor_fail_closed() {
    let f = Fixture::new(PAIR);
    let mut att = f.att();
    att.push(0);
    assert_eq!(strip_receipt(&att), Err(AttestError::Cbor));
    let mut att = decode(&f.att());
    let fields = att.as_map_mut().unwrap();
    fields.push(fields[0].clone());
    assert_eq!(strip_receipt(&encode(&att)), Err(AttestError::Cbor));
    let mut att = decode(&f.att());
    att.as_map_mut()
        .unwrap()
        .push((Value::Text("extra".into()), Value::Null));
    assert_eq!(strip_receipt(&encode(&att)), Err(AttestError::Cbor));
    assert_eq!(
        strip_receipt(&vec![0; 1024 * 1024 + 1]),
        Err(AttestError::Size)
    );
}

#[test]
fn counter_uses_all_earlier_ledger_positions_regardless_of_input_order() {
    let record = |ledger_index, tx_index, counter| CounterRecord {
        ledger_index,
        tx_index,
        counter,
    };
    let history = [record(20, 2, 9), record(19, 4, 3), record(20, 4, 12)];
    assert_eq!(verify_counter(record(20, 3, 10), &history), Ok(()));
    assert_eq!(
        verify_counter(record(20, 3, 9), &history),
        Err(AttestError::CounterReplay)
    );
    assert_eq!(
        verify_counter(record(20, 2, 42), &history),
        Err(AttestError::CounterReplay)
    );
    assert_eq!(verify_counter(record(20, 1, 4), &history), Ok(()));
    assert_eq!(
        verify_counter(record(19, 1, 0), &[]),
        Err(AttestError::Counter)
    );
}
