use galaxy_protocol::{network::NetworkId, Hash32};
use galaxy_verifier::{parse_lookup_response, LookupError, LookupPage, Refs, MAX_LOOKUP_RESPONSE_BYTES};
use serde_json::{json, Value};

use super::*;

fn decode(input: &[u8], photo: &Hash32, network: u32, limit: usize) -> Result<LookupPage, LookupError> {
    bounded("lookup response", 16 * KIB + 16 * input.len().min(MAX_LOOKUP_RESPONSE_BYTES), || {
        parse_lookup_response(input, photo, network, limit)
    })
}

#[test]
fn lookup_arbitrary_bodies_and_response_cap() {
    property(256, (noise(4 * KIB), any::<bool>()), |(bytes, framed)| {
        let input = if framed { [b"{\"captures\":[" as &[u8], &bytes, b"]}"].concat() } else { bytes };
        let _ = decode(&input, &[0; 32], 1, 50);
        Ok(())
    });
    let mut valid = json!({"protocol": "galaxy-node/v1", "network": "xrpl-testnet", "network_id": 1,
        "software_version": "1.0.0", "photo_id": "00".repeat(32), "indexed_from": null,
        "indexed_through": null, "indexed_ranges": [], "captures": [], "incomplete": true,
        "next_cursor": null, "extension": ""});
    let overhead = serde_json::to_vec(&valid).unwrap().len();
    valid["extension"] = json!(" ".repeat(MAX_LOOKUP_RESPONSE_BYTES - overhead));
    let mut at_cap = serde_json::to_vec(&valid).unwrap();
    assert_eq!(at_cap.len(), MAX_LOOKUP_RESPONSE_BYTES);
    assert!(decode(&at_cap, &[0; 32], 1, 1).is_ok());
    at_cap.push(b' '); // valid JSON but outside the transport cap
    assert!(bounded("oversized lookup", KIB, || parse_lookup_response(&at_cap, &[0; 32], 1, 1)).is_err());
}

#[test]
fn lookup_valid_pages_and_single_byte_mutations() {
    let hashes = any::<[u8; 32]>();
    property(192, (0u32..=2, any::<Hash32>(), vec((hashes, any::<Option<Hash32>>()), 1..=12), mutation()),
        |(network, photo, refs, (position, mask))| {
            let captures: Vec<_> = refs.iter().enumerate().map(|(i, (manifest, proof))| json!({
                "capture_id": format!("{i:064x}"), "manifest_tx": hex::encode_upper(manifest),
                "proof_tx": proof.map(hex::encode_upper), "ledger_index": 100 + i,
            })).collect();
            let document = json!({"protocol": "galaxy-node/v1", "network": NetworkId::from_u32(network).unwrap().name(),
                "network_id": network, "software_version": "1.2.3-rc.1", "photo_id": hex::encode(photo),
                "indexed_from": 100, "indexed_through": 111, "indexed_ranges": [{"from": 100, "through": 111}],
                "captures": captures, "incomplete": false, "next_cursor": "cursor-v1"});
            let mut body = serde_json::to_vec(&document).unwrap();
            let page = decode(&body, &photo, network, refs.len()).unwrap();
            prop_assert_eq!(page.result.candidates, refs.iter().map(|&(manifest_tx, proof_tx)| Refs {manifest_tx, proof_tx}).collect::<Vec<_>>());
            prop_assert_eq!(page.next_cursor.as_deref(), Some("cursor-v1"));
            prop_assert!(!page.result.incomplete);
            prop_assert_eq!((page.result.ranges[0].min, page.result.ranges[0].max), (100, 111));

            change(&mut body, position, mask);
            if let Ok(mutated) = decode(&body, &photo, network, refs.len()) {
                // The wire form is ordinary JSON (not canonical) and permits unknown fields.
                let normalized: Value = serde_json::from_slice(&body).unwrap();
                let normalized = serde_json::to_vec(&normalized).unwrap();
                let reparsed = decode(&normalized, &photo, network, refs.len()).unwrap();
                prop_assert_eq!(reparsed.result.candidates, mutated.result.candidates);
                prop_assert_eq!(reparsed.next_cursor, mutated.next_cursor);
                prop_assert_eq!(reparsed.result.incomplete, mutated.result.incomplete);
                prop_assert!(reparsed.result.ranges.iter().map(|r| (r.min, r.max)).eq(mutated.result.ranges.iter().map(|r| (r.min, r.max))));
            }
            Ok(())
        },
    );
}
