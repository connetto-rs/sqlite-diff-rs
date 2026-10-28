//! [`DecodeError`]: shared failure mode for every decoder in the
//! [`wire`](super) module.
//!
//! Per-format `ConversionError` types (in `pg_walstream::ConversionError`,
//! `wal2json::ConversionError`, `maxwell::ConversionError`) wrap this
//! via a `Decode(DecodeError)` variant so users can pattern-match on
//! the outer error and route to a common inner arm.

use alloc::string::String;

/// Failure modes shared across every `wire::Decoder` implementation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The [`TypeMap`](super::TypeMap) contains no decoder for the
    /// wire type carried by this column.
    #[error("no decoder registered for column {column:?}")]
    NoDecoderForType {
        /// Column whose type was unrecognized.
        column: String,
    },

    /// The decoder skeleton exists but its implementation is not yet populated.
    #[error("decoder {decoder} is not yet implemented")]
    NotYetImplemented {
        /// The unpopulated decoder's type name.
        decoder: &'static str,
    },

    /// Column payload was expected to be valid UTF-8 and was not.
    #[error("column {column:?} carried non-UTF-8 bytes")]
    InvalidUtf8 {
        /// Offending column name.
        column: String,
    },

    /// Column payload did not parse as a UUID (36-char hyphenated or
    /// braced form).
    #[error("column {column:?} carried a malformed UUID (source length {source_len})")]
    InvalidUuid {
        /// Offending column name.
        column: String,
        /// Length of the string the decoder tried to parse.
        source_len: usize,
    },

    /// Column payload was expected to be a `\xHEX` PG BYTEA escape and
    /// contained an invalid character at the reported byte offset.
    #[error("column {column:?} carried a malformed \\x hex escape at offset {at}")]
    InvalidHexEscape {
        /// Offending column name.
        column: String,
        /// Zero-based byte offset of the first invalid character.
        at: usize,
    },

    /// Integer payload exceeded `i64` range and its shape (MySQL
    /// `bigint unsigned`, PG `numeric` cast, ...) did not permit a
    /// silent fallback to `Text`.
    #[error("column {column:?} integer digits overflowed i64: {digits}")]
    IntegerOverflow {
        /// Offending column name.
        column: String,
        /// The raw digit string as received on the wire.
        digits: String,
    },

    /// JSON payload failed to serialize into the target text form.
    /// Only observable when a JSON decoder receives a value that
    /// `serde_json` cannot round-trip through `to_string`.
    #[error("column {column:?} JSON value did not serialize: {error}")]
    JsonNotSerializable {
        /// Offending column name.
        column: String,
        /// `serde_json` error message.
        error: String,
    },

    /// Payload kind was not what this decoder expected. Emitted when a
    /// decoder registered for one shape (e.g. JSON string) receives a
    /// different one (e.g. JSON object).
    #[error("column {column:?} payload kind mismatch: expected {expected}, got {actual}")]
    WrongPayloadKind {
        /// Offending column name.
        column: String,
        /// Expected payload kind name.
        expected: &'static str,
        /// Actual payload kind name.
        actual: &'static str,
    },

    /// Temporal value outside the replica's text form, which is `infinity`,
    /// `-infinity`, a year outside 1 to 9999 (BC included), a `time` of
    /// `24:00:00`, or an infinite `interval`.
    #[error("column {column:?} carried {value}, which a replica cannot hold")]
    UnrepresentableTemporal {
        /// Offending column name.
        column: String,
        /// The value as received, or its binary rendering.
        value: String,
    },

    /// Decimal that is not a whole number of minor units at the column's
    /// scale fitting an `i64`, which is a value needing rounding, one out
    /// of `i64` range, `NaN`, or an infinity.
    #[error(
        "column {column:?} carried {value}, which is not a whole i64 of minor units at scale {scale}"
    )]
    UnrepresentableDecimal {
        /// Offending column name.
        column: String,
        /// The value as received, or its binary rendering.
        value: String,
        /// The scale the column declares.
        scale: u32,
    },

    /// Array that JSON array text cannot hold, which is one with a lower
    /// bound other than 1 or with an element decoding to a `Blob`.
    #[error("column {column:?} carried an array with {reason}, which JSON cannot hold")]
    UnrepresentableArray {
        /// Offending column name.
        column: String,
        /// What JSON cannot hold.
        reason: &'static str,
    },

    /// Free-form failure emitted by user-supplied decoders.
    #[error("column {column:?}: {message}")]
    Custom {
        /// Offending column name.
        column: String,
        /// Failure message from the decoder.
        message: String,
    },

    /// Vector whose element count differs from the dimension the column
    /// declares.
    #[error("column {column:?} carried a vector of {actual} elements, expected {expected}")]
    VectorDimensionMismatch {
        /// Offending column name.
        column: String,
        /// The dimension the column declares.
        expected: u32,
        /// The number of elements the value carried.
        actual: usize,
    },
}
