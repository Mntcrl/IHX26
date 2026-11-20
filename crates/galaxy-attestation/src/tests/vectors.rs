//! Golden cases in `test-vectors/attestation/cases.json`, derived from the
//! public Apple fixtures and a labelled test-only Ed25519 key. Regenerate with
//! `GALAXY_WRITE_VECTORS=1 cargo test -p galaxy-attestation vectors`.

use ciborium::value::Value;
use ed25519_dalek::{Signer, SigningKey};
use galaxy_protocol::ids::bundle_id;
use serde_json::{json, Value as Json};

use super::*;

const CASES_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-vectors/attestation/cases.json"
);
/// Test-only development key seed; only its public key and signatures are published.
const DEV_SEED: [u8; 32] = *b"GALAXY test-only dev-unattested!";
const NETWORK: u32 = 1;
const ACCOUNT: AccountId = [0x11; 20];

fn h(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn env_name(env: AttestationEnv) -> &'static str {
    match env {
        AttestationEnv::NotApplicable => "not_applicable",
        AttestationEnv::Development => "development",
        AttestationEnv::Production => "production",
    }
}

fn parse_env(name: &str) -> AttestationEnv {
    serde_json::from_value(Json::String(name.into())).unwrap()
}

fn class_name(class: AttestationClass) -> &'static str {
    match class {
        AttestationClass::AppleDevelopment => "apple_development",
        AttestationClass::AppleProduction => "apple_production",
        AttestationClass::DevUnattested => "dev_unattested",
    }
}

fn capture_core(account: AccountId, policy_byte: u8) -> Vec<u8> {
    CaptureCore {
        network: NETWORK,
        registration_tx: [0x21; 32],
        preledger_hash: [0x22; 32],
        nonce: [0x23; 32],
        preledger_index: 1000,
        claimed_time_ms: 1_763_600_000_000,
        width: 4032,
        height: 3024,
        flags: 0,
        photo_id: [0x24; 32],
        rgb_root: [0x05; 32],
        evidence_root: [0x06; 32],
        policy_id: [policy_byte; 32],
        circuit_id: [0x27; 32],
        account_id: account,
    }
    .encode()
}

fn bundle(
    kind: DeviceKind,
    env: AttestationEnv,
    app_id: &str,
    key_id: Hash32,
    attestation: Vec<u8>,
) -> DeviceBundle {
    DeviceBundle {
        kind,
        env,
        app_id: app_id.into(),
        key_id,
        network: NETWORK,
        reg_ledger_index: 900,
        reg_ledger_hash: [0x31; 32],
        reg_nonce: [0x32; 32],
        account_id: ACCOUNT,
        attestation,
    }
}

fn manifest(bundle: &DeviceBundle) -> DeviceManifest {
    let encoded = bundle.encode();
    let len = u32::try_from(encoded.len()).unwrap();
    DeviceManifest {
        network: bundle.network,
        bundle_id: bundle_id(&encoded),
        bundle_len: len,
        chunk_total: u16::try_from(len.div_ceil(800)).unwrap(),
        first_chunk_ledger: 905,
        last_chunk_ledger: 910,
        kind: bundle.kind,
        env: bundle.env,
        key_id: bundle.key_id,
        account_id: bundle.account_id,
        reg_ledger_index: bundle.reg_ledger_index,
        reg_ledger_hash: bundle.reg_ledger_hash,
    }
}

fn case(name: &str, check: &str, input: Json, expected: Result<Json, AttestError>) -> Json {
    let (expected, result) = match expected {
        Ok(result) => ("ok", result),
        Err(error) => (error.code(), Json::Null),
    };
    json!({"name": name, "check": check, "input": input, "expected": expected, "result": result})
}

