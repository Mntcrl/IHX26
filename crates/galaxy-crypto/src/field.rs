//! Canonical scalar encoding (§2); field reduction is used only for signed integers.
use crate::Fr;
use ark_ff::{BigInt, PrimeField};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("non-canonical BN254 scalar")]
pub struct FieldError;

pub fn fr_from_be_canonical(bytes: &[u8]) -> Result<Fr, FieldError> {
    let bytes: &[u8; 32] = bytes.try_into().map_err(|_| FieldError)?;
    Fr::from_bigint(bigint_from_be(bytes)).ok_or(FieldError)
}

pub(crate) fn bigint_from_be(bytes: &[u8; 32]) -> BigInt<4> {
    BigInt(std::array::from_fn(|i| {
        let start = 24 - 8 * i;
        u64::from_be_bytes(bytes[start..start + 8].try_into().unwrap())
    }))
}

pub(crate) fn bigint_to_be(value: BigInt<4>) -> [u8; 32] {
    let mut out = [0; 32];
    for (dst, limb) in out.chunks_exact_mut(8).zip(value.0.into_iter().rev()) {
        dst.copy_from_slice(&limb.to_be_bytes());
    }
    out
}

pub fn fr_to_be(value: &Fr) -> [u8; 32] {
    bigint_to_be(value.into_bigint())
}

pub fn hash_limbs(bytes: &[u8; 32]) -> (Fr, Fr) {
    let (hi, lo) = bytes.split_at(16);
    (
        Fr::from(u128::from_be_bytes(hi.try_into().unwrap())),
        Fr::from(u128::from_be_bytes(lo.try_into().unwrap())),
    )
}

pub fn fr_from_i128(value: i128) -> Fr {
    let magnitude = Fr::from(value.unsigned_abs());
    if value < 0 { -magnitude } else { magnitude }
}

pub fn fr_to_dec(value: &Fr) -> String {
    value.into_bigint().to_string()
}

/// Parse unsigned, canonical base-10 digits, checking before converting to a field.
pub(crate) fn bigint_from_decimal(bytes: &[u8]) -> Option<BigInt<4>> {
    if bytes.is_empty() || bytes.len() > 77 || (bytes.len() > 1 && bytes[0] == b'0') {
        return None;
    }
    let mut limbs = [0u64; 4];
    for &digit in bytes {
        if !digit.is_ascii_digit() {
            return None;
        }
        let mut carry = u128::from(digit - b'0');
        for limb in &mut limbs {
            let value = 10 * u128::from(*limb) + carry;
            *limb = value as u64;
            carry = value >> 64;
        }
        if carry != 0 {
            return None;
        }
    }
    Some(BigInt(limbs))
}

pub fn fr_from_dec(text: &str) -> Result<Fr, FieldError> {
    bigint_from_decimal(text.as_bytes())
        .and_then(Fr::from_bigint)
        .ok_or(FieldError)
}
