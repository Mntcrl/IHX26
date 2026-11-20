//! Exact decimal conversion for STAmount and STNumber without floating-point
//! rounding or dependence on the host's locale or word size.

use super::{Reader, Result, Writer, account_bytes, currency_bytes, currency_to_string, invalid, keys, map, text};
use serde_json::{Value, json};

const POSITIVE: u64 = 1 << 62;
const IOU: u64 = 1 << 63;
const MPT_BYTE: u8 = 0x20;
const IOU_MANTISSA_MASK: u64 = (1 << 54) - 1;
const IOU_MIN_MANTISSA: u64 = 1_000_000_000_000_000;
const IOU_MAX_MANTISSA: u64 = 9_999_999_999_999_999;
const IOU_EXPONENT: std::ops::RangeInclusive<i32> = -96..=80;
const MAX_DROPS: u64 = 100_000_000_000_000_000;
const NUMBER_MIN_MANTISSA: u64 = 1_000_000_000_000_000_000;
const NUMBER_EXPONENT: std::ops::RangeInclusive<i32> = -32_768..=32_768;
const NUMBER_ZERO_EXPONENT: i32 = i32::MIN;
const MAX_SIGNED: u64 = i64::MAX as u64;

pub(super) fn read_amount(reader: &mut Reader<'_>) -> Result<Value> {
    let flags = reader.0.first().copied().ok_or_else(|| invalid("truncated XRPL amount"))?;
    if flags & 0x80 != 0 {
        let word = u64::from_be_bytes(reader.fixed()?);
        let currency = reader.fixed::<20>()?;
        let issuer = reader.fixed::<20>()?;
        if currency == [0; 20] || issuer == [0; 20] {
            return Err(invalid("invalid XRPL issued amount currency or issuer"));
        }
        let mantissa = word & IOU_MANTISSA_MASK;
        let exponent = ((word >> 54) & 0xff) as i32 - 97;
        let value = if word == IOU {
            "0".to_owned()
        } else if (IOU_MIN_MANTISSA..=IOU_MAX_MANTISSA).contains(&mantissa) && IOU_EXPONENT.contains(&exponent) {
            format_decimal(mantissa, exponent, word & POSITIVE == 0)
        } else {
            return Err(invalid("noncanonical XRPL issued amount"));
        };
        return Ok(json!({"value": value, "currency": currency_to_string(&currency), "issuer": keys::classic_address(&issuer)}));
    }
    if flags & MPT_BYTE != 0 {
        if reader.byte()? & !0x60 != 0 {
            return Err(invalid("invalid XRPL MPT flags"));
        }
        let magnitude = u64::from_be_bytes(reader.fixed()?);
        let value = signed_amount(magnitude, flags & 0x40 == 0, MAX_SIGNED)?;
        return Ok(json!({"value": value, "mpt_issuance_id": hex::encode_upper(reader.take(24)?)}));
    }
    let word = u64::from_be_bytes(reader.fixed()?);
    Ok(signed_amount(word & !POSITIVE, word & POSITIVE == 0, MAX_DROPS)?.into())
}