fn attestation_input(
    f: &Fixture,
    att: &[u8],
    app_id: &str,
    env: AttestationEnv,
    key_id: &Hash32,
    challenge: &Hash32,
    time: u64,
) -> Json {
    json!({
        "attestation_cbor_hex": h(att), "app_id": app_id, "environment": env_name(env),
        "key_id_hex": h(key_id), "client_data_hash_hex": h(challenge), "verify_at_unix_seconds": time,
        "source": if f.env() == AttestationEnv::Production { "public-production.json" } else { "public-ios-14.4-pair.json" },
    })
}
/// Unix-second validity bounds of the credential certificate (`x5c[0]`).
fn leaf_validity(att: &[u8]) -> (u64, u64) {
    use x509_cert::der::Decode;
    let mut att = decode(att);
    let x5c = field(field(&mut att, "attStmt"), "x5c").as_array().unwrap();
    let leaf = x509_cert::Certificate::from_der(x5c[0].as_bytes().unwrap()).unwrap();
    let validity = leaf.tbs_certificate().validity();
    (
        validity.not_before.to_unix_duration().as_secs(),
        validity.not_after.to_unix_duration().as_secs(),
    )
}

fn generate() -> Json {
    let mut cases = Vec::new();
    for (label, f) in [
        ("production", Fixture::new(PRODUCTION)),
        ("development", Fixture::new(PAIR)),
    ] {
        let (att, env, id, hash, time) = (
            f.att(),
            f.env(),
            f.key_id(),
            f.reg_hash(),
            f.verify_at_unix_seconds,
        );
        let point = f.verify(&att).unwrap().to_sec1_point(false);
        let ok = || Ok(json!({"public_key_sec1_hex": h(point.as_bytes())}));
        let mut push = |name: &str,
                        att: &[u8],
                        app: &str,
                        env,
                        id: &Hash32,
                        challenge: &Hash32,
                        time,
                        expected| {
            let input = attestation_input(&f, att, app, env, id, challenge, time);
            cases.push(case(
                &format!("attestation_{label}_{name}"),
                "attestation",
                input,
                expected,
            ));
        };
        push("valid", &att, &f.app_id, env, &id, &hash, time, ok());
        let (not_before, not_after) = leaf_validity(&att);
        push(
            "not_yet_valid",
            &att,
            &f.app_id,
            env,
            &id,
            &hash,
            not_before - 1,
            Err(AttestError::CertificateTime),
        );
        push(
            "expired",
            &att,
            &f.app_id,
            env,
            &id,
            &hash,
            not_after + 1,
            Err(AttestError::CertificateTime),
        );
        push(
            "other_challenge",
            &att,
            &f.app_id,
            env,
            &id,
            &[0x42; 32],
            time,
            Err(AttestError::NonceMismatch),
        );
        push(
            "wrong_app_id",
            &att,
            "WRONG00000.example.app",
            env,
            &id,
            &hash,
            time,
            Err(AttestError::AppId),
        );
        let other_env = if env == AttestationEnv::Production {
            AttestationEnv::Development
        } else {
            AttestationEnv::Production
        };
        push(
            "wrong_environment",
            &att,
            &f.app_id,
            other_env,
            &id,
            &hash,
            time,
            Err(AttestError::Environment),
        );
        push(
            "wrong_key_id",
            &att,
            &f.app_id,
            env,
            &[0x42; 32],
            &hash,
            time,
            Err(AttestError::KeyId),
        );

        let mut wrong_root = decode(&att);
        let x5c = field(field(&mut wrong_root, "attStmt"), "x5c")
            .as_array_mut()
            .unwrap();
        *x5c[1].as_bytes_mut().unwrap().last_mut().unwrap() ^= 1;
        push(
            "intermediate_not_signed_by_pinned_root",
            &encode(&wrong_root),
            &f.app_id,
            env,
            &id,
            &hash,
            time,
            Err(AttestError::CertificateChain),
        );

        let mut swapped = decode(&att);
        field(field(&mut swapped, "attStmt"), "x5c")
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        push(
            "chain_order_swapped",
            &encode(&swapped),
            &f.app_id,
            env,
            &id,
            &hash,
            time,
            Err(AttestError::CertificateProfile),
        );

        let mut nonzero_counter = decode(&att);
        field(&mut nonzero_counter, "authData")
            .as_bytes_mut()
            .unwrap()[36] = 1;
        push(
            "auth_data_counter_changed",
            &encode(&nonzero_counter),
            &f.app_id,
            env,
            &id,
            &hash,
            time,
            Err(AttestError::NonceMismatch),
        );

        let mut format = decode(&att);
        *field(&mut format, "fmt") = Value::Text("packed".into());
        push(
            "wrong_format",
            &encode(&format),
            &f.app_id,
            env,
            &id,
            &hash,
            time,
            Err(AttestError::AttestationFormat),
        );

        let mut with_receipt = decode(&att);
        field(&mut with_receipt, "attStmt")
            .as_map_mut()
            .unwrap()
            .push((
                Value::Text("receipt".into()),
                Value::Bytes(b"opaque receipt bytes".to_vec()),
            ));
        let with_receipt = encode(&with_receipt);
        assert_eq!(strip_receipt(&with_receipt).unwrap(), att);
        push(
            "receipt_present",
            &with_receipt,
            &f.app_id,
            env,
            &id,
            &hash,
            time,
            ok(),
        );
        cases.push(case(
            &format!("strip_receipt_{label}"),
            "strip_receipt",
            json!({"attestation_cbor_hex": h(&with_receipt)}),
            Ok(json!({"stripped_cbor_hex": h(&att)})),
        ));
    }

    let f = Fixture::new(PAIR);
    let key = f.verify(&f.att()).unwrap();
    let point = key.to_sec1_point(false);
    let assertion = unhex(f.assertion_cbor_hex.as_deref().unwrap());
    let hash = sha256(&[&unhex(f.assertion_client_data_hex.as_deref().unwrap())]);
    let mut push = |name: &str, assertion: &[u8], app: &str, hash: &Hash32, expected| {
        let input = json!({
            "assertion_cbor_hex": h(assertion), "app_id": app, "client_data_hash_hex": h(hash),
            "public_key_sec1_hex": h(point.as_bytes()), "source": "public-ios-14.4-pair.json",
        });
        cases.push(case(
            &format!("assertion_{name}"),
            "assertion",
            input,
            expected,
        ));
    };
    push(
        "valid",
        &assertion,
        &f.app_id,
        &hash,
        Ok(json!({"counter": f.counter.unwrap()})),
    );
    push(
        "other_client_data",
        &assertion,
        &f.app_id,
        &[0; 32],
        Err(AttestError::Signature),
    );
    push(
        "wrong_app_id",
        &assertion,
        "WRONG00000.example.app",
        &hash,
        Err(AttestError::AppId),
    );
    let mut changed = decode(&assertion);
    field(&mut changed, "authenticatorData")
        .as_bytes_mut()
        .unwrap()[36] = 2;
    push(
        "counter_changed",
        &encode(&changed),
        &f.app_id,
        &hash,
        Err(AttestError::Signature),
    );
    field(&mut changed, "authenticatorData")
        .as_bytes_mut()
        .unwrap()[36] = 0;
    push(
        "zero_counter",
        &encode(&changed),
        &f.app_id,
        &hash,
        Err(AttestError::Counter),
    );
    let mut raw_sig = decode(&assertion);
    let der = field(&mut raw_sig, "signature").as_bytes().unwrap().clone();
    *field(&mut raw_sig, "signature") =
        Value::Bytes(Signature::from_der(&der).unwrap().to_bytes().to_vec());
    push(
        "raw_signature_encoding",
        &encode(&raw_sig),
        &f.app_id,
        &hash,
        Err(AttestError::SignatureEncoding),
    );
    let mut trailing = assertion.clone();
    trailing.push(0);
    push(
        "trailing_bytes",
        &trailing,
        &f.app_id,
        &hash,
        Err(AttestError::Cbor),
    );

    let record = |l: u32, t: u32, c: u32| json!({"ledger_index": l, "tx_index": t, "counter": c});
    let history = json!([record(20, 2, 9), record(19, 4, 3), record(20, 4, 12)]);
    for (name, candidate, expected) in [
        ("advances", record(20, 3, 10), Ok(Json::Null)),
        (
            "not_greater_than_earlier",
            record(20, 3, 9),
            Err(AttestError::CounterReplay),
        ),
        (
            "same_ledger_position",
            record(20, 2, 42),
            Err(AttestError::CounterReplay),
        ),
        ("later_records_ignored", record(20, 1, 4), Ok(Json::Null)),
        ("zero", record(21, 0, 0), Err(AttestError::Counter)),
    ] {
        let input = json!({"candidate": candidate, "history": history});
        cases.push(case(&format!("counter_{name}"), "counter", input, expected));
    }

    let accepted =
        |f: &Fixture| json!({"app_ids": [f.app_id], "environments": [env_name(f.env())]});
    let p = Fixture::new(PRODUCTION);
    let apple = bundle(
        DeviceKind::AppleAppAttest,
        p.env(),
        &p.app_id,
        p.key_id(),
        p.att(),
    );
    let mut push = |name: &str,
                    bundle: &DeviceBundle,
                    manifest: &DeviceManifest,
                    time: u64,
                    accepted: Json,
                    core: &[u8],
                    assertion: &[u8],
                    expected| {
        let mut input = json!({
            "device_bundle_hex": h(&bundle.encode()), "device_manifest_hex": h(&manifest.encode()),
            "manifest_ledger_close_unix_seconds": time, "accepted": accepted,
        });
        let check = if core.is_empty() {
            "registration"
        } else {
            input["core299_hex"] = json!(h(core));
            input["assertion_hex"] = json!(h(assertion));
            "registration_and_capture"
        };
        cases.push(case(
            &format!("registration_{name}"),
            check,
            input,
            expected,
        ));
    };
    let core0 = capture_core(ACCOUNT, 0x26);
    push(
        "apple_not_bound_to_galaxy_challenge",
        &apple,
        &manifest(&apple),
        p.verify_at_unix_seconds,
        accepted(&p),
        &core0,
        &[],
        Err(AttestError::NonceMismatch),
    );
    push(
        "apple_environment_not_accepted",
        &apple,
        &manifest(&apple),
        p.verify_at_unix_seconds,
        json!({"app_ids": [p.app_id], "environments": ["development"]}),
        &[],
        &[],
        Err(AttestError::Environment),
    );
    push(
        "apple_app_id_not_accepted",
        &apple,
        &manifest(&apple),
        p.verify_at_unix_seconds,
        json!({"app_ids": ["OTHER00000.example.app"], "environments": ["production"]}),
        &[],
        &[],
        Err(AttestError::AppId),
    );

    let signing = SigningKey::from_bytes(&DEV_SEED);
    let public = signing.verifying_key().to_bytes();
    let dev = bundle(
        DeviceKind::DevUnattested,
        AttestationEnv::NotApplicable,
        "",
        dev_key_id(&public),
        public.to_vec(),
    );
    let dev_manifest = manifest(&dev);
    let sign = |core: &[u8]| {
        [
            &[0xf0][..],
            &signing.sign(&dev_assertion_message(core)).to_bytes(),
        ]
        .concat()
    };
    let none = json!({"app_ids": [], "environments": []});
    let mut dev_ok = credential_json(
        "dev_unattested",
        AttestationEnv::NotApplicable,
        "",
        &dev.key_id,
        &public,
    );
    dev_ok["counter"] = Json::Null;
    let dev_ok = Ok(dev_ok);
    push(
        "dev_unattested_valid",
        &dev,
        &dev_manifest,
        0,
        none.clone(),
        &core0,
        &sign(&core0),
        dev_ok,
    );
    let core1 = capture_core(ACCOUNT, 0x28);
    push(
        "dev_unattested_other_core",
        &dev,
        &dev_manifest,
        0,
        none.clone(),
        &core1,
        &sign(&core0),
        Err(AttestError::Signature),
    );
    let other_account = capture_core([0x12; 20], 0x26);
    push(
        "dev_unattested_other_account",
        &dev,
        &dev_manifest,
        0,
        none.clone(),
        &other_account,
        &sign(&other_account),
        Err(AttestError::CaptureBinding),
    );
    push(
        "dev_unattested_missing_kind_prefix",
        &dev,
        &dev_manifest,
        0,
        none.clone(),
        &core0,
        &sign(&core0)[1..],
        Err(AttestError::SignatureEncoding),
    );
    let mut wrong_id = dev.clone();
    wrong_id.key_id = sha256(&[&public]);
    push(
        "dev_unattested_undomained_key_id",
        &wrong_id,
        &manifest(&wrong_id),
        0,
        none.clone(),
        &core0,
        &sign(&core0),
        Err(AttestError::KeyId),
    );
    let mut other_manifest = dev_manifest.clone();
    other_manifest.reg_ledger_index += 1;
    push(
        "manifest_differs_from_bundle",
        &dev,
        &other_manifest,
        0,
        none,
        &core0,
        &sign(&core0),
        Err(AttestError::ManifestBinding),
    );

    let d = DeviceFixture::load();
    let device = d.bundle();
    let point = leaf_point(&device.attestation);
    assert_eq!(sha256(&[&point]), device.key_id);
    let device_ok = Ok(credential_json(
        "apple_development",
        AttestationEnv::Development,
        &d.app_id,
        &device.key_id,
        &point,
    ));
    let accepted_dev = json!({"app_ids": [d.app_id], "environments": ["development"]});
    let at = d.verify_at_unix_seconds;
    let (_, device_not_after) = leaf_validity(&device.attestation);
    let with = |change: &dyn Fn(&mut DeviceBundle)| {
        let mut changed = device.clone();
        change(&mut changed);
        changed
    };
    let other_account = with(&|b| b.account_id = [0x12; 20]);
    let other_ledger = with(&|b| b.reg_ledger_hash = [0x5a; 32]);
    let declared_production = with(&|b| b.env = AttestationEnv::Production);
    let other_app = with(&|b| b.app_id = "PPS57BF3X3.utcq.Other".into());
    for (name, bundle, time, accepted, expected) in [
        (
            "device_development_valid",
            &device,
            at,
            accepted_dev.clone(),
            device_ok,
        ),
        (
            "device_development_not_accepted",
            &device,
            at,
            json!({"app_ids": [d.app_id], "environments": ["production"]}),
            Err(AttestError::Environment),
        ),
        (
            "device_declared_production",
            &declared_production,
            at,
            json!({"app_ids": [d.app_id], "environments": ["production"]}),
            Err(AttestError::Environment),
        ),
        (
            "device_manifest_after_credential_expiry",
            &device,
            device_not_after + 1,
            accepted_dev.clone(),
            Err(AttestError::CertificateTime),
        ),
        (
            "device_other_account",
            &other_account,
            at,
            accepted_dev.clone(),
            Err(AttestError::NonceMismatch),
        ),
        (
            "device_other_registration_ledger",
            &other_ledger,
            at,
            accepted_dev.clone(),
            Err(AttestError::NonceMismatch),
        ),
        (
            "device_other_app_id",
            &other_app,
            at,
            json!({"app_ids": ["PPS57BF3X3.utcq.Other"], "environments": ["development"]}),
            Err(AttestError::AppId),
        ),
    ] {
        push(
            name,
            bundle,
            &manifest(bundle),
            time,
            accepted,
            &[],
            &[],
            expected,
        );
    }

    let device_assertion =
        |name: &str, assertion: &DeviceAssertion, client_data: &str, app: &str, expected| {
            let input = json!({
                "assertion_cbor_hex": assertion.assertion_cbor_hex, "app_id": app,
                "client_data_hash_hex": h(&sha256(&[&unhex(client_data)])),
                "public_key_sec1_hex": h(&point), "source": "device-development.json",
            });
            case(&format!("assertion_{name}"), "assertion", input, expected)
        };
    let [first, second] = &d.assertions[..] else {
        panic!("device fixture must hold two assertions");
    };
    cases.extend([
        device_assertion(
            "device_first",
            first,
            &first.client_data_hex,
            &d.app_id,
            Ok(json!({"counter": first.counter})),
        ),
        device_assertion(
            "device_second",
            second,
            &second.client_data_hex,
            &d.app_id,
            Ok(json!({"counter": second.counter})),
        ),
        device_assertion(
            "device_first_over_second_client_data",
            first,
            &second.client_data_hex,
            &d.app_id,
            Err(AttestError::Signature),
        ),
        device_assertion(
            "device_second_wrong_app_id",
            second,
            &second.client_data_hex,
            "PPS57BF3X3.utcq.Other",
            Err(AttestError::AppId),
        ),
    ]);
    for (name, earlier, later, expected) in [
        ("device_second_after_first", first, second, Ok(Json::Null)),
        (
            "device_first_replayed_after_second",
            second,
            first,
            Err(AttestError::CounterReplay),
        ),
    ] {
        let input = json!({
            "candidate": record(1001, 0, later.counter),
            "history": [record(1000, 3, earlier.counter)],
        });
        cases.push(case(&format!("counter_{name}"), "counter", input, expected));
    }

    json!({
        "description": "Galaxy App Attest verification cases; see README.md. Apple objects derive from the public fixtures and the test-device fixture; the dev-unattested key is a labelled test-only key.",
        "cases": cases,
    })
}

