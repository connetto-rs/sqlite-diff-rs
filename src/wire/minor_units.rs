//! Exact conversion of a decimal into the integer count of minor units a
//! [`WireType::ScaledDecimal`](super::WireType::ScaledDecimal) column
//! stores, `value × 10^scale`, from decimal text or a PostgreSQL binary
//! `numeric`.
//!
//! A value converts only when it is a whole number of minor units fitting
//! an `i64`. The spelling does not matter, so `12.500`, `1.25E+1` and a
//! binary `numeric` with any `dscale` holding 12.5 are all 1250 at scale 2,
//! while `12.505` is refused rather than rounded.

use alloc::string::ToString;

use super::error::DecodeError;
use super::scalar_helpers::PgNumeric;
use super::wire_type::WireType;
use crate::encoding::Value;

/// Why decimal text has no minor-units form.
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
enum Refusal {
    /// The input is not a decimal at all.
    Malformed,
    /// The input is a decimal that needs rounding, overflows `i64`, or is
    /// `NaN` or an infinity.
    Unrepresentable,
}

/// The decimal digits of a value, fed most significant first, kept as the
/// digits up to the last nonzero one and a count of the zeros after it.
///
/// Trailing zeros stay counted rather than multiplied in, so a value
/// spelled with more zeros than an `i64` has digits still converts when
/// the scale divides them away again.
struct Digits {
    /// The digits up to the last nonzero one, `None` once they overflow.
    significand: Option<u64>,
    /// Zero digits after the significand.
    zeros: u64,
}

impl Digits {
    const fn new() -> Self {
        Self {
            significand: Some(0),
            zeros: 0,
        }
    }

    fn push(&mut self, digit: u8) {
        if digit == 0 {
            if self.significand != Some(0) {
                self.zeros = self.zeros.saturating_add(1);
            }
            return;
        }
        self.significand = self.significand.and_then(|significand| {
            let shift = u32::try_from(self.zeros.checked_add(1)?).ok()?;
            significand
                .checked_mul(10_u64.checked_pow(shift)?)?
                .checked_add(u64::from(digit))
        });
        self.zeros = 0;
    }

    /// The minor units at `scale` of the value these digits spell when
    /// the last one is worth `10^exponent`, or `None` when that is not a
    /// whole `i64`.
    fn minor_units(&self, negative: bool, exponent: i64, scale: u32) -> Option<i64> {
        let significand = self.significand?;
        if significand == 0 {
            return Some(0);
        }
        // The significand ends in a nonzero digit, so a negative shift
        // would need rounding.
        let shift = i128::from(self.zeros) + i128::from(exponent) + i128::from(scale);
        let magnitude = significand.checked_mul(10_u64.checked_pow(u32::try_from(shift).ok()?)?)?;
        if negative {
            0_i64.checked_sub_unsigned(magnitude)
        } else {
            i64::try_from(magnitude).ok()
        }
    }
}

/// Push the run of ASCII digits at the start of `rest` into `digits` and
/// advance past it, returning how many there were.
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
fn take_digits(rest: &mut &[u8], digits: &mut Digits) -> Result<i64, Refusal> {
    let count = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if count == 0 {
        return Err(Refusal::Malformed);
    }
    let (taken, after) = rest.split_at(count);
    for b in taken {
        digits.push(b - b'0');
    }
    *rest = after;
    i64::try_from(count).map_err(|_| Refusal::Malformed)
}

