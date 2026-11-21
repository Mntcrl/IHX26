use ciborium::Value as Cbor;
use galaxy_attestation::{assertion_counter_hint, strip_receipt, verify_capture_assertion, verify_registration, AcceptedAttestation, AttestationEnv};
use galaxy_protocol::wire::{CaptureCore, DeviceBundle, DeviceManifest};
use serde_json::Value;

use super::*;

const ATTESTATION_CAP: usize = MIB;
const ASSERTION_CAP: usize = 600;
const ATTESTATION_HEAP: usize = 16 * ATTESTATION_CAP;
const ASSERTION_HEAP: usize = 64 * KIB;

fn fixtures() -> (DeviceBundle, DeviceManifest, u64, AcceptedAttestation, Vec<u8>, u32) {
    let vectors: Value = serde_json::from_str(include_str!("../../../test-vectors/attestation/cases.json")).unwrap();
    let input = &vectors["cases"].as_array().unwrap().iter().find(|case| {
        case["name"] == "registration_device_development_valid" && case["expected"] == "ok"
    }).unwrap()["input"];
    let bundle = DeviceBundle::decode(&hex::decode(input["device_bundle_hex"].as_str().unwrap()).unwrap()).unwrap();
    let manifest = DeviceManifest::decode(&hex::decode(input["device_manifest_hex"].as_str().unwrap()).unwrap()).unwrap();
    let time = input["manifest_ledger_close_unix_seconds"].as_u64().unwrap();
    let accepted = AcceptedAttestation { app_ids: vec![bundle.app_id.clone()], environments: vec![AttestationEnv::Development] };
    let device: Value = serde_json::from_str(include_str!("../../../test-vectors/attestation/device-development.json")).unwrap();
    let assertion = &device["assertions"][0];
    let assertion_bytes = hex::decode(assertion["assertion_cbor_hex"].as_str().unwrap()).unwrap();
    let counter = assertion["counter"].as_u64().unwrap().try_into().unwrap();
    (bundle, manifest, time, accepted, assertion_bytes, counter)
}

fn field<'a>(value: &'a mut Cbor, name: &str) -> &'a mut Cbor {
    &mut value.as_map_mut().unwrap().iter_mut().find(|(key, _)| key.as_text() == Some(name)).unwrap().1
}

fn encode(value: &Cbor) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn certificates(packet: &mut Cbor) -> &mut Vec<Cbor> {
    field(field(packet, "attStmt"), "x5c").as_array_mut().unwrap()
}

#[test]
fn cbor_and_der_arbitrary_bytes_with_size_caps() {
    let (mut bundle, manifest, time, accepted, _, _) = fixtures();
    let credential = verify_registration(&bundle, &manifest, time, &accepted).unwrap();
    let attestation: Cbor = ciborium::from_reader(bundle.attestation.as_slice()).unwrap();
    let vectors: Value = serde_json::from_str(include_str!("../../../test-vectors/protocol/wire.json")).unwrap();
    let row = vectors["cases"]["valid"].as_array().unwrap().iter().find(|row| row["type"] == "CaptureMemo").unwrap();
    let mut core = CaptureCore::decode(&hex::decode(row["core_hex"].as_str().unwrap()).unwrap()).unwrap();
    core.network = bundle.network;
    core.account_id = bundle.account_id;
    let core = core.encode();

    property(128, (noise(8 * KIB), any::<bool>()), |(bytes, framed)| {
        let input = if framed { [b"\xa2" as &[u8], &bytes].concat() } else { bytes };
        let _ = bounded("App Attest CBOR", ATTESTATION_HEAP, || strip_receipt(&input));
        let _ = bounded("assertion CBOR/DER", ASSERTION_HEAP, || assertion_counter_hint(&input));
        let _ = bounded("signed assertion CBOR/DER", ASSERTION_HEAP, || verify_capture_assertion(&credential, &core, &input));
        let mut received = bundle.clone();
        received.attestation = input;
        let _ = bounded("registration CBOR", ATTESTATION_HEAP, || verify_registration(&received, &manifest, time, &accepted));
        Ok(())
    });

    property(96, (noise(2 * KIB), 0..2usize), |(bytes, certificate)| {
        let mut packet = attestation.clone();
        certificates(&mut packet)[certificate] = Cbor::Bytes(bytes);
        let mut bundle = bundle.clone();
        bundle.attestation = encode(&packet);
        let _ = bounded("certificate DER", ATTESTATION_HEAP, || verify_registration(&bundle, &manifest, time, &accepted));
        Ok(())
    });
    bundle.attestation = vec![0; ATTESTATION_CAP + 1];
    assert!(bounded("attestation oversize", 0, || strip_receipt(&bundle.attestation)).is_err());
    assert!(bounded("registration oversize", KIB, || verify_registration(&bundle, &manifest, time, &accepted)).is_err());
    let assertion = vec![0; ASSERTION_CAP + 1];
    assert!(bounded("assertion oversize", 0, || assertion_counter_hint(&assertion)).is_err());
}

