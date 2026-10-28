//! Conversion of a pgvector `vector`, from its text or its binary send
//! form, into the float32 `BLOB` a
//! [`WireType::Vector`](super::WireType::Vector) column stores, the
//! elements as little-endian IEEE 754 `f32` with no header, the layout
//! sqlite-vec reads.

use alloc::string::ToString;
use alloc::vec::Vec;

use super::error::DecodeError;
use super::wire_type::WireType;
use crate::encoding::Value;

/// The declared dimension of a [`WireType::Vector`] payload, `None` when
/// the column declares none.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] when the payload's wire type is not a
/// vector, which happens when the vector decoder is registered under
/// another kind.
pub(crate) fn payload_dimensions(
    column_name: &str,
    wire_type: WireType,
) -> Result<Option<u32>, DecodeError> {
    match wire_type {
        WireType::Vector { dimensions } => Ok(dimensions),
        _ => Err(DecodeError::WrongPayloadKind {
            column: column_name.to_string(),
            expected: "a vector wire type",
            actual: "a wire type that is not a vector",
        }),
    }
}

/// Appends `element` to the float32 blob, or returns `None` when it is not
/// finite, as pgvector requires of every element.
fn push_finite(blob: &mut Vec<u8>, element: f32) -> Option<()> {
    if !element.is_finite() {
        return None;
    }
    blob.extend_from_slice(&element.to_le_bytes());
    Some(())
}

/// Wraps the float32 blob of `count` elements, refusing a count other
/// than the declared `dimensions`.
fn checked_blob<S, B>(
    column_name: &str,
    blob: Vec<u8>,
    count: usize,
    dimensions: Option<u32>,
) -> Result<Value<S, B>, DecodeError>
where
    B: From<Vec<u8>>,
{
    match dimensions {
        Some(expected) if usize::try_from(expected).ok() != Some(count) => {
            Err(DecodeError::VectorDimensionMismatch {
                column: column_name.to_string(),
                expected,
                actual: count,
            })
        }
        _ => Ok(Value::Blob(B::from(blob))),
    }
}

/// The float32 blob of pgvector text and its element count, `None` for
/// text pgvector's `vector_in` would refuse: anything but a bracketed,
/// comma-separated, non-empty list of finite `float4`, with whitespace
/// allowed around the brackets and the elements.
#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
fn text_blob(text: &str) -> Option<(Vec<u8>, usize)> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    let count = inner.bytes().filter(|&b| b == b',').count() + 1;
    let mut blob = Vec::with_capacity(count * 4);
    for element in inner.split(',') {
        push_finite(&mut blob, element.trim().parse().ok()?)?;
    }
    Some((blob, count))
}

/// Decodes pgvector text, as pgoutput and wal2json carry it, into the
/// float32 blob.
///
/// # Errors
///
/// [`DecodeError::VectorDimensionMismatch`] for a length other than the
/// declared `dimensions`, and [`DecodeError::WrongPayloadKind`] for text
/// that is not a pgvector literal of finite elements.
#[cfg(any(feature = "wal2json", feature = "pg-walstream"))]
pub(crate) fn decode_pgvector_text<S, B>(
    column_name: &str,
    text: &str,
    dimensions: Option<u32>,
) -> Result<Value<S, B>, DecodeError>
where
    B: From<Vec<u8>>,
{
    let (blob, count) = text_blob(text).ok_or_else(|| DecodeError::WrongPayloadKind {
        column: column_name.to_string(),
        expected: "pgvector text of finite elements",
        actual: "other text",
    })?;
    checked_blob(column_name, blob, count, dimensions)
}

/// The float32 blob of pgvector's binary send form and its element count,
/// `None` for bytes `vector_recv` would refuse. The form is the dimension
/// and an unused field as big-endian `u16`, the unused one zero, then that
/// many finite big-endian `float4`, at least one.
fn binary_blob(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    let (header, elements) = bytes.split_first_chunk::<4>()?;
    let count = usize::from(u16::from_be_bytes([header[0], header[1]]));
    let unused = u16::from_be_bytes([header[2], header[3]]);
    if count == 0 || unused != 0 || elements.len() != count * 4 {
        return None;
    }
    let mut blob = Vec::with_capacity(elements.len());
    for element in elements.chunks_exact(4) {
        let bits = [element[0], element[1], element[2], element[3]];
        push_finite(&mut blob, f32::from_be_bytes(bits))?;
    }
    Some((blob, count))
}

/// Decodes pgvector's binary send form, as a binary query result and
/// binary pgoutput carry it, into the float32 blob.
///
/// # Errors
///
/// [`DecodeError::VectorDimensionMismatch`] for a length other than the
/// declared `dimensions`, and [`DecodeError::WrongPayloadKind`] for bytes
/// pgvector's `vector_send` would not produce.
pub(crate) fn decode_pgvector_binary<S, B>(
    column_name: &str,
    bytes: &[u8],
    dimensions: Option<u32>,
) -> Result<Value<S, B>, DecodeError>
where
    B: From<Vec<u8>>,
{
    let (blob, count) = binary_blob(bytes).ok_or_else(|| DecodeError::WrongPayloadKind {
        column: column_name.to_string(),
        expected: "pgvector binary of finite elements",
        actual: "other binary contents",
    })?;
    checked_blob(column_name, blob, count, dimensions)
}