/// Minor units at `scale` of decimal text, which is an optional `-`,
/// digits, an optional `.` and digits, and an optional exponent, the
/// union of `numeric_out` and JSON number spellings. `NaN`, `Infinity`
/// and `-Infinity` are decimals with no minor units.
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
fn text_minor_units(text: &str, scale: u32) -> Result<i64, Refusal> {
    if matches!(text, "NaN" | "Infinity" | "-Infinity") {
        return Err(Refusal::Unrepresentable);
    }
    let (negative, mut rest) = match text.as_bytes() {
        [b'-', rest @ ..] => (true, rest),
        bytes => (false, bytes),
    };
    let mut digits = Digits::new();
    take_digits(&mut rest, &mut digits)?;
    let mut exponent = 0_i64;
    if let [b'.', after @ ..] = rest {
        rest = after;
        exponent = -take_digits(&mut rest, &mut digits)?;
    }
    if let [b'e' | b'E', after @ ..] = rest {
        let (exponent_negative, after) = match after {
            [b'-', after @ ..] => (true, after),
            [b'+', after @ ..] | after => (false, after),
        };
        let count = after.iter().take_while(|b| b.is_ascii_digit()).count();
        if count == 0 {
            return Err(Refusal::Malformed);
        }
        let (taken, after) = after.split_at(count);
        let written = taken.iter().fold(0_i64, |value, b| {
            value.saturating_mul(10).saturating_add(i64::from(b - b'0'))
        });
        exponent = if exponent_negative {
            exponent.saturating_sub(written)
        } else {
            exponent.saturating_add(written)
        };
        rest = after;
    }
    if !rest.is_empty() {
        return Err(Refusal::Malformed);
    }
    digits
        .minor_units(negative, exponent, scale)
        .ok_or(Refusal::Unrepresentable)
}

/// Minor units at `scale` of a PostgreSQL binary `numeric`, or `None` when
/// they are not a whole `i64`.
fn binary_minor_units(numeric: &PgNumeric<'_>, scale: u32) -> Option<i64> {
    if numeric.special.is_some() {
        return None;
    }
    let mut digits = Digits::new();
    let mut count = 0_i64;
    for base_10000 in numeric.digits() {
        for place in [1000, 100, 10, 1] {
            // Each base-10000 digit is at most 9999, so every place is a decimal digit.
            digits.push(u8::try_from(base_10000 / place % 10).unwrap_or_default());
        }
        count += 1;
    }
    // The last base-10000 digit is worth 10000^(weight - count + 1).
    let exponent = 4 * (numeric.weight - count + 1);
    digits.minor_units(numeric.negative, exponent, scale)
}

/// Decode decimal text as the minor units at `scale`.
///
/// # Errors
///
/// [`DecodeError::UnrepresentableDecimal`] for a value that is not a whole
/// `i64` of minor units, and [`DecodeError::WrongPayloadKind`] for text
/// that is not a decimal.
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
pub(crate) fn decode_text_minor_units<S, B>(
    column_name: &str,
    text: &str,
    scale: u32,
) -> Result<Value<S, B>, DecodeError> {
    text_minor_units(text, scale)
        .map(Value::Integer)
        .map_err(|refusal| match refusal {
            Refusal::Malformed => DecodeError::WrongPayloadKind {
                column: column_name.to_string(),
                expected: "decimal text",
                actual: "other text",
            },
            Refusal::Unrepresentable => DecodeError::UnrepresentableDecimal {
                column: column_name.to_string(),
                value: text.to_string(),
                scale,
            },
        })
}

/// Decode a PostgreSQL binary `numeric` as the minor units at `scale`.
///
/// # Errors
///
/// [`DecodeError::UnrepresentableDecimal`] for a value that is not a whole
/// `i64` of minor units, and [`DecodeError::WrongPayloadKind`] for bytes
/// `numeric_send` would not produce.
pub(crate) fn decode_pg_numeric_binary_minor_units<S, B>(
    column_name: &str,
    bytes: &[u8],
    scale: u32,
) -> Result<Value<S, B>, DecodeError> {
    let numeric = PgNumeric::parse(bytes).ok_or_else(|| DecodeError::WrongPayloadKind {
        column: column_name.to_string(),
        expected: "numeric binary",
        actual: "other binary contents",
    })?;
    binary_minor_units(&numeric, scale)
        .map(Value::Integer)
        .ok_or_else(|| DecodeError::UnrepresentableDecimal {
            column: column_name.to_string(),
            value: numeric.render(),
            scale,
        })
}

/// The scale of a [`WireType::ScaledDecimal`] payload.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] when the payload's wire type names no
/// scale, which happens when the minor-units decoder is registered under
/// another kind.
pub(crate) fn payload_scale(column_name: &str, wire_type: WireType) -> Result<u32, DecodeError> {
    match wire_type {
        WireType::ScaledDecimal { scale } => Ok(scale),
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "a scaled decimal wire type",
            actual: "a wire type without a scale",
        }),
    }
}
