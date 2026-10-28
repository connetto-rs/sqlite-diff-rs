//! Source-independent scalar decode primitives shared by the wire
//! [`Decoder`](super::Decoder) implementations.
//!
//! [`normalize_real`] matches SQLite's `decode_value` float handling and
//! is used by every source. The `decode_pg_*_binary` helpers decode the
//! PostgreSQL binary send format for integer, floating-point, boolean,
//! numeric and JSON fields, which is identical whether the bytes arrive
//! over logical replication in binary mode ([`PgWalstream`](crate::pg_walstream::PgWalstream))
//! or as a binary query result ([`PgBinary`](super::PgBinary)).

use alloc::string::{String, ToString};
use core::fmt::Write;

use super::error::DecodeError;
use super::wire_type::WireType;
use crate::encoding::Value;

/// Map a decoded float to a value, NaN becoming `Value::Null` because SQLite never stores NaN.
#[inline]
pub(crate) fn normalize_real<S, B>(f: f64) -> Value<S, B> {
    if f.is_nan() {
        Value::Null
    } else {
        Value::Real(f)
    }
}

/// Decode a PostgreSQL binary integer (`int2`/`int4`/`int8`) from its 2,
/// 4, or 8 byte big-endian two's complement form, widened to `i64`.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for any other byte count.
pub(crate) fn decode_pg_int_binary<S, B>(
    column_name: &str,
    bytes: &[u8],
) -> Result<Value<S, B>, DecodeError> {
    match bytes.len() {
        2 => {
            let arr: [u8; 2] = bytes.try_into().unwrap();
            Ok(Value::Integer(i16::from_be_bytes(arr).into()))
        }
        4 => {
            let arr: [u8; 4] = bytes.try_into().unwrap();
            Ok(Value::Integer(i32::from_be_bytes(arr).into()))
        }
        8 => {
            let arr: [u8; 8] = bytes.try_into().unwrap();
            Ok(Value::Integer(i64::from_be_bytes(arr)))
        }
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "int2, int4, or int8 binary (2, 4, or 8 bytes)",
            actual: "unexpected binary integer width",
        }),
    }
}

/// Decode a PostgreSQL binary float (`float4`/`float8`) from its 4 or 8
/// byte big-endian IEEE 754 form, normalized via [`normalize_real`].
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for any other byte count.
pub(crate) fn decode_pg_real_binary<S, B>(
    column_name: &str,
    bytes: &[u8],
) -> Result<Value<S, B>, DecodeError> {
    match bytes.len() {
        4 => {
            let arr: [u8; 4] = bytes.try_into().unwrap();
            Ok(normalize_real(f64::from(f32::from_be_bytes(arr))))
        }
        8 => {
            let arr: [u8; 8] = bytes.try_into().unwrap();
            Ok(normalize_real(f64::from_be_bytes(arr)))
        }
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "float4 or float8 binary (4 or 8 bytes)",
            actual: "unexpected binary float width",
        }),
    }
}

/// Decode a PostgreSQL binary boolean: a single byte `0x01` (true) or
/// `0x00` (false).
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for any other contents.
pub(crate) fn decode_pg_bool_binary<S, B>(
    column_name: &str,
    bytes: &[u8],
) -> Result<Value<S, B>, DecodeError> {
    match bytes {
        [0x01] => Ok(Value::Integer(1)),
        [0x00] => Ok(Value::Integer(0)),
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "single byte 0x00 or 0x01",
            actual: "other binary contents",
        }),
    }
}

/// Render a PostgreSQL binary `numeric` as the text `numeric_out` prints,
/// so it matches the text a `numeric` carries over logical replication.
///
/// The layout is `ndigits`, `weight`, `sign` and `dscale` as big-endian
/// 16-bit fields, then `ndigits` base-10000 digits, the first of which is
/// multiplied by 10000 to the power `weight`.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for bytes `numeric_send` would not
/// produce.
pub(crate) fn decode_pg_numeric_binary<S, B>(
    column_name: &str,
    bytes: &[u8],
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String>,
{
    PgNumeric::parse(bytes)
        .map(|numeric| Value::Text(S::from(numeric.render())))
        .ok_or_else(|| DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "numeric binary",
            actual: "other binary contents",
        })
}

/// A PostgreSQL binary `numeric` whose header and digits are well formed.
pub(crate) struct PgNumeric<'a> {
    /// `NaN`, `Infinity` or `-Infinity`, which carry no digits.
    pub(crate) special: Option<&'static str>,
    /// Whether a finite value is negative.
    pub(crate) negative: bool,
    /// Power of 10000 the first digit is multiplied by.
    pub(crate) weight: i64,
    /// Fractional decimal digits the value is displayed with.
    dscale: usize,
    /// Big-endian base-10000 digits, each at most 9999.
    digits: &'a [u8],
}