#[test]
fn receipt_free_attestation_and_der_mutations() {
    let (bundle, manifest, time, accepted, _, _) = fixtures();
    let packet: Cbor = ciborium::from_reader(bundle.attestation.as_slice()).unwrap();
    let credential = bounded("App Attest registration", ATTESTATION_HEAP, || verify_registration(&bundle, &manifest, time, &accepted)).unwrap();
    let encoded = strip_receipt(&bundle.attestation).unwrap();
    assert!(encoded == bundle.attestation);

    property(128, (any::<bool>(), mutation()), |(der, (position, mask))| {
        let mut mutated = bundle.clone();
        if der {
            let mut cbor = packet.clone();
            let Cbor::Bytes(cert) = &mut certificates(&mut cbor)[position % 2] else { unreachable!() };
            change(cert, position / 2, mask);
            mutated.attestation = encode(&cbor);
        } else {
            change(&mut mutated.attestation, position, mask);
        }
        if let Ok(stripped) = bounded("mutated attestation CBOR", ATTESTATION_HEAP, || strip_receipt(&mutated.attestation)) {
            // Receipt removal accepts non-deterministic CBOR and does not authenticate DER.
            prop_assert!(strip_receipt(&stripped).is_ok_and(|again| again == stripped));
        }
        if let Ok(result) = bounded("mutated registration", ATTESTATION_HEAP, || verify_registration(&mutated, &manifest, time, &accepted)) {
            prop_assert_eq!(result.key_id(), credential.key_id());
            prop_assert!(!der, "modified certificate accepted");
            prop_assert!(strip_receipt(&mutated.attestation).is_ok_and(|v| v == encoded));
        }
        Ok(())
    });
}

#[test]
fn assertion_cbor_round_trips_and_der_mutations() {
    let (_, _, _, _, assertion, counter) = fixtures();
    assert_eq!(assertion_counter_hint(&assertion), Ok(counter));
    let packet: Cbor = ciborium::from_reader(assertion.as_slice()).unwrap();
    property(192, (1u32..=u32::MAX, any::<bool>(), mutation()), |(counter, der, (position, mask))| {
        let mut cbor = packet.clone();
        let Cbor::Bytes(auth) = field(&mut cbor, "authenticatorData") else { unreachable!() };
        auth[33..37].copy_from_slice(&counter.to_be_bytes());
        let mut encoded = encode(&cbor);
        prop_assert_eq!(bounded("generated assertion", ASSERTION_HEAP, || assertion_counter_hint(&encoded)), Ok(counter));
        if der {
            let Cbor::Bytes(signature) = field(&mut cbor, "signature") else { unreachable!() };
            change(signature, position, mask);
            encoded = encode(&cbor);
        } else {
            change(&mut encoded, position, mask);
        }
        if let Ok(counter) = bounded("mutated assertion", ASSERTION_HEAP, || assertion_counter_hint(&encoded)) {
            // No assertion encoder is exposed. Check that a lossless CBOR model round-trip
            // preserves the structural hint; this does not imply signature verification.
            let parsed: Cbor = ciborium::from_reader(encoded.as_slice()).unwrap();
            prop_assert_eq!(assertion_counter_hint(&encode(&parsed)), Ok(counter));
        }
        Ok(())
    });
}
