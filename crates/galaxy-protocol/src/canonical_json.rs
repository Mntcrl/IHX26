//! Strict Galaxy canonical JSON (§2). Pretty JSON can be normalized explicitly,
//! but identifiers of externally supplied bytes require `validate` first.
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("canonical JSON requires ASCII strings")]
    NonAscii,
    #[error("canonical JSON permits only integers of magnitude less than 2^53")]
    Number,
    #[error("input is not canonical Galaxy JSON")]
    NonCanonical,
}

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    encode(&serde_json::to_value(value)?, &mut bytes)?;
    Ok(bytes)
}

/// Validate exact bytes, rejecting nonminimal escapes, duplicate keys and alternate numbers.
pub fn validate(bytes: &[u8]) -> Result<(), Error> {
    let value: Value = serde_json::from_slice(bytes)?;
    if to_vec(&value)? != bytes {
        return Err(Error::NonCanonical);
    }
    Ok(())
}

/// `validate`, additionally requiring a top-level object (descriptor and policy documents).
pub fn validate_object(bytes: &[u8]) -> Result<(), Error> {
    validate(bytes)?;
    if bytes.first() != Some(&b'{') {
        return Err(Error::NonCanonical);
    }
    Ok(())
}

fn string(value: &str, out: &mut Vec<u8>) -> Result<(), Error> {
    if !value.is_ascii() {
        return Err(Error::NonAscii);
    }
    const HEX: &[u8] = b"0123456789abcdef";
    out.push(b'"');
    for b in value.bytes() {
        match b {
            b'"' | b'\\' => out.extend([b'\\', b]),
            0..=31 => {
                out.extend(b"\\u00");
                out.extend([HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]]);
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
    Ok(())
}

fn encode(value: &Value, out: &mut Vec<u8>) -> Result<(), Error> {
    match value {
        Value::Null => out.extend(b"null"),
        Value::Bool(true) => out.extend(b"true"),
        Value::Bool(false) => out.extend(b"false"),
        Value::Number(n) => {
            let n = n.as_i64().filter(|n| n.unsigned_abs() < (1 << 53)).ok_or(Error::Number)?;
            out.extend(n.to_string().bytes());
        }
        Value::String(s) => string(s, out)?,
        Value::Array(xs) => {
            out.push(b'[');
            for (i, x) in xs.iter().enumerate() {
                if i != 0 { out.push(b','); }
                encode(x, out)?;
            }
            out.push(b']');
        }
        Value::Object(xs) => {
            // Explicit sorting is necessary even if a workspace consumer enables serde_json's
            // insertion-order feature.
            let ordered: BTreeMap<_, _> = xs.iter().collect();
            out.push(b'{');
            for (i, (key, x)) in ordered.into_iter().enumerate() {
                if i != 0 { out.push(b','); }
                string(key, out)?;
                out.push(b':');
                encode(x, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}