fn hex_field<const N: usize>(input: &Json, name: &str) -> [u8; N] {
    unhex(input[name].as_str().unwrap()).try_into().unwrap()
}

fn bytes_field(input: &Json, name: &str) -> Vec<u8> {
    unhex(input[name].as_str().unwrap())
}

fn counter_record(value: &Json) -> CounterRecord {
    let n = |k: &str| u32::try_from(value[k].as_u64().unwrap()).unwrap();
    CounterRecord {
        ledger_index: n("ledger_index"),
        tx_index: n("tx_index"),
        counter: n("counter"),
    }
}

#[derive(serde::Deserialize)]
struct DeviceFixture {
    app_id: String,
    environment: String,
    verify_at_unix_seconds: u64,
    key_id_hex: String,
    registration_client_data_hex: String,
    attestation_cbor_hex: String,
    assertions: Vec<DeviceAssertion>,
}

#[derive(serde::Deserialize)]
struct DeviceAssertion {
    client_data_hex: String,
    assertion_cbor_hex: String,
    counter: u32,
}

impl DeviceFixture {
    fn load() -> Self {
        serde_json::from_str(DEVICE).unwrap()
    }

    /// Rebuild the DEVICE bundle from the §3 registration preimage the app attested.
    fn bundle(&self) -> DeviceBundle {
        let preimage = unhex(&self.registration_client_data_hex);
        let (tag, rest) = preimage.split_at(21);
        assert_eq!(tag, b"GALAXY/DEVICE-REG/v1\0");
        assert_eq!(rest.len(), 124);
        let word = |i: usize| u32::from_be_bytes(rest[i..i + 4].try_into().unwrap());
        let hash = |i: usize| -> Hash32 { rest[i..i + 32].try_into().unwrap() };
        let bundle = DeviceBundle {
            kind: DeviceKind::AppleAppAttest,
            env: parse_env(&self.environment),
            app_id: self.app_id.clone(),
            key_id: hash(72),
            network: word(0),
            reg_ledger_index: word(4),
            reg_ledger_hash: hash(8),
            reg_nonce: hash(40),
            account_id: rest[104..].try_into().unwrap(),
            attestation: unhex(&self.attestation_cbor_hex),
        };
        assert_eq!(h(&bundle.key_id), self.key_id_hex);
        // Published form: receipt removed, deterministic outer CBOR.
        assert_eq!(
            strip_receipt(&bundle.attestation).unwrap(),
            bundle.attestation
        );
        assert_eq!(
            registration_client_data_hash(
                bundle.network,
                bundle.reg_ledger_index,
                &bundle.reg_ledger_hash,
                &bundle.reg_nonce,
                &bundle.key_id,
                &bundle.account_id,
            ),
            sha256(&[&preimage]),
        );
        bundle
    }
}

