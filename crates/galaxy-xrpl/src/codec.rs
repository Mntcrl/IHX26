//! XRPL binary serialization for transactions and metadata. This is a wire codec,
//! not a transaction schema validator or a substitute for ledger validation.
//!
//! The field and enum tables are the unmodified `src/enums/definitions.json`
//! from `ripple-binary-codec@2.11.0` (npm, ISC). SHA-256:
//! `dbd5c60a3b0fffa6c88a71414c5a0e8225192c90161886fe552d70f17e8cf2ff`.

mod decimal;
#[cfg(test)]
mod tests;

use crate::{AccountId, Error, keys};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{collections::HashMap, sync::LazyLock};

const MAX_BLOB: usize = 4 * 1024 * 1024;
const MAX_VL: usize = 918_744;
const MAX_DEPTH: usize = 32;
const MAX_ARRAY_ITEMS: usize = 65_536;
const OBJECT_END: u16 = 0x0e01;
const ARRAY_END: u16 = 0x0f01;
const NO_ACCOUNT: AccountId = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
const BRIDGE_SIDES: [(&str, &str); 2] = [
    ("LockingChainDoor", "LockingChainIssue"),
    ("IssuingChainDoor", "IssuingChainIssue"),
];

type Result<T, E = Error> = std::result::Result<T, E>;

fn invalid(message: impl Into<String>) -> Error {
    Error::Codec(message.into())
}

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct RawDefinitions<'a> {
    #[serde(borrow)]
    types: HashMap<&'a str, i32>,
    #[serde(borrow)]
    fields: Vec<(&'a str, RawField<'a>)>,
    #[serde(borrow, rename = "TRANSACTION_TYPES")]
    transaction_types: HashMap<&'a str, i32>,
    #[serde(borrow, rename = "LEDGER_ENTRY_TYPES")]
    ledger_entry_types: HashMap<&'a str, i32>,
    #[serde(borrow, rename = "TRANSACTION_RESULTS")]
    transaction_results: HashMap<&'a str, i32>,
}

#[derive(Deserialize)]
struct RawField<'a> {
    nth: i32,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "isSerialized")]
    serialized: bool,
    #[serde(rename = "isSigningField")]
    signing: bool,
    #[serde(rename = "isVLEncoded")]
    vl: bool,
}

#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    kind: &'static str,
    ordinal: u16,
    vl: bool,
    signing: bool,
}

struct Codes {
    by_name: HashMap<&'static str, i32>,
    by_code: HashMap<i32, &'static str>,
}

impl Codes {
    fn new(by_name: HashMap<&'static str, i32>) -> Self {
        let by_code = by_name.iter().map(|(&name, &code)| (code, name)).collect();
        Self { by_name, by_code }
    }

    fn decode(&self, code: i32) -> Result<Value> {
        self.by_code.get(&code).map(|&name| Value::from(name)).ok_or_else(|| invalid("unknown XRPL enum code"))
    }

    fn encode(&self, value: &Value) -> Result<i32> {
        let code = match value {
            Value::String(name) => self.by_name.get(name.as_str()).copied().or_else(|| {
                let code: i32 = name.parse().ok()?;
                self.by_code.contains_key(&code).then_some(code)
            }),
            _ => value.as_i64().and_then(|code| i32::try_from(code).ok()).filter(|code| self.by_code.contains_key(code)),
        };
        code.ok_or_else(|| invalid("unknown XRPL enum value"))
    }
}

struct Definitions {
    by_name: HashMap<&'static str, Option<Field>>,
    by_ordinal: HashMap<u16, Field>,
    transaction_types: Codes,
    transaction_results: Codes,
    ledger_entry_types: Codes,
    permissions: Codes,
}

