use galaxy_protocol::{ids, wire::*};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../../test-vectors/protocol/wire.json")).unwrap()
}

fn hexv(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

/// Strict decode followed by re-encode; `Ok` carries the re-encoded bytes.
fn roundtrip(kind: &str, bytes: &[u8]) -> Result<Vec<u8>, WireError> {
    Ok(match kind {
        "CaptureCore" => CaptureCore::decode(bytes)?.encode(),
        "CaptureMemo" => CaptureMemo::decode(bytes)?.encode(),
        "ProofMemo" => ProofMemo::decode(bytes)?.encode(),
        "ChunkMemo" => ChunkMemo::decode(bytes)?.encode(),
        "DeviceManifest" => DeviceManifest::decode(bytes)?.encode(),
        "DeviceBundle" => DeviceBundle::decode(bytes)?.encode(),
        "CircuitManifest" => CircuitManifest::decode(bytes)?.encode(),
        "CircuitBundle" => CircuitBundle::decode(bytes)?.encode(),
        "RevokeMemo" => RevokeMemo::decode(bytes)?.encode(),
        "NodeAnnouncement" => NodeAnnouncement::decode(bytes)?.encode(),
        other => panic!("unknown vector type {other}"),
    })
}

const KINDS: [&str; 10] = [
    "CaptureCore", "CaptureMemo", "ProofMemo", "ChunkMemo", "DeviceManifest", "DeviceBundle",
    "CircuitManifest", "CircuitBundle", "RevokeMemo", "NodeAnnouncement",
];

#[test]
fn valid_vectors_round_trip_exactly() {
    let v = vectors();
    let valid = v["cases"]["valid"].as_array().unwrap();
    assert!(valid.len() >= 13);
    for case in valid {
        let kind = case["type"].as_str().unwrap();
        let bytes = hexv(&case["hex"]);
        let encoded = roundtrip(kind, &bytes).unwrap_or_else(|e| panic!("{}: {e}", case["description"]));
        assert_eq!(encoded, bytes, "{}", case["description"]);
        if let Some(memo_type) = case["memo_type"].as_str() {
            let memo = MemoKind::from_memo_type(memo_type).unwrap();
            assert_eq!(memo.as_str(), memo_type);
        }
        if kind == "CaptureMemo" {
            let core = hexv(&case["core_hex"]);
            assert_eq!(core.len(), CAPTURE_CORE_LEN);
            assert_eq!(CaptureMemo::decode(&bytes).unwrap().core.encode(), core);
            assert_eq!(&bytes[..CAPTURE_CORE_LEN], core.as_slice());
        }
    }
}

#[test]
fn reject_vectors_fail_strict_decoding() {
    let v = vectors();
    let reject = v["cases"]["reject"].as_array().unwrap();
    assert!(reject.len() >= 40);
    for case in reject {
        let kind = case["type"].as_str().unwrap();
        assert!(roundtrip(kind, &hexv(&case["hex"])).is_err(), "{kind}: {} accepted", case["description"]);
    }
}

#[test]
fn memo_kind_parser_is_exact() {
    for kind in [
        MemoKind::Capture, MemoKind::Proof, MemoKind::DeviceChunk, MemoKind::CircuitChunk,
        MemoKind::Device, MemoKind::Circuit, MemoKind::Revoke, MemoKind::Node,
    ] {
        assert_eq!(MemoKind::from_memo_type(kind.as_str()), Some(kind));
        assert_eq!(kind.to_string().parse::<MemoKind>().unwrap(), kind);
    }
    for bad in ["", "galaxy/v1/", "galaxy/v2/capture", "Galaxy/v1/capture", "galaxy/v1/capture ", " galaxy/v1/proof", "galaxy/v1/capture\0", "galaxy/v1/chunk"] {
        assert_eq!(MemoKind::from_memo_type(bad), None, "{bad:?}");
    }
}

#[test]
fn ids_match_node_vectors() {
    let v: Value = serde_json::from_str(include_str!("../../../test-vectors/protocol/ids.json")).unwrap();
    let cases = v["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    let h32 = |v: &Value| -> [u8; 32] { hexv(v).try_into().unwrap() };
    let s = |v: &Value| v.as_str().unwrap().as_bytes().to_vec();
    for c in cases {
        let p = &c["photo_id"];
        let w = p["w"].as_u64().unwrap() as u32;
        let h = p["h"].as_u64().unwrap() as u32;
        assert_eq!(ids::photo_id(w, h, &hexv(&p["rgb"])), h32(&p["id"]));

        let core = hexv(&c["capture_id"]["core"]);
        CaptureCore::decode(&core).unwrap();
        assert_eq!(ids::capture_id(&core), h32(&c["capture_id"]["id"]));

        let a = &c["assertion_client_data"];
        assert_eq!(ids::assertion_client_data(&hexv(&a["core"])), hexv(&a["client_data"]));
        assert_eq!(ids::assertion_client_data_hash(&hexv(&a["core"])), h32(&a["hash"]));

        let r = &c["registration_client_data_hash"];
        let account: [u8; 20] = hexv(&r["account_id"]).try_into().unwrap();
        let got = ids::registration_client_data_hash(
            r["network_id"].as_u64().unwrap() as u32,
            r["reg_ledger_index"].as_u64().unwrap() as u32,
            &h32(&r["reg_ledger_hash"]),
            &h32(&r["reg_nonce"]),
            &h32(&r["key_id"]),
            &account,
        );
        assert_eq!(got, h32(&r["hash"]));

        let d = &c["dev_assertion_message"];
        assert_eq!(ids::dev_assertion_message(&hexv(&d["core"])), hexv(&d["message"]));
        assert_eq!(ids::dev_key_id(&h32(&c["dev_key_id"]["public_key"])), h32(&c["dev_key_id"]["id"]));
        assert_eq!(ids::bundle_id(&hexv(&c["bundle_id"]["bundle"])), h32(&c["bundle_id"]["id"]));
        assert_eq!(ids::policy_id(&s(&c["policy_id"]["json"])).unwrap(), h32(&c["policy_id"]["id"]));
        assert_eq!(ids::circuit_id(&s(&c["circuit_id"]["json"])).unwrap(), h32(&c["circuit_id"]["id"]));
        assert_eq!(ids::vk_digest(&hexv(&c["vk_digest"]["vk"])), h32(&c["vk_digest"]["digest"]));
        let n = &c["node_descriptor_signature_message"];
        assert_eq!(ids::node_descriptor_signature_message(&s(&n["json"])).unwrap(), hexv(&n["message"]));

        // Non-canonical JSON must not hash to an id.
        let mut spaced = s(&c["policy_id"]["json"]);
        spaced.insert(1, b' ');
        assert!(ids::policy_id(&spaced).is_err());
        assert!(ids::circuit_id(&spaced).is_err());
    }
}

/// xorshift64*: deterministic test-only byte stream.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[test]
fn decoders_reject_truncation_and_extension() {
    let v = vectors();
    for case in v["cases"]["valid"].as_array().unwrap() {
        let kind = case["type"].as_str().unwrap();
        let bytes = hexv(&case["hex"]);
        for len in 0..bytes.len() {
            assert!(roundtrip(kind, &bytes[..len]).is_err(), "{kind} truncated to {len} accepted");
        }
        for extra in [0u8, 1, 0xff] {
            let mut longer = bytes.clone();
            longer.push(extra);
            assert!(roundtrip(kind, &longer).is_err(), "{kind} with trailing byte accepted");
        }
    }
}

#[test]
fn decoders_never_panic_on_mutations_or_noise() {
    let v = vectors();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in v["cases"]["valid"].as_array().unwrap() {
        let kind = case["type"].as_str().unwrap();
        let bytes = hexv(&case["hex"]);
        for _ in 0..400 {
            let mut m = bytes.clone();
            for _ in 0..=(rng.next() % 4) {
                let i = (rng.next() as usize) % m.len();
                m[i] ^= (rng.next() as u8) | 1;
            }
            // Any accepted mutation must re-encode to exactly the input bytes.
            if let Ok(encoded) = roundtrip(kind, &m) {
                assert_eq!(encoded, m, "{kind}: non-canonical acceptance");
            }
        }
    }
    for kind in KINDS {
        for _ in 0..300 {
            let len = (rng.next() % 1200) as usize;
            let noise: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            if let Ok(encoded) = roundtrip(kind, &noise) {
                assert_eq!(encoded, noise);
            }
        }
    }
}