/// Uncompressed credential public key from the certificate, independent of the verifier.
fn leaf_point(att: &[u8]) -> Vec<u8> {
    use x509_cert::der::Decode;
    let mut att = decode(att);
    let x5c = field(field(&mut att, "attStmt"), "x5c").as_array().unwrap();
    let leaf = x509_cert::Certificate::from_der(x5c[0].as_bytes().unwrap()).unwrap();
    let spki = leaf.tbs_certificate().subject_public_key_info();
    spki.subject_public_key.as_bytes().unwrap().to_vec()
}

fn credential_json(
    class: &str,
    env: AttestationEnv,
    app_id: &str,
    key_id: &Hash32,
    public_key: &[u8],
) -> Json {
    json!({
        "class": class, "environment": env_name(env), "app_id": app_id,
        "key_id_hex": h(key_id), "public_key_hex": h(public_key),
    })
}

fn registration(input: &Json) -> Result<VerifiedCredential, AttestError> {
    let accepted = AcceptedAttestation {
        app_ids: serde_json::from_value(input["accepted"]["app_ids"].clone()).unwrap(),
        environments: serde_json::from_value(input["accepted"]["environments"].clone()).unwrap(),
    };
    verify_registration(
        &DeviceBundle::decode(&bytes_field(input, "device_bundle_hex")).unwrap(),
        &DeviceManifest::decode(&bytes_field(input, "device_manifest_hex")).unwrap(),
        input["manifest_ledger_close_unix_seconds"]
            .as_u64()
            .unwrap(),
        &accepted,
    )
}

