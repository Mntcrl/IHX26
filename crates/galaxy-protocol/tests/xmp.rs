use galaxy_protocol::xmp::{GalaxyXmp, XmpError, MAX_PACKET_LEN};

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
