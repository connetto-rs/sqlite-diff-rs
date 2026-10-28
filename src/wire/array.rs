//! Conversion of a PostgreSQL array, from its text or its binary
//! `array_send` form, into the JSON array text a
//! [`WireType::Array`](super::WireType::Array) column stores. The caller
//! decodes each element, normally through the scalar decoder of the
//! element type, and the element's [`Value`] is written as JSON.

#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
mod text;

use alloc::string::{String, ToString};
use core::fmt::Write;

use super::error::DecodeError;
use super::wire_type::{ArrayElement, WireType};
use crate::encoding::Value;
#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
pub(crate) use text::decode_text_array;

/// Most dimensions a PostgreSQL array has, its `MAXDIM`.
const MAX_DIMENSIONS: usize = 6;

/// The element type of a [`WireType::Array`] payload.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] when the payload's wire type is not an
/// array, which happens when the array decoder is registered under another
/// kind.
pub(crate) fn payload_element(
    column_name: &str,
    wire_type: WireType,
) -> Result<ArrayElement, DecodeError> {
    match wire_type {
        WireType::Array { element } => Ok(element),
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "an array wire type",
            actual: "a wire type that is not an array",
        }),
    }
}

/// The refusal of an array whose lower bound is not 1.
fn lower_bound_refusal(column: &str) -> DecodeError {
    DecodeError::UnrepresentableArray {
        column: column.to_string(),
        reason: "a lower bound other than 1",
    }
}

/// JSON array text under construction, compact as SQLite's `json_array`
/// writes it.
struct JsonArray<'c> {
    column: &'c str,
    out: String,
    /// Whether the next value or array follows a sibling.
    after_sibling: bool,
}

impl<'c> JsonArray<'c> {
    fn new(column: &'c str) -> Self {
        Self {
            column,
            out: String::new(),
            after_sibling: false,
        }
    }

    fn separate(&mut self) {
        if self.after_sibling {
            self.out.push(',');
        }
    }

    fn open(&mut self) {
        self.separate();
        self.out.push('[');
        self.after_sibling = false;
    }

    fn close(&mut self) {
        self.out.push(']');
        self.after_sibling = true;
    }

    /// Append one element's value: `NULL` as `null`, an integer as a
    /// number, a real as its shortest round-trip number and text as a
    /// string.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnrepresentableArray`] for a blob.
    fn push<S: AsRef<str>, B>(&mut self, value: &Value<S, B>) -> Result<(), DecodeError> {
        self.separate();
        self.after_sibling = true;
        match value {
            Value::Null => self.out.push_str("null"),
            Value::Integer(i) => {
                let _ = write!(self.out, "{i}");
            }
            Value::Real(f) => push_real(&mut self.out, *f),
            Value::Text(s) => push_string(&mut self.out, s.as_ref()),
            Value::Blob(_) => {
                return Err(DecodeError::UnrepresentableArray {
                    column: self.column.to_string(),
                    reason: "an element decoding to a blob",
                });
            }
        }
        Ok(())
    }

    fn finish<S: From<String>, B>(self) -> Value<S, B> {
        Value::Text(S::from(self.out))
    }
}

/// Append `f` as the shortest JSON number that parses back to it. NaN, which
/// no decoder yields, is `null`, and an infinity is `9.0e+999`, the overflowing
/// number SQLite writes for one and reads back as one.
fn push_real(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("null");
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "9.0e+999" } else { "-9.0e+999" });
    } else {
        let _ = write!(out, "{f:?}");
    }
}