fn verified_json(cred: &VerifiedCredential) -> Json {
    let public_key = match cred.public_key() {
        CredentialPublicKey::AppleP256(key) => key.to_sec1_point(false).as_bytes().to_vec(),
        CredentialPublicKey::DevEd25519(key) => key.as_bytes().to_vec(),
    };
    credential_json(
        class_name(cred.class()),
        cred.env(),
        cred.app_id(),
        cred.key_id(),
        &public_key,
    )
}

#[test]
fn real_device_registration_and_assertions_share_key() {
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    let d = DeviceFixture::load();
    let device = d.bundle();
    let cred = verify_registration(
        &device,
        &manifest(&device),
        d.verify_at_unix_seconds,
        &AcceptedAttestation {
            app_ids: vec![d.app_id.clone()],
            environments: vec![AttestationEnv::Development],
        },
    )
    .unwrap();
    assert_eq!(cred.class(), AttestationClass::AppleDevelopment);
    assert_eq!(cred.key_id(), &device.key_id);
    let CredentialPublicKey::AppleP256(key) = cred.public_key() else {
        panic!("verified development credential must carry a P-256 key");
    };
    let [first, second] = d.assertions.as_slice() else {
        panic!("device fixture must hold two assertions");
    };
    for (assertion, counter) in [(first, 1), (second, 2)] {
        assert_eq!(assertion.counter, counter);
        let assertion_bytes = unhex(&assertion.assertion_cbor_hex);
        let client_data = unhex(&assertion.client_data_hex);
        let core299: &[u8; 299] = client_data
            .strip_prefix(b"GALAXY/ASSERT/v1\0")
            .unwrap()
            .try_into()
            .unwrap();
        let client_data_hash = assertion_client_data_hash(core299);
        assert_eq!(client_data_hash, sha256(&[&client_data]));
        assert_eq!(
            apple::verify_assertion(&assertion_bytes, cred.app_id(), &client_data_hash, key),
            Ok(counter)
        );
        // The probe signed placeholder bytes, not a publishable CAPTURE core.
        assert_eq!(
            verify_capture_assertion(&cred, core299, &assertion_bytes),
            Err(AttestError::CaptureCore)
        );
        let mut decoded = decode(&assertion_bytes);
        let auth = field(&mut decoded, "authenticatorData")
            .as_bytes()
            .unwrap()
            .clone();
        let signature =
            Signature::from_der(field(&mut decoded, "signature").as_bytes().unwrap()).unwrap();
        let nonce = sha256(&[&auth, &client_data_hash]);
        assert!(
            key.verify_prehash(&nonce, &signature).is_err(),
            "nonce is not the ECDSA digest"
        );
        assert!(key.verify_prehash(&sha256(&[&nonce]), &signature).is_ok());
    }
}