impl<'a> PgNumeric<'a> {
    /// Read the `ndigits`, `weight`, `sign` and `dscale` big-endian 16-bit
    /// fields and the `ndigits` base-10000 digits after them, or `None` for
    /// bytes `numeric_send` would not produce.
    pub(crate) fn parse(bytes: &'a [u8]) -> Option<Self> {
        const POSITIVE: u16 = 0x0000;
        const NEGATIVE: u16 = 0x4000;
        const NAN: u16 = 0xC000;
        const INFINITY: u16 = 0xD000;
        const NEGATIVE_INFINITY: u16 = 0xF000;
        const MAX_DSCALE: u16 = 0x3FFF;

        let field = |at: usize| Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
        let ndigits = usize::from(field(0)?);
        let weight = i64::from(field(2)?.cast_signed());
        let sign = field(4)?;
        let dscale = field(6)?;
        let digits = bytes.get(8..)?;
        if digits.len() != 2 * ndigits || dscale > MAX_DSCALE {
            return None;
        }
        let special = match sign {
            NAN => Some("NaN"),
            INFINITY => Some("Infinity"),
            NEGATIVE_INFINITY => Some("-Infinity"),
            POSITIVE | NEGATIVE => None,
            _ => return None,
        };
        if special.is_some() && ndigits != 0 {
            return None;
        }
        let numeric = Self {
            special,
            negative: sign == NEGATIVE,
            weight,
            dscale: usize::from(dscale),
            digits,
        };
        let well_formed = numeric.digits().all(|d| d <= 9999);
        well_formed.then_some(numeric)
    }

    /// The base-10000 digits, most significant first.
    pub(crate) fn digits(&self) -> impl Iterator<Item = u16> + '_ {
        self.digits
            .chunks_exact(2)
            .map(|d| u16::from_be_bytes([d[0], d[1]]))
    }

    /// The text `numeric_out` prints, which is what a `numeric` carries over
    /// logical replication in text mode.
    pub(crate) fn render(&self) -> String {
        if let Some(special) = self.special {
            return String::from(special);
        }
        let digit = |index: i64| -> u16 {
            usize::try_from(index)
                .ok()
                .and_then(|i| self.digits.get(2 * i..2 * i + 2))
                .map_or(0, |d| u16::from_be_bytes([d[0], d[1]]))
        };

        let mut out = String::with_capacity(self.digits.len() * 2 + self.dscale + 3);
        if self.negative {
            out.push('-');
        }
        if self.weight < 0 {
            out.push('0');
        } else {
            write!(out, "{}", digit(0)).unwrap();
            for index in 1..=self.weight {
                write!(out, "{:04}", digit(index)).unwrap();
            }
        }
        if self.dscale > 0 {
            out.push('.');
            let fraction_start = out.len();
            let mut index = self.weight + 1;
            while out.len() - fraction_start < self.dscale {
                write!(out, "{:04}", digit(index)).unwrap();
                index += 1;
            }
            out.truncate(fraction_start + self.dscale);
        }
        out
    }
}

/// Decode a PostgreSQL binary `json` or `jsonb` into [`Value::Text`].
///
/// # Errors
///
/// As [`pg_json_binary_text`].
pub(crate) fn decode_pg_json_binary<S, B>(
    column_name: &str,
    wire_type: WireType,
    bytes: &[u8],
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String>,
{
    pg_json_binary_text(column_name, wire_type, bytes)
        .map(|text| Value::Text(S::from(text.to_string())))
}

/// The JSON text of a PostgreSQL binary `json` (the UTF-8 text) or `jsonb`
/// (a version byte `1`, then the UTF-8 text).
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for a `jsonb` version other than
/// `1`, [`DecodeError::InvalidUtf8`] for text that is not UTF-8.
pub(crate) fn pg_json_binary_text<'a>(
    column_name: &str,
    wire_type: WireType,
    bytes: &'a [u8],
) -> Result<&'a str, DecodeError> {
    let text = match (wire_type, bytes) {
        (WireType::Jsonb, [1, text @ ..]) => text,
        (WireType::Jsonb, _) => {
            return Err(DecodeError::WrongPayloadKind {
                column: column_name.to_string(),
                expected: "jsonb binary version 1",
                actual: "other jsonb binary version",
            });
        }
        _ => bytes,
    };
    core::str::from_utf8(text).map_err(|_| DecodeError::InvalidUtf8 {
        column: column_name.to_string(),
    })
}