static DEFINITIONS: LazyLock<Definitions> = LazyLock::new(|| {
    let raw: RawDefinitions<'static> = serde_json::from_str(include_str!("../vendor/definitions.json"))
        .expect("valid pinned ripple-binary-codec definitions");
    let mut by_name = HashMap::with_capacity(raw.fields.len());
    let mut by_ordinal = HashMap::with_capacity(raw.fields.len());
    for (name, raw_field) in raw.fields {
        let kind = raw.types[raw_field.kind];
        let field = if raw_field.serialized && (1..=255).contains(&kind) && (1..=255).contains(&raw_field.nth) {
            let ordinal = ((kind as u16) << 8) | raw_field.nth as u16;
            let field = Field { name, kind: raw_field.kind, ordinal, vl: raw_field.vl, signing: raw_field.signing };
            assert!(by_ordinal.insert(ordinal, field).is_none(), "duplicate pinned field ordinal");
            Some(field)
        } else {
            None
        };
        assert!(by_name.insert(name, field).is_none(), "duplicate pinned field name");
    }
    // These are defined in XrplDefinitionsBase rather than in the upstream JSON.
    let mut permissions: HashMap<_, _> = raw.transaction_types.iter().map(|(&name, &code)| (name, code + 1)).collect();
    for (index, name) in [
        "TrustlineAuthorize", "TrustlineFreeze", "TrustlineUnfreeze", "AccountDomainSet",
        "AccountEmailHashSet", "AccountMessageKeySet", "AccountTransferRateSet", "AccountTickSizeSet",
        "PaymentMint", "PaymentBurn", "MPTokenIssuanceLock", "MPTokenIssuanceUnlock",
    ].into_iter().enumerate() {
        permissions.insert(name, 65_537 + index as i32);
    }
    Definitions {
        by_name,
        by_ordinal,
        transaction_types: Codes::new(raw.transaction_types),
        transaction_results: Codes::new(raw.transaction_results),
        ledger_entry_types: Codes::new(raw.ledger_entry_types),
        permissions: Codes::new(permissions),
    }
});

/// Decode one complete, unframed XRPL transaction or metadata object.
/// Rejects malformed fields, noncanonical object order, and incomplete containers.
/// The root object ends at EOF; use the transaction hash to authenticate its extent.
pub fn decode(blob: &[u8]) -> Result<Value> {
    if blob.len() > MAX_BLOB {
        return Err(invalid("XRPL blob exceeds size limit"));
    }
    Reader(blob).object(0, false)
}

/// Encode XRPL JSON into canonical binary field order. Known nonserialized
/// fields and lowercase RPC auxiliary keys are ignored, as in xrpl.js.
/// Account fields accept classic addresses or raw account IDs in hex.
pub fn encode(object: &Value) -> Result<Vec<u8>> {
    let mut writer = Writer(Vec::new());
    writer.object(object, 0, false)?;
    Ok(writer.0)
}

