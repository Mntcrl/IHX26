use galaxy_protocol::xmp::{GalaxyInfo, GalaxyXmp, XmpError, INFO_NAMESPACE, MAX_PACKET_LEN};
use proptest::prelude::*;

fn sample() -> GalaxyXmp {
    GalaxyXmp {
        protocol: "galaxy/v1".into(),
        network: "xrpl-testnet".into(),
        image_profile: "png-v1".into(),
        photo_id: [0xab; 32],
        capture_id: [0x01; 32],
        manifest_tx: [0xcd; 32],
        proof_tx: [0xef; 32],
    }
}

fn packet_with(body: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:g=\"https://xrpl-galaxy.org/ns/galaxy/1.0/\" xmlns:o=\"urn:other\">{body}\
         </rdf:Description></rdf:RDF></x:xmpmeta>"
    )
}

const PHOTO: &str = "abababababababababababababababababababababababababababababababab";
const CAPTURE: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const MANIFEST: &str = "CDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCD";
const PROOF: &str = "efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef";

fn elements(skip: Option<&str>) -> String {
    [
        ("protocol", "galaxy/v1"),
        ("network", "xrpl-testnet"),
        ("imageProfile", "png-v1"),
        ("photoId", PHOTO),
        ("captureId", CAPTURE),
        ("manifestTx", MANIFEST),
        ("proofTx", PROOF),
    ]
    .iter()
    .filter(|(name, _)| Some(*name) != skip)
    .map(|(name, value)| format!("<g:{name}>{value}</g:{name}>"))
    .collect()
}

#[test]
fn packet_round_trip_and_case() {
    let packet = sample().to_packet().unwrap();
    let text = std::str::from_utf8(&packet).unwrap();
    assert!(text.contains(&format!("<galaxy:photoId>{PHOTO}</galaxy:photoId>")));
    assert!(text.contains(&format!("<galaxy:manifestTx>{MANIFEST}</galaxy:manifestTx>")));
    assert!(text.contains("<galaxy:proofTx>EFEF"));
    assert_eq!(GalaxyXmp::parse_packet(&packet).unwrap(), sample());
}

#[test]
fn accepts_other_prefix_attributes_unknown_properties_and_mixed_case() {
    let body = format!("<o:creator>someone</o:creator><g:futureField>x</g:futureField>{}", elements(None));
    assert_eq!(GalaxyXmp::parse_packet(packet_with(&body).as_bytes()).unwrap(), sample());

    let attrs = format!(
        "<rdf:Description xmlns:galaxy=\"https://xrpl-galaxy.org/ns/galaxy/1.0/\" galaxy:protocol=\"galaxy/v1\" \
         galaxy:network=\"xrpl-testnet\" galaxy:imageProfile=\"png-v1\" galaxy:photoId=\"{}\" galaxy:captureId=\"{CAPTURE}\" \
         galaxy:manifestTx=\"{}\" galaxy:proofTx=\"{PROOF}\"/>",
        PHOTO.to_uppercase(),
        MANIFEST.to_lowercase()
    );
    assert_eq!(GalaxyXmp::parse_packet(packet_with(&attrs).as_bytes()).unwrap(), sample());
}

#[test]
fn rejects_malformed_packets() {
    let reject = |packet: String| GalaxyXmp::parse_packet(packet.as_bytes()).unwrap_err();
    for name in ["protocol", "network", "imageProfile", "photoId", "captureId", "manifestTx", "proofTx"] {
        assert!(matches!(reject(packet_with(&elements(Some(name)))), XmpError::Missing(_)), "{name}");
    }
    let dup = format!("{}<g:photoId>{PHOTO}</g:photoId>", elements(None));
    assert!(matches!(reject(packet_with(&dup)), XmpError::Duplicate("photoId")));
    let dup_attr = packet_with(&format!("<o:x g:proofTx=\"{PROOF}\"/>{}", elements(None)));
    assert!(matches!(reject(dup_attr), XmpError::Duplicate("proofTx")));
    for (from, to) in [
        (PHOTO, &PHOTO[..62]),
        (PHOTO, &format!("{}zz", &PHOTO[..62])[..]),
        (PHOTO, &format!(" {PHOTO}")[..]),
        ("galaxy/v1", "galaxy/v2"),
        ("xrpl-testnet", "xrpl-other"),
        ("png-v1", "png-v2"),
    ] {
        assert!(matches!(reject(packet_with(&elements(None).replacen(from, to, 1))), XmpError::Invalid(_)), "{to}");
    }
    let nested = elements(None).replace(&format!(">{PROOF}<"), &format!("><g:x>{PROOF}</g:x><"));
    assert!(matches!(reject(packet_with(&nested)), XmpError::Invalid(_)));
    let entity = elements(None).replace("galaxy/v1", "galaxy&#47;v1");
    assert!(matches!(reject(packet_with(&entity)), XmpError::Invalid(_)));
    let doctype = format!("<!DOCTYPE x [<!ENTITY e \"v\">]>{}", packet_with(&elements(None)));
    assert!(matches!(reject(doctype), XmpError::DocType));
    let unclosed = packet_with(&elements(None)).replace("</x:xmpmeta>", "");
    assert!(GalaxyXmp::parse_packet(unclosed.as_bytes()).is_err());
    assert!(matches!(GalaxyXmp::parse_packet(&[0xff, 0xfe]), Err(XmpError::Utf8)));
    let big = packet_with(&format!("{}<o:pad>{}</o:pad>", elements(None), "a".repeat(MAX_PACKET_LEN)));
    assert!(matches!(reject(big), XmpError::TooLarge));
}