/// Append `s` as a JSON string, escaped as SQLite's `json_array` escapes
/// it.
fn push_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Render PostgreSQL's binary `array_send` form as JSON array text, each
/// non-null element's bytes decoded by `element`.
///
/// The layout is the dimension count, a has-nulls flag and the element
/// type's OID as big-endian 32-bit fields, a length and lower bound per
/// dimension, then every element in row-major order as a 32-bit length,
/// `-1` for `NULL`, and that many bytes.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for bytes `array_send` would not
/// produce, [`DecodeError::UnrepresentableArray`] for a lower bound other
/// than 1 or a blob element, and any error of `element`.
pub(crate) fn decode_binary_array<'a, S, B>(
    column: &str,
    bytes: &'a [u8],
    mut element: impl FnMut(&'a [u8]) -> Result<Value<S, B>, DecodeError>,
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String> + AsRef<str>,
{
    let malformed = |actual| DecodeError::WrongPayloadKind {
        column: column.to_string(),
        expected: "PostgreSQL binary array",
        actual,
    };
    let mut rest = bytes;
    let mut header = || read_i32(&mut rest).ok_or_else(|| malformed("a truncated header"));
    let ndim = usize::try_from(header()?)
        .ok()
        .filter(|&n| n <= MAX_DIMENSIONS)
        .ok_or_else(|| malformed("a dimension count outside 0 to 6"))?;
    if !matches!(header()?, 0 | 1) {
        return Err(malformed("flags other than 0 or 1"));
    }
    header()?;
    let mut lengths = [0usize; MAX_DIMENSIONS];
    let mut lower_bound_is_one = true;
    for length in &mut lengths[..ndim] {
        *length =
            usize::try_from(header()?).map_err(|_| malformed("a negative dimension length"))?;
        lower_bound_is_one &= header()? == 1;
    }
    let lengths = &lengths[..ndim];
    let items = lengths
        .iter()
        .try_fold(usize::from(ndim > 0), |n, &len| n.checked_mul(len))
        .ok_or_else(|| malformed("dimensions overflowing the element count"))?;
    let mut json = JsonArray::new(column);
    let Some((&outer, inner)) = lengths.split_first().filter(|_| items > 0) else {
        if !rest.is_empty() {
            return Err(malformed("bytes after an empty array"));
        }
        json.open();
        json.close();
        return Ok(json.finish());
    };
    if !lower_bound_is_one {
        return Err(lower_bound_refusal(column));
    }
    if items > rest.len() / 4 {
        return Err(malformed("fewer elements than the dimensions declare"));
    }
    write_binary_level(&mut json, outer, inner, &mut rest, &mut element, &malformed)?;
    if !rest.is_empty() {
        return Err(malformed("bytes after the last element"));
    }
    Ok(json.finish())
}

/// Append one array level of `length` items, each an element when `inner`
/// is empty and otherwise an array of the `inner` lengths, reading the
/// elements from `rest`.
fn write_binary_level<'a, S, B>(
    json: &mut JsonArray<'_>,
    length: usize,
    inner: &[usize],
    rest: &mut &'a [u8],
    element: &mut impl FnMut(&'a [u8]) -> Result<Value<S, B>, DecodeError>,
    malformed: &impl Fn(&'static str) -> DecodeError,
) -> Result<(), DecodeError>
where
    S: AsRef<str>,
{
    json.open();
    for _ in 0..length {
        if let Some((&next, deeper)) = inner.split_first() {
            write_binary_level(json, next, deeper, rest, element, malformed)?;
        } else {
            let size = read_i32(rest).ok_or_else(|| malformed("a truncated element"))?;
            let value = if size == -1 {
                Value::Null
            } else {
                let size =
                    usize::try_from(size).map_err(|_| malformed("an element length below -1"))?;
                let (bytes, tail) = rest
                    .split_at_checked(size)
                    .ok_or_else(|| malformed("a truncated element"))?;
                *rest = tail;
                element(bytes)?
            };
            json.push(&value)?;
        }
    }
    json.close();
    Ok(())
}

/// Read a big-endian `i32` off the front of `rest`.
fn read_i32(rest: &mut &[u8]) -> Option<i32> {
    let (head, tail) = rest.split_first_chunk::<4>()?;
    *rest = tail;
    Some(i32::from_be_bytes(*head))
}