/// Encode with the `STX\0` prefix and only the root signing fields, as in xrpl.js.
pub fn encode_for_signing(object: &Value) -> Result<Vec<u8>> {
    let mut writer = Writer(b"STX\0".to_vec());
    writer.object(object, 0, true)?;
    Ok(writer.0)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(len).ok_or_else(|| invalid("truncated XRPL blob"))?;
        self.0 = tail;
        Ok(head)
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(*self.take(N)?.first_chunk::<N>().expect("exact read size"))
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn ordinal(&mut self) -> Result<u16> {
        let head = self.byte()?;
        let mut kind = head >> 4;
        let mut nth = head & 15;
        if kind == 0 {
            kind = self.byte()?;
            if kind < 16 {
                return Err(invalid("noncanonical extended XRPL field type"));
            }
        }
        if nth == 0 {
            nth = self.byte()?;
            if nth < 16 {
                return Err(invalid("noncanonical extended XRPL field number"));
            }
        }
        Ok(u16::from_be_bytes([kind, nth]))
    }

    fn vl(&mut self) -> Result<&'a [u8]> {
        let length = match self.byte()? as usize {
            n @ 0..=192 => n,
            n @ 193..=240 => 193 + (n - 193) * 256 + self.byte()? as usize,
            n @ 241..=254 => 12_481 + (n - 241) * 65_536 + u16::from_be_bytes(self.fixed()?) as usize,
            _ => return Err(invalid("invalid XRPL variable length header")),
        };
        if length > MAX_VL {
            return Err(invalid("XRPL variable length field exceeds size limit"));
        }
        self.take(length)
    }

    fn object(&mut self, depth: usize, nested: bool) -> Result<Value> {
        check_depth(depth)?;
        let mut last = 0;
        let mut object = Map::new();
        loop {
            if !nested && self.0.is_empty() {
                break;
            }
            let ordinal = self.ordinal()?;
            match ordinal {
                OBJECT_END if nested => break,
                OBJECT_END | ARRAY_END => return Err(invalid("unexpected XRPL container terminator")),
                _ => {}
            }
            if ordinal <= last {
                return Err(invalid("duplicate or out-of-order XRPL object field"));
            }
            last = ordinal;
            let field = lookup(ordinal)?;
            object.insert(field.name.to_owned(), self.field(field, depth)?);
        }
        Ok(Value::Object(object))
    }

    fn array(&mut self, depth: usize) -> Result<Value> {
        check_depth(depth)?;
        let mut entries = Vec::new();
        loop {
            let ordinal = self.ordinal()?;
            if ordinal == ARRAY_END {
                break;
            }
            let field = lookup(ordinal)?;
            if ordinal == OBJECT_END || field.kind != "STObject" {
                return Err(invalid("XRPL array entry is not an object"));
            }
            check_items(entries.len() + 1)?;
            let mut entry = Map::new();
            entry.insert(field.name.to_owned(), self.object(depth + 1, true)?);
            entries.push(Value::Object(entry));
        }
        Ok(Value::Array(entries))
    }

    fn field(&mut self, field: Field, depth: usize) -> Result<Value> {
        // VL values are isolated before interpretation, so nothing inside can
        // consume a following field or discard a remainder from its own field.
        if field.vl {
            return match (field.kind, self.vl()?) {
                ("Blob", bytes) => Ok(hex::encode_upper(bytes).into()),
                // rippled distinguishes a default account (zero-length) from
                // an explicitly serialized zero ID; both have the same JSON.
                ("AccountID", []) => Ok(keys::classic_address(&[0; 20]).into()),
                ("AccountID", bytes) => {
                    let id: &AccountId = bytes.try_into().map_err(|_| invalid("invalid XRPL account length"))?;
                    Ok(keys::classic_address(id).into())
                }
                ("Vector256", bytes) if bytes.len().is_multiple_of(32) => Ok(Value::Array(bytes.chunks_exact(32)
                    .map(|hash| hex::encode_upper(hash).into()).collect())),
                _ => Err(invalid("invalid XRPL variable length field")),
            };
        }
        match field.name {
            "TransactionType" => return DEFINITIONS.transaction_types.decode(u16::from_be_bytes(self.fixed()?).into()),
            "LedgerEntryType" => return DEFINITIONS.ledger_entry_types.decode(u16::from_be_bytes(self.fixed()?).into()),
            "TransactionResult" => return DEFINITIONS.transaction_results.decode(self.byte()?.into()),
            "PermissionValue" => return DEFINITIONS.permissions.decode(u32::from_be_bytes(self.fixed()?) as i32),
            _ => {}
        }
        if let Some(width) = hex_width(field.kind) {
            return Ok(hex::encode_upper(self.take(width)?).into());
        }
        Ok(match field.kind {
            "UInt8" => self.byte()?.into(),
            "UInt16" => u16::from_be_bytes(self.fixed()?).into(),
            "UInt32" => u32::from_be_bytes(self.fixed()?).into(),
            "UInt64" => {
                let bytes = self.fixed()?;
                if decimal_u64(field.name) { u64::from_be_bytes(bytes).to_string().into() }
                else { hex::encode_upper(bytes).into() }
            }
            "Int32" => i32::from_be_bytes(self.fixed()?).into(),
            "Int64" => i64::from_be_bytes(self.fixed()?).to_string().into(),
            "Number" => decimal::read_number(self)?.into(),
            "Currency" => currency_to_string(&self.fixed()?).into(),
            "AccountID" => self.account()?.into(),
            "Amount" => decimal::read_amount(self)?,
            "Issue" => self.issue()?,
            "XChainBridge" => self.bridge()?,
            "PathSet" => self.paths()?,
            "STObject" => self.object(depth + 1, true)?,
            "STArray" => self.array(depth + 1)?,
            _ => return Err(invalid("unsupported XRPL wire type")),
        })
    }

    fn account(&mut self) -> Result<String> {
        Ok(keys::classic_address(&self.fixed()?))
    }

    fn issue(&mut self) -> Result<Value> {
        let currency_or_account = self.fixed::<20>()?;
        if currency_or_account == [0; 20] {
            return Ok(json!({"currency": "XRP"}));
        }
        let issuer = self.fixed::<20>()?;
        if issuer == NO_ACCOUNT {
            let mut id = [0; 24];
            let sequence = u32::from_le_bytes(self.fixed()?);
            id[..4].copy_from_slice(&sequence.to_be_bytes());
            id[4..].copy_from_slice(&currency_or_account);
            return Ok(json!({"mpt_issuance_id": hex::encode_upper(id)}));
        }
        if issuer == [0; 20] {
            return Err(invalid("XRPL issued currency cannot have the XRP issuer"));
        }
        Ok(json!({"currency": currency_to_string(&currency_or_account), "issuer": keys::classic_address(&issuer)}))
    }

    fn bridge(&mut self) -> Result<Value> {
        let mut bridge = Map::new();
        for (door, issue) in BRIDGE_SIDES {
            if self.byte()? != 20 {
                return Err(invalid("invalid XRPL bridge account length"));
            }
            bridge.insert(door.to_owned(), self.account()?.into());
            bridge.insert(issue.to_owned(), self.issue()?);
        }
        Ok(Value::Object(bridge))
    }

    fn paths(&mut self) -> Result<Value> {
        let mut paths = Vec::new();
        let mut path = Vec::new();
        loop {
            match self.byte()? {
                marker @ (0 | 255) => {
                    if path.is_empty() {
                        return Err(invalid("empty XRPL payment path"));
                    }
                    check_items(paths.len() + 1)?;
                    paths.push(Value::Array(std::mem::take(&mut path)));
                    if marker == 0 {
                        return Ok(Value::Array(paths));
                    }
                }
                mask if mask & !0x31 == 0 => {
                    check_items(path.len() + 1)?;
                    let mut hop = Map::new();
                    if mask & 1 != 0 {
                        hop.insert("account".to_owned(), self.account()?.into());
                    }
                    if mask & 0x10 != 0 {
                        hop.insert("currency".to_owned(), currency_to_string(&self.fixed()?).into());
                    }
                    if mask & 0x20 != 0 {
                        hop.insert("issuer".to_owned(), self.account()?.into());
                    }
                    path.push(Value::Object(hop));
                }
                _ => return Err(invalid("invalid XRPL payment path element flags")),
            }
        }
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn extend(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_BLOB - self.0.len() {
            return Err(invalid("XRPL blob exceeds size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn byte(&mut self, byte: u8) -> Result<()> {
        self.extend(&[byte])
    }

    fn hex(&mut self, text: &str, len: usize) -> Result<()> {
        if text.len() != len * 2 || len > MAX_BLOB - self.0.len() {
            return Err(invalid("invalid XRPL hex field length"));
        }
        let offset = self.0.len();
        self.0.resize(offset + len, 0);
        hex::decode_to_slice(text, &mut self.0[offset..]).map_err(|_| invalid("invalid XRPL hex"))
    }

    fn ordinal(&mut self, ordinal: u16) -> Result<()> {
        let [kind, nth] = ordinal.to_be_bytes();
        self.byte((if kind < 16 { kind << 4 } else { 0 }) | if nth < 16 { nth } else { 0 })?;
        if kind >= 16 {
            self.byte(kind)?;
        }
        if nth >= 16 {
            self.byte(nth)?;
        }
        Ok(())
    }

    fn vl(&mut self, length: usize) -> Result<()> {
        match length {
            0..=192 => self.byte(length as u8),
            193..=12_480 => {
                let difference = length - 193;
                self.extend(&[193 + (difference >> 8) as u8, difference as u8])
            }
            12_481..=MAX_VL => {
                let difference = length - 12_481;
                self.extend(&[241 + (difference >> 16) as u8, (difference >> 8) as u8, difference as u8])
            }
            _ => Err(invalid("XRPL variable length field exceeds size limit")),
        }
    }

    fn object(&mut self, value: &Value, depth: usize, signing_only: bool) -> Result<()> {
        check_depth(depth)?;
        let object = map(value)?;
        let mut fields = Vec::with_capacity(object.len().min(DEFINITIONS.by_name.len()));
        for (name, value) in object {
            match DEFINITIONS.by_name.get(name.as_str()) {
                Some(Some(field)) if matches!(field.ordinal, OBJECT_END | ARRAY_END) => {
                    return Err(invalid("XRPL container markers are implicit"));
                }
                Some(Some(field)) if !signing_only || field.signing => fields.push((*field, value)),
                Some(_) => {}
                None if name.as_bytes().first().is_some_and(u8::is_ascii_lowercase) => {}
                None => return Err(invalid(format!("unknown XRPL field {name}"))),
            }
        }
        fields.sort_unstable_by_key(|(field, _)| field.ordinal);
        let is_unl_modify = fields.first().is_some_and(|(field, value)| {
            field.name == "TransactionType" && DEFINITIONS.transaction_types.encode(value).ok() == DEFINITIONS.transaction_types.by_name.get("UNLModify").copied()
        });
        for (field, value) in fields {
            self.ordinal(field.ordinal)?;
            if is_unl_modify && field.name == "Account" {
                // rippled uses a default (empty) Account for this pseudo-tx.
                if account_bytes(text(value)?)? != [0; 20] {
                    return Err(invalid("UNLModify requires the default account"));
                }
                self.vl(0)?;
            } else {
                self.field(field, value, depth)?;
            }
        }
        if depth != 0 {
            self.ordinal(OBJECT_END)?;
        }
        Ok(())
    }

    fn array(&mut self, value: &Value, depth: usize) -> Result<()> {
        check_depth(depth)?;
        let values = array(value)?;
        check_items(values.len())?;
        for value in values {
            let entry = map(value)?;
            let (name, value) = entry.iter().next().filter(|_| entry.len() == 1)
                .ok_or_else(|| invalid("XRPL array entry must have one object field"))?;
            let field = DEFINITIONS.by_name.get(name.as_str()).copied().flatten()
                .filter(|field| field.kind == "STObject" && field.ordinal != OBJECT_END)
                .ok_or_else(|| invalid("XRPL array entry is not an object"))?;
            self.ordinal(field.ordinal)?;
            self.object(value, depth + 1, false)?;
        }
        self.ordinal(ARRAY_END)
    }

    fn field(&mut self, field: Field, value: &Value, depth: usize) -> Result<()> {
        if field.vl {
            return match field.kind {
                "AccountID" => { self.vl(20)?; self.extend(&account_bytes(text(value)?)?) }
                "Blob" => {
                    let text = text(value)?;
                    self.vl(text.len() / 2)?;
                    self.hex(text, text.len() / 2)
                }
                "Vector256" => {
                    let hashes = array(value)?;
                    if hashes.len() > MAX_VL / 32 {
                        return Err(invalid("XRPL vector field exceeds size limit"));
                    }
                    self.vl(hashes.len() * 32)?;
                    for hash in hashes {
                        self.hex(text(hash)?, 32)?;
                    }
                    Ok(())
                }
                _ => Err(invalid("unsupported XRPL variable length type")),
            };
        }
        match field.name {
            "TransactionType" => return self.extend(&(DEFINITIONS.transaction_types.encode(value)? as u16).to_be_bytes()),
            "LedgerEntryType" => return self.extend(&(DEFINITIONS.ledger_entry_types.encode(value)? as u16).to_be_bytes()),
            "TransactionResult" => return self.byte(DEFINITIONS.transaction_results.encode(value)? as u8),
            "PermissionValue" => return self.extend(&(DEFINITIONS.permissions.encode(value)? as u32).to_be_bytes()),
            _ => {}
        }
        if let Some(width) = hex_width(field.kind) {
            return self.hex(text(value)?, width);
        }
        match field.kind {
            "UInt8" => self.byte(uint(value, u8::MAX.into())? as u8),
            "UInt16" => self.extend(&(uint(value, u16::MAX.into())? as u16).to_be_bytes()),
            "UInt32" => self.extend(&(uint(value, u32::MAX.into())? as u32).to_be_bytes()),
            "UInt64" => {
                let number = if !decimal_u64(field.name) && value.is_string() {
                    let text = text(value)?;
                    if text.len() > 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(invalid("invalid XRPL UInt64 hex"));
                    }
                    u64::from_str_radix(text, 16).map_err(|_| invalid("invalid XRPL UInt64 hex"))?
                } else {
                    uint(value, u64::MAX)?
                };
                self.extend(&number.to_be_bytes())
            }
            "Int32" => {
                let number = i32::try_from(int(value)?).map_err(|_| invalid("XRPL Int32 out of range"))?;
                self.extend(&number.to_be_bytes())
            }
            "Int64" => self.extend(&int(value)?.to_be_bytes()),
            "Number" => decimal::write_number(self, text(value)?),
            "Currency" => self.extend(&currency_bytes(text(value)?)?),
            "AccountID" => self.extend(&account_bytes(text(value)?)?),
            "Amount" => decimal::write_amount(self, field.name, value),
            "Issue" => self.issue(value),
            "XChainBridge" => self.bridge(value),
            "PathSet" => self.paths(value),
            "STObject" => self.object(value, depth + 1, false),
            "STArray" => self.array(value, depth + 1),
            _ => Err(invalid("unsupported XRPL wire type")),
        }
    }

    fn issue(&mut self, value: &Value) -> Result<()> {
        let issue = map(value)?;
        match (issue.len(), issue.get("currency"), issue.get("issuer"), issue.get("mpt_issuance_id")) {
            (1, Some(currency), None, None) if currency_bytes(text(currency)?)? == [0; 20] => self.extend(&[0; 20]),
            (2, Some(currency), Some(issuer), None) => {
                let currency = currency_bytes(text(currency)?)?;
                let issuer = account_bytes(text(issuer)?)?;
                if currency == [0; 20] || issuer == [0; 20] || issuer == NO_ACCOUNT {
                    return Err(invalid("invalid XRPL issued currency or issuer"));
                }
                self.extend(&currency)?;
                self.extend(&issuer)
            }
            (1, None, None, Some(id)) => {
                let id = fixed_hex::<24>(text(id)?)?;
                if id[4..] == [0; 20] {
                    return Err(invalid("invalid XRPL MPT issuer"));
                }
                self.extend(&id[4..])?;
                self.extend(&NO_ACCOUNT)?;
                self.extend(&[id[3], id[2], id[1], id[0]])
            }
            _ => Err(invalid("invalid XRPL issue")),
        }
    }

    fn bridge(&mut self, value: &Value) -> Result<()> {
        let bridge = map(value)?;
        if bridge.len() != 4 {
            return Err(invalid("invalid XRPL bridge fields"));
        }
        for (door, issue) in BRIDGE_SIDES {
            self.byte(20)?;
            self.extend(&account_bytes(text(required(bridge, door)?)?)?)?;
            self.issue(required(bridge, issue)?)?;
        }
        Ok(())
    }

    fn paths(&mut self, value: &Value) -> Result<()> {
        let paths = array(value)?;
        if paths.is_empty() {
            return Err(invalid("empty XRPL path set"));
        }
        check_items(paths.len())?;
        for (index, path) in paths.iter().enumerate() {
            if index != 0 {
                self.byte(255)?;
            }
            let path = array(path)?;
            if path.is_empty() {
                return Err(invalid("empty XRPL payment path"));
            }
            check_items(path.len())?;
            for hop in path {
                let hop = map(hop)?;
                let account = hop.get("account");
                let currency = hop.get("currency");
                let issuer = hop.get("issuer");
                let mask = u8::from(account.is_some()) | (u8::from(currency.is_some()) << 4) | (u8::from(issuer.is_some()) << 5);
                if mask == 0 || mask.count_ones() as usize != hop.len() {
                    return Err(invalid("invalid XRPL payment path element"));
                }
                self.byte(mask)?;
                if let Some(value) = account {
                    self.extend(&account_bytes(text(value)?)?)?;
                }
                if let Some(value) = currency {
                    self.extend(&currency_bytes(text(value)?)?)?;
                }
                if let Some(value) = issuer {
                    self.extend(&account_bytes(text(value)?)?)?;
                }
            }
        }
        self.byte(0)
    }
}

fn lookup(ordinal: u16) -> Result<Field> {
    DEFINITIONS.by_ordinal.get(&ordinal).copied().ok_or_else(|| invalid(format!("unknown XRPL field {ordinal:04X}")))
}

fn check_depth(depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid("XRPL nesting depth exceeded"));
    }
    Ok(())
}

