use galaxy_protocol::wire::*;
use serde_json::Value;

use super::*;

type Check = fn(&[u8], bool) -> TestCaseResult;

fn check<T: PartialEq>(
    kind: &str,
    bytes: &[u8],
    valid: bool,
    decode: fn(&[u8]) -> Result<T, WireError>,
    encode: fn(&T) -> Vec<u8>,
) -> TestCaseResult {
    // Bundles have a 1 MiB encoded cap; leave headroom for the canonical JSON AST.
    let budget = if kind == "CircuitBundle" { 8 * MIB } else { MAX_BUNDLE_LEN };
    match bounded(kind, budget, || decode(bytes)) {
        Ok(value) => {
            let encoded = encode(&value);
            prop_assert!(decode(&encoded).is_ok_and(|decoded| decoded == value), "{kind}: model round-trip failed");
            prop_assert!(encoded == bytes, "{kind}: noncanonical bytes were accepted");
        }
        Err(_) => prop_assert!(!valid, "{kind}: valid bytes were rejected"),
    }
    Ok(())
}

macro_rules! decoder {
    ($name:ident, $ty:ty) => {
        (stringify!($name), |bytes, valid| {
            check(stringify!($ty), bytes, valid, <$ty>::decode, <$ty>::encode)
        })
    };
}

const DECODERS: [(&str, Check); 11] = [
    decoder!(CaptureCore, CaptureCore),
    decoder!(CaptureMemo, CaptureMemo),
    decoder!(ProofMemo, ProofMemo),
    decoder!(DeviceChunk, ChunkMemo),
    decoder!(CircuitChunk, ChunkMemo),
    decoder!(DeviceManifest, DeviceManifest),
    decoder!(DeviceBundle, DeviceBundle),
    decoder!(CircuitManifest, CircuitManifest),
    decoder!(CircuitBundle, CircuitBundle),
    decoder!(RevokeMemo, RevokeMemo),
    decoder!(NodeAnnouncement, NodeAnnouncement),
];

fn kind(row: &Value) -> &str {
    match row["memo_type"].as_str() {
        Some("galaxy/v1/device-chunk") => "DeviceChunk",
        Some("galaxy/v1/circuit-chunk") => "CircuitChunk",
        _ => row["type"].as_str().unwrap(),
    }
}

#[test]
fn memo_wire_noise_and_canonical_mutations() {
    let vectors: Value = serde_json::from_str(include_str!("../../../test-vectors/protocol/wire.json")).unwrap();
    let rows = vectors["cases"]["valid"].as_array().unwrap();
    for (kind_name, check) in DECODERS {
        let (row_type, hex_field) = if kind_name == "CaptureCore" {
            ("CaptureMemo", "core_hex")
        } else {
            (kind_name, "hex")
        };
        let fixtures: Vec<_> = rows.iter().filter(|row| kind(row) == row_type)
            .map(|row| hex::decode(row[hex_field].as_str().unwrap()).unwrap()).collect();
        assert!(!fixtures.is_empty(), "missing fixture: {kind_name}");
        for fixture in &fixtures {
            check(fixture, true).unwrap();
        }
        property(96, (noise(2 * KIB), any::<bool>()), |(mut bytes, framed)| {
            if framed {
                let prefix: &[u8] = match kind_name {
                    "DeviceBundle" => b"GXDB\x01",
                    "CircuitBundle" => b"GXCB\x01",
                    _ => b"\x01",
                };
                bytes.splice(..0, prefix.iter().copied());
            }
            check(&bytes, false)
        });
        property(128, (0..fixtures.len(), mutation()), |(fixture, (position, mask))| {
            let mut bytes = fixtures[fixture].clone();
            change(&mut bytes, position, mask);
            check(&bytes, false)
        });
    }
}

#[test]
fn device_and_circuit_bundle_size_caps() {
    let mut bytes = vec![0; MAX_BUNDLE_LEN + 1];
    bytes[..5].copy_from_slice(b"GXDB\x01");
    assert!(bounded("DeviceBundle (oversize)", 0, || DeviceBundle::decode(&bytes)).is_err());
    bytes[..5].copy_from_slice(b"GXCB\x01");
    assert!(bounded("CircuitBundle (oversize)", 0, || CircuitBundle::decode(&bytes)).is_err());
}