pub(super) fn write_amount(writer: &mut Writer, field: &str, value: &Value) -> Result<()> {
    if let Some(value) = value.as_str() {
        let (magnitude, negative) = parse_signed_amount(value, MAX_DROPS)?;
        let word = if negative { magnitude } else { magnitude | POSITIVE };
        return writer.extend(&word.to_be_bytes());
    }
    if field == "FeeAmountDelta" {
        return Err(invalid("XRPL FeeAmountDelta must be native"));
    }
    let amount = map(value)?;
    match (amount.len(), amount.get("value"), amount.get("currency"), amount.get("issuer"), amount.get("mpt_issuance_id")) {
        (2, Some(value), None, None, Some(id)) => {
            let (magnitude, negative) = parse_signed_amount(text(value)?, MAX_SIGNED)?;
            writer.byte(if negative { 0x20 } else { 0x60 })?;
            writer.extend(&magnitude.to_be_bytes())?;
            writer.hex(text(id)?, 24)
        }
        (3, Some(value), Some(currency), Some(issuer), None) => {
            let currency = currency_bytes(text(currency)?)?;
            let issuer = account_bytes(text(issuer)?)?;
            if currency == [0; 20] || issuer == [0; 20] {
                return Err(invalid("invalid XRPL issued amount currency or issuer"));
            }
            let parts = parse_decimal(text(value)?, false)?;
            let mut word = IOU;
            if parts.digits != 0 {
                let padding = 16 - parts.digits;
                let exponent = parts.exponent - padding;
                if padding < 0 || !IOU_EXPONENT.contains(&exponent) {
                    return Err(invalid("XRPL issued amount precision or exponent out of range"));
                }
                let mantissa = parts.mantissa * 10_u64.pow(padding as u32);
                word |= ((exponent + 97) as u64) << 54 | mantissa;
                if !parts.negative {
                    word |= POSITIVE;
                }
            }
            writer.extend(&word.to_be_bytes())?;
            writer.extend(&currency)?;
            writer.extend(&issuer)
        }
        _ => Err(invalid("invalid XRPL amount")),
    }
}

fn signed_amount(magnitude: u64, negative: bool, maximum: u64) -> Result<String> {
    if magnitude > maximum || (magnitude == 0 && negative) {
        return Err(invalid("noncanonical XRPL amount magnitude"));
    }
    let value = magnitude as i64;
    Ok(if negative { -value } else { value }.to_string())
}

fn parse_signed_amount(text: &str, maximum: u64) -> Result<(u64, bool)> {
    let value = text.parse::<i64>().map_err(|_| invalid("invalid XRPL integer amount"))?;
    let magnitude = value.unsigned_abs();
    if magnitude > maximum {
        return Err(invalid("XRPL amount magnitude out of range"));
    }
    Ok((magnitude, value < 0))
}

pub(super) fn read_number(reader: &mut Reader<'_>) -> Result<String> {
    let mantissa = i64::from_be_bytes(reader.fixed()?);
    let mut exponent = i32::from_be_bytes(reader.fixed()?);
    if mantissa == 0 && exponent == NUMBER_ZERO_EXPONENT {
        return Ok("0".to_owned());
    }
    let mut magnitude = mantissa.unsigned_abs();
    if !(MAX_SIGNED / 10 + 1..=MAX_SIGNED).contains(&magnitude) || !NUMBER_EXPONENT.contains(&exponent) {
        return Err(invalid("noncanonical XRPL Number"));
    }
    // A 19-digit mantissa that exceeds i64 is stored with one fewer digit.
    if magnitude < NUMBER_MIN_MANTISSA {
        magnitude *= 10;
        exponent -= 1;
        if exponent < *NUMBER_EXPONENT.start() {
            return Err(invalid("noncanonical XRPL Number exponent"));
        }
    }
    if exponent == 0 || (-28..=-8).contains(&exponent) {
        return Ok(format_decimal(magnitude, exponent, mantissa < 0));
    }
    while magnitude.is_multiple_of(10) && exponent < *NUMBER_EXPONENT.end() {
        magnitude /= 10;
        exponent += 1;
    }
    Ok(format!("{}{magnitude}e{exponent}", if mantissa < 0 { "-" } else { "" }))
}

pub(super) fn write_number(writer: &mut Writer, value: &str) -> Result<()> {
    let parts = parse_decimal(value, true)?;
    if parts.digits == 0 {
        writer.extend(&[0; 8])?;
        return writer.extend(&NUMBER_ZERO_EXPONENT.to_be_bytes());
    }
    let difference = parts.digits - 19;
    let mut exponent = parts.exponent + difference;
    let mut mantissa = parts.mantissa;
    let mut rounding = parts.rounding;
    if difference < 0 {
        mantissa *= 10_u64.pow((-difference) as u32);
    }
    if !NUMBER_EXPONENT.contains(&exponent) {
        return Err(invalid("XRPL Number exponent out of range"));
    }
    if mantissa > MAX_SIGNED {
        exponent += 1;
        rounding = (mantissa % 10) as u8;
        mantissa /= 10;
    }
    if rounding >= 5 {
        mantissa += 1;
        if mantissa > MAX_SIGNED {
            exponent += 1;
            mantissa = (mantissa + 5) / 10;
        }
    }
    if !NUMBER_EXPONENT.contains(&exponent) {
        return Err(invalid("XRPL Number exponent out of range"));
    }
    let signed = mantissa as i64;
    writer.extend(&(if parts.negative { -signed } else { signed }).to_be_bytes())?;
    writer.extend(&exponent.to_be_bytes())
}