fn check_items(items: usize) -> Result<()> {
    if items > MAX_ARRAY_ITEMS {
        return Err(invalid("XRPL container item limit exceeded"));
    }
    Ok(())
}

fn hex_width(kind: &str) -> Option<usize> {
    match kind {
        "Hash128" => Some(16),
        "Hash160" => Some(20),
        "Hash192" => Some(24),
        "Hash256" => Some(32),
        "Hash384" => Some(48),
        "Hash512" => Some(64),
        _ => None,
    }
}

fn decimal_u64(name: &str) -> bool {
    matches!(name, "MaximumAmount" | "OutstandingAmount" | "MPTAmount" | "LockedAmount" | "ConfidentialOutstandingAmount")
}

fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| invalid("expected XRPL string"))
}

fn map(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(|| invalid("expected XRPL object"))
}

fn array(value: &Value) -> Result<&[Value]> {
    value.as_array().map(Vec::as_slice).ok_or_else(|| invalid("expected XRPL array"))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    object.get(key).ok_or_else(|| invalid("missing XRPL object member"))
}

fn uint(value: &Value, max: u64) -> Result<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok()).filter(|n| *n <= max)
        .ok_or_else(|| invalid("invalid XRPL unsigned integer"))
}

fn int(value: &Value) -> Result<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok()).ok_or_else(|| invalid("invalid XRPL signed integer"))
}