fn run(check: &str, input: &Json) -> Result<Json, AttestError> {
    let text = |k: &str| input[k].as_str().unwrap();
    match check {
        "attestation" => apple::verify_attestation(
            &bytes_field(input, "attestation_cbor_hex"),
            text("app_id"),
            parse_env(text("environment")),
            &hex_field(input, "key_id_hex"),
            &hex_field(input, "client_data_hash_hex"),
            input["verify_at_unix_seconds"].as_u64().unwrap(),
        )
        .map(|key| json!({"public_key_sec1_hex": h(key.to_sec1_point(false).as_bytes())})),
        "strip_receipt" => strip_receipt(&bytes_field(input, "attestation_cbor_hex"))
            .map(|stripped| json!({"stripped_cbor_hex": h(&stripped)})),
        "assertion" => {
            let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&bytes_field(
                input,
                "public_key_sec1_hex",
            ))
            .unwrap();
            apple::verify_assertion(
                &bytes_field(input, "assertion_cbor_hex"),
                text("app_id"),
                &hex_field(input, "client_data_hash_hex"),
                &key,
            )
            .map(|counter| json!({"counter": counter}))
        }
        "counter" => {
            let history: Vec<_> = input["history"]
                .as_array()
                .unwrap()
                .iter()
                .map(counter_record)
                .collect();
            verify_counter(counter_record(&input["candidate"]), &history).map(|()| Json::Null)
        }
        "registration" => registration(input).map(|cred| verified_json(&cred)),
        "registration_and_capture" => {
            let cred = registration(input)?;
            let info = verify_capture_assertion(
                &cred,
                &bytes_field(input, "core299_hex"),
                &bytes_field(input, "assertion_hex"),
            )?;
            let mut result = verified_json(&cred);
            result["counter"] = json!(info.counter);
            Ok(result)
        }
        other => panic!("unknown check {other}"),
    }
}

#[test]
fn vectors_file_matches_generated_cases() {
    let mut generated = serde_json::to_string_pretty(&generate()).unwrap();
    generated.push('\n');
    if std::env::var_os("GALAXY_WRITE_VECTORS").is_some() {
        std::fs::write(CASES_PATH, &generated).unwrap();
    }
    let stored = std::fs::read_to_string(CASES_PATH).expect("cases.json missing; regenerate it");
    assert!(stored == generated, "cases.json is stale; regenerate it");
}

#[test]
fn vectors_file_cases_produce_expected_outcomes() {
    let stored: Json = serde_json::from_str(&std::fs::read_to_string(CASES_PATH).unwrap()).unwrap();
    let cases = stored["cases"].as_array().unwrap();
    assert!(cases.len() > 40);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let outcome = run(case["check"].as_str().unwrap(), &case["input"]);
        match (case["expected"].as_str().unwrap(), outcome) {
            ("ok", Ok(result)) => assert_eq!(result, case["result"], "{name}"),
            (code, Err(error)) => assert_eq!(code, error.code(), "{name}"),
            (expected, outcome) => panic!("{name}: expected {expected}, got {outcome:?}"),
        }
    }
}
