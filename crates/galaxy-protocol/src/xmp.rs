//! Galaxy XMP lookup references (§4). The metadata is untrusted: verifiers use it only to
//! locate the CAPTURE/PROOF transactions and then check every binding on the ledger.
//!
//! Parsing is namespace-aware (the `galaxy` prefix is not significant, the namespace URI is),
//! accepts each Galaxy property either as an element with text content or as an attribute,
//! ignores properties of other namespaces and unknown Galaxy properties, and rejects
//! oversize packets, DOCTYPE declarations, duplicate or missing Galaxy properties, nested or
//! entity-escaped Galaxy values, unknown protocol/network/profile strings and malformed hex.
use crate::Hash32;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::NsReader;

pub const NAMESPACE: &str = "https://xrpl-galaxy.org/ns/galaxy/1.0/";
pub const PREFIX: &str = "galaxy";
pub const PROTOCOL: &str = "galaxy/v1";
pub const NETWORKS: [&str; 3] = ["xrpl-mainnet", "xrpl-testnet", "xrpl-devnet"];
pub const IMAGE_PROFILES: [&str; 2] = ["png-v1", "jpeg-v1"];
/// Packet size cap shared with the PNG/JPEG containers (§4).
pub const MAX_PACKET_LEN: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum XmpError {
    #[error("XMP packet exceeds {MAX_PACKET_LEN} bytes")]
    TooLarge,
    #[error("XMP packet is not UTF-8")]
    Utf8,
    #[error("malformed XMP: {0}")]
    Xml(String),
    #[error("XMP DOCTYPE declarations are not accepted")]
    DocType,
    #[error("duplicate Galaxy XMP property {0}")]
    Duplicate(&'static str),
    #[error("missing Galaxy XMP property {0}")]
    Missing(&'static str),
    #[error("invalid Galaxy XMP property {0}")]
    Invalid(&'static str),
}

impl From<quick_xml::Error> for XmpError {
    fn from(err: quick_xml::Error) -> Self {
        Self::Xml(err.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GalaxyXmp {
    pub protocol: String,
    pub network: String,
    pub image_profile: String,
    pub photo_id: Hash32,
    pub capture_id: Hash32,
    pub manifest_tx: Hash32,
    pub proof_tx: Hash32,
}

/// XMP local names in packet order.
const PROPERTIES: [&str; 7] = ["protocol", "network", "imageProfile", "photoId", "captureId", "manifestTx", "proofTx"];

impl GalaxyXmp {
    /// Serialize a complete XMP packet (`<?xpacket?>`-wrapped, UTF-8, no BOM outside the PI).
    /// Galaxy ids are written lowercase, XRPL transaction hashes uppercase (§2).
    pub fn to_packet(&self) -> Result<Vec<u8>, XmpError> {
        self.validate_strings()?;
        let values = [
            self.protocol.clone(),
            self.network.clone(),
            self.image_profile.clone(),
            hex::encode(self.photo_id),
            hex::encode(self.capture_id),
            hex::encode_upper(self.manifest_tx),
            hex::encode_upper(self.proof_tx),
        ];
        let mut out = String::with_capacity(1024);
        out.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
        out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
        out.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
        out.push_str(&format!("  <rdf:Description rdf:about=\"\" xmlns:{PREFIX}=\"{NAMESPACE}\">\n"));
        // Every value is validated ASCII without XML metacharacters, so no escaping is needed.
        for (name, value) in PROPERTIES.iter().zip(values) {
            out.push_str(&format!("   <{PREFIX}:{name}>{value}</{PREFIX}:{name}>\n"));
        }
        out.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
        Ok(out.into_bytes())
    }

    pub fn parse_packet(packet: &[u8]) -> Result<Self, XmpError> {
        if packet.len() > MAX_PACKET_LEN {
            return Err(XmpError::TooLarge);
        }
        let text = std::str::from_utf8(packet).map_err(|_| XmpError::Utf8)?;
        let mut reader = NsReader::from_str(text);
        let mut found: [Option<String>; 7] = Default::default();
        let mut depth = 0usize;
        // Index of the Galaxy property whose element text is being collected, and its depth.
        let mut open: Option<(usize, usize, String)> = None;
        loop {
            let (ns, event) = reader.read_resolved_event()?;
            let galaxy_ns = matches!(ns, ResolveResult::Bound(Namespace(uri)) if uri == NAMESPACE.as_bytes());
            match event {
                Event::Start(start) => {
                    if open.is_some() {
                        return Err(XmpError::Invalid("nested element in Galaxy property"));
                    }
                    depth += 1;
                    take_attributes(&reader, &start, &mut found)?;
                    if galaxy_ns {
                        if let Some(index) = property_index(start.local_name().as_ref()) {
                            open = Some((index, depth, String::new()));
                        }
                    }
                }
                Event::Empty(start) => {
                    if open.is_some() {
                        return Err(XmpError::Invalid("nested element in Galaxy property"));
                    }
                    take_attributes(&reader, &start, &mut found)?;
                    if galaxy_ns {
                        if let Some(index) = property_index(start.local_name().as_ref()) {
                            set(&mut found, index, String::new())?;
                        }
                    }
                }
                Event::End(_) => {
                    if let Some((index, at, value)) = open.take_if(|(_, at, _)| *at == depth) {
                        debug_assert_eq!(at, depth);
                        set(&mut found, index, value)?;
                    }
                    depth = depth.checked_sub(1).ok_or_else(|| XmpError::Xml("unmatched end tag".into()))?;
                }
                Event::Text(text) => {
                    if let Some((_, _, value)) = open.as_mut() {
                        value.push_str(&text.decode().map_err(|e| XmpError::Xml(e.to_string()))?);
                    }
                }
                Event::CData(_) | Event::GeneralRef(_) | Event::Comment(_) | Event::PI(_) if open.is_some() => {
                    return Err(XmpError::Invalid("non-text content in Galaxy property"));
                }
                Event::DocType(_) => return Err(XmpError::DocType),
                Event::CData(_) | Event::GeneralRef(_) | Event::Comment(_) | Event::PI(_) | Event::Decl(_) => {}
                Event::Eof => break,
            }
        }
        if depth != 0 || open.is_some() {
            return Err(XmpError::Xml("unclosed element".into()));
        }
        let [protocol, network, image_profile, photo_id, capture_id, manifest_tx, proof_tx] = found;
        let required = |value: Option<String>, name: &'static str| value.ok_or(XmpError::Missing(name));
        let xmp = Self {
            protocol: required(protocol, "protocol")?,
            network: required(network, "network")?,
            image_profile: required(image_profile, "imageProfile")?,
            photo_id: parse_hash(&required(photo_id, "photoId")?, "photoId")?,
            capture_id: parse_hash(&required(capture_id, "captureId")?, "captureId")?,
            manifest_tx: parse_hash(&required(manifest_tx, "manifestTx")?, "manifestTx")?,
            proof_tx: parse_hash(&required(proof_tx, "proofTx")?, "proofTx")?,
        };
        xmp.validate_strings()?;
        Ok(xmp)
    }

    fn validate_strings(&self) -> Result<(), XmpError> {
        if self.protocol != PROTOCOL {
            return Err(XmpError::Invalid("protocol"));
        }
        if !NETWORKS.contains(&self.network.as_str()) {
            return Err(XmpError::Invalid("network"));
        }
        if !IMAGE_PROFILES.contains(&self.image_profile.as_str()) {
            return Err(XmpError::Invalid("imageProfile"));
        }
        Ok(())
    }
}

fn property_index(local_name: &[u8]) -> Option<usize> {
    PROPERTIES.iter().position(|name| name.as_bytes() == local_name)
}

fn set(found: &mut [Option<String>; 7], index: usize, value: String) -> Result<(), XmpError> {
    if found[index].replace(value).is_some() {
        return Err(XmpError::Duplicate(PROPERTIES[index]));
    }
    Ok(())
}

fn take_attributes(
    reader: &NsReader<&[u8]>,
    start: &quick_xml::events::BytesStart<'_>,
    found: &mut [Option<String>; 7],
) -> Result<(), XmpError> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| XmpError::Xml(e.to_string()))?;
        let (ns, local) = reader.resolver().resolve_attribute(attribute.key);
        if !matches!(ns, ResolveResult::Bound(Namespace(uri)) if uri == NAMESPACE.as_bytes()) {
            continue;
        }
        let Some(index) = property_index(local.as_ref()) else { continue };
        if attribute.value.contains(&b'&') {
            return Err(XmpError::Invalid("entity in Galaxy property"));
        }
        let value = std::str::from_utf8(&attribute.value).map_err(|_| XmpError::Utf8)?;
        set(found, index, value.to_owned())?;
    }
    Ok(())
}

/// 64 hex digits, either case (§2: parsers are case-insensitive with exact length).
fn parse_hash(text: &str, name: &'static str) -> Result<Hash32, XmpError> {
    let mut out = [0u8; 32];
    if text.len() != 64 || hex::decode_to_slice(text, &mut out).is_err() {
        return Err(XmpError::Invalid(name));
    }
    Ok(out)
}