#[test]
fn to_packet_rejects_unknown_strings() {
    let mut xmp = sample();
    xmp.network = "xrpl-testnet\"><x".into();
    assert!(xmp.to_packet().is_err());
}

#[test]
fn truncated_and_mutated_packets_never_panic() {
    let packet = sample().to_packet().unwrap();
    for len in 0..packet.len() {
        let _ = GalaxyXmp::parse_packet(&packet[..len]);
    }
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..4000 {
        let mut bytes = packet.clone();
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let at = (state % bytes.len() as u64) as usize;
            bytes[at] = (state >> 32) as u8;
        }
        let _ = GalaxyXmp::parse_packet(&bytes);
    }
}

fn export_info() -> GalaxyInfo<'static> {
    GalaxyInfo {
        account: Some("rrrrrrrrrrrrrrrrrrrrrhoLvTp"),
        registration_tx: Some(MANIFEST),
        capture_ledger: Some(24),
        selection_ledger: Some(29),
        proof_ledger: Some(30),
        capture_time: Some("2026-11-21T10:57:18Z"),
        claimed_time: Some("2026-11-21T10:57:17.123Z"),
        circuit_id: Some(PHOTO),
        policy_id: Some(PROOF),
        policy_name: Some("Galaxy <v1> & public"),
        setup_ceremony: Some("development-single-party"),
        attestation_environment: Some("development"),
        capture_tx_url: "https://example.org/transactions?kind=capture&network=testnet",
        proof_tx_url: "https://example.org/transactions?kind=proof&network=testnet",
        app_version: "2.0.4",
    }
}

#[test]
fn informational_packet_vector_keeps_lookup_references() {
    let expected = include_str!("../../../test-vectors/protocol/xmp-info.xml");
    let packet = sample().to_packet_with_info(&export_info()).unwrap();
    assert_eq!(std::str::from_utf8(&packet).unwrap(), expected.trim_end());
    assert_eq!(GalaxyXmp::parse_packet(&packet).unwrap(), sample());
}

#[test]
fn rejects_invalid_info_and_enforces_packet_cap() {
    let too_long = "<".repeat(MAX_PACKET_LEN / 3);
    for info in [
        GalaxyInfo { registration_tx: Some(PROOF), ..export_info() },
        GalaxyInfo { circuit_id: Some(MANIFEST), ..export_info() },
        GalaxyInfo { policy_id: Some("abcd"), ..export_info() },
        GalaxyInfo { capture_time: Some("2026-02-30T00:00:00Z"), ..export_info() },
        GalaxyInfo { claimed_time: Some("2026-01-01T00:00:00+02:00"), ..export_info() },
        GalaxyInfo { attestation_environment: Some("other"), ..export_info() },
        GalaxyInfo { capture_tx_url: "http://example.org/transactions", ..export_info() },
        GalaxyInfo { app_version: "bad\0version", ..export_info() },
    ] {
        assert!(matches!(sample().to_packet_with_info(&info), Err(XmpError::Invalid(_))));
    }
    let oversized = GalaxyInfo { policy_name: Some(&too_long), ..export_info() };
    assert!(matches!(sample().to_packet_with_info(&oversized), Err(XmpError::TooLarge)));
    let only_known = GalaxyInfo { capture_time: None, proof_ledger: None, ..export_info() };
    let packet = String::from_utf8(sample().to_packet_with_info(&only_known).unwrap()).unwrap();
    assert!(!packet.contains("CreateDate"));
    assert!(!packet.contains("proofLedger"));
}

proptest! {
    #[test]
    fn informational_markup_cannot_change_lookup_refs(
        field in "[A-Za-z][a-z0-9]{0,20}", text in any::<String>(),
        conflicting_hash in "[A-Fa-f0-9]{64}", depth in 0..8usize,
    ) {
        let text: String = text.chars().filter(|c| !c.is_control() && !matches!(c, '\u{fffe}' | '\u{ffff}')).collect();
        let escaped = quick_xml::escape::escape(&text);
        let opens = "<extra>".repeat(depth);
        let closes = "</extra>".repeat(depth);
        let body = format!(
            "<i:{field} xmlns:i=\"{INFO_NAMESPACE}\" g:photoId=\"{conflicting_hash}\">\
             {opens}{escaped}<g:proofTx>{conflicting_hash}</g:proofTx>{closes}</i:{field}>\
             <i:photoId xmlns:i=\"{INFO_NAMESPACE}\" g:captureId=\"{conflicting_hash}\"/>{}",
            elements(None),
        );
        let packet = packet_with(&body);
        prop_assert_eq!(GalaxyXmp::parse_packet(packet.as_bytes()).unwrap(), sample());
    }
}
