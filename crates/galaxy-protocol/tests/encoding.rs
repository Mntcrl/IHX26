use galaxy_protocol::{canonical_json, policy::Policy};
use serde_json::{json, Value};

#[test]
fn policy_matches_canonical_reference() {
    let initial = include_bytes!("../../../policies/galaxy-pc-v1m.json");
    let policy: Policy = serde_json::from_slice(initial).unwrap();
    let vectors: Value = serde_json::from_str(include_str!("../../../test-vectors/protocol/policy.json")).unwrap();
    let case = &vectors["cases"][0];
    let bytes = policy.canonical_json().unwrap();
    assert_eq!(hex::encode(&bytes), case["canonical_hex"].as_str().unwrap());
    assert_eq!(hex::encode(policy.id().unwrap()), case["id"].as_str().unwrap());
    assert!(canonical_json::validate(&bytes).is_ok());
    assert!(canonical_json::validate(initial).is_err());
    assert_eq!(policy, serde_json::from_slice(&bytes).unwrap());
}

#[test]
fn canonical_json_rejects_alternate_spellings() {
    let value = json!({"z":{"b":[true,null,9007199254740991i64],"a":"\n\\\""}, "A":-9007199254740991i64});
    let expected = br#"{"A":-9007199254740991,"z":{"a":"\u000a\\\"","b":[true,null,9007199254740991]}}"#;
    assert_eq!(canonical_json::to_vec(&value).unwrap(), expected);
    canonical_json::validate(expected).unwrap();
    for bad in ["-0", "01", "1.0", "1e1", "9007199254740992", "-9007199254740992", "\"é\"", "\"\\u00e9\"", "\"\\n\"", "\"\\u000A\"", "\"\\u0061\"", "{} ", "{\"z\":1,\"a\":1}", "{\"a\":1,\"a\":1}", "[] []"] {
        assert!(canonical_json::validate(bad.as_bytes()).is_err(), "{bad}");
    }
    for length in 0..expected.len() {
        assert!(canonical_json::validate(&expected[..length]).is_err());
    }
}

#[test]
fn impossible_policy_contract_is_rejected() {
    let policy: Policy = serde_json::from_slice(include_bytes!("../../../policies/galaxy-pc-v1m.json")).unwrap();
    let mut bad = policy.clone();
    bad.lidar.d_max_mm = 8192;
    assert!(bad.id().is_err());
    let mut bad = policy.clone();
    bad.lidar.d_min_mm = 0;
    assert!(bad.validate().is_err());
    let mut bad = policy.clone();
    bad.lidar.sep_log2 = -1;
    assert!(bad.validate().is_err());
    let mut bad = policy.clone();
    bad.motion.m_cd = [1, 0, 0, 1, 0, 0, 1, 0, 0];
    assert!(bad.validate().is_err());
    let mut bad = policy;
    bad.structural.slots = 17;
    assert!(bad.validate().is_err());
}

#[test]
fn canonical_json_and_policy_decoders_never_panic() {
    let policy: Policy = serde_json::from_slice(include_bytes!("../../../policies/galaxy-pc-v1m.json")).unwrap();
    let canonical = policy.canonical_json().unwrap();
    let mut state = 0x243f_6a88_85a3_08d3u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for len in 0..canonical.len() {
        assert!(canonical_json::validate(&canonical[..len]).is_err(), "prefix {len} accepted");
    }
    for _ in 0..2000 {
        let mut m = canonical.clone();
        let i = (next() as usize) % m.len();
        m[i] = next() as u8;
        // Any accepted mutation must already be in canonical form, and policies parse without panicking.
        if canonical_json::validate(&m).is_ok() {
            let value: Value = serde_json::from_slice(&m).unwrap();
            assert_eq!(canonical_json::to_vec(&value).unwrap(), m);
        }
        if let Ok(p) = serde_json::from_slice::<Policy>(&m) {
            let _ = p.validate();
            let _ = p.id();
        }
    }
}