struct Decimal {
    // First 19 significant digits, plus the first discarded digit for rounding.
    mantissa: u64,
    rounding: u8,
    digits: i32,
    exponent: i32,
    negative: bool,
}

fn parse_decimal(text: &str, require_integer_and_fraction_digits: bool) -> Result<Decimal> {
    if text.len() > 1024 {
        return Err(invalid("XRPL decimal string exceeds size limit"));
    }
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (decimal, exponent) = match text.split_once(['e', 'E']) {
        Some((decimal, exponent)) => (decimal, exponent.parse::<i32>().ok().filter(|exp| exp.unsigned_abs() <= 1_000_000)
            .ok_or_else(|| invalid("invalid XRPL decimal exponent"))?),
        None => (text, 0),
    };
    let (integer, fraction) = if let Some((integer, fraction)) = decimal.split_once('.') {
        if require_integer_and_fraction_digits && (integer.is_empty() || fraction.is_empty()) {
            return Err(invalid("invalid XRPL decimal digits"));
        }
        (integer, fraction)
    } else {
        (decimal, "")
    };
    if integer.is_empty() && fraction.is_empty() {
        return Err(invalid("missing XRPL decimal digits"));
    }
    let mut parts = Decimal { mantissa: 0, rounding: 0, digits: 0, exponent: 0, negative };
    let mut trailing_zeros = 0;
    for digit in integer.bytes().chain(fraction.bytes()) {
        if !digit.is_ascii_digit() {
            return Err(invalid("invalid XRPL decimal digit"));
        }
        let digit = digit - b'0';
        if parts.digits == 0 && digit == 0 {
            continue;
        }
        parts.digits += 1;
        if parts.digits <= 19 {
            parts.mantissa = parts.mantissa * 10 + u64::from(digit);
        } else if parts.digits == 20 {
            parts.rounding = digit;
        }
        trailing_zeros = if digit == 0 { trailing_zeros + 1 } else { 0 };
    }
    let stored_zeros = trailing_zeros - (parts.digits - 19).max(0);
    if stored_zeros > 0 {
        parts.mantissa /= 10_u64.pow(stored_zeros as u32);
    }
    parts.digits -= trailing_zeros;
    parts.exponent = exponent - fraction.len() as i32 + trailing_zeros;
    Ok(parts)
}

fn format_decimal(mut magnitude: u64, mut exponent: i32, negative: bool) -> String {
    while exponent < 0 && magnitude.is_multiple_of(10) {
        magnitude /= 10;
        exponent += 1;
    }
    if exponent == 0 {
        let digits = magnitude as i128;
        return if negative { -digits } else { digits }.to_string();
    }
    let mut digits = magnitude.to_string();
    if exponent > 0 {
        digits.extend(std::iter::repeat_n('0', exponent as usize));
        if negative {
            digits.insert(0, '-');
        }
        return digits;
    }
    let decimal_places = exponent.unsigned_abs() as usize;
    if digits.len() > decimal_places {
        digits.insert(digits.len() - decimal_places, '.');
        if negative {
            digits.insert(0, '-');
        }
        digits
    } else {
        let prefix = if negative { "-0." } else { "0." };
        let mut result = String::with_capacity(prefix.len() + decimal_places);
        result.push_str(prefix);
        result.extend(std::iter::repeat_n('0', decimal_places - digits.len()));
        result.push_str(&digits);
        result
    }
}