fn fixed_hex<const N: usize>(text: &str) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    hex::decode_to_slice(text, &mut bytes).map_err(|_| invalid("invalid XRPL fixed hex"))?;
    Ok(bytes)
}

fn account_bytes(text: &str) -> Result<AccountId> {
    match text.len() {
        0 => Ok([0; 20]),
        40 => fixed_hex(text),
        _ => keys::decode_address(text),
    }
}

fn iso_currency(code: &[u8]) -> bool {
    code.iter().all(|&b| b.is_ascii_alphanumeric() || b"?!@#$%^&*(){}[]|".contains(&b))
}

fn currency_bytes(text: &str) -> Result<[u8; 20]> {
    if text.len() == 3 && iso_currency(text.as_bytes()) {
        let mut bytes = [0; 20];
        if text != "XRP" {
            bytes[12..15].copy_from_slice(text.as_bytes());
        }
        Ok(bytes)
    } else {
        fixed_hex(text)
    }
}

fn currency_to_string(bytes: &[u8; 20]) -> String {
    if *bytes == [0; 20] {
        "XRP".to_owned()
    } else if bytes[..12] == [0; 12] && bytes[15..] == [0; 5] && &bytes[12..15] != b"XRP" && iso_currency(&bytes[12..15]) {
        std::str::from_utf8(&bytes[12..15]).expect("ASCII currency code").to_owned()
    } else {
        hex::encode_upper(bytes)
    }
}
