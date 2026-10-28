//! [`Decoder`]: per-(source, semantic) conversion from a payload into
//! [`Value`](crate::encoding::Value), plus every built-in decoder unit
//! type the crate ships.

use super::error::DecodeError;
use super::source::WireSource;
use crate::encoding::Value;

/// Converts a per-column payload into a [`Value`].
///
/// Zero-sized unit-struct implementations (the crate's built-ins) cover
/// the common cases. User-defined stateful decoders carry their own
/// fields (lookup tables, compiled regexes, per-adapter configuration)
/// and implement the same trait shape.
pub trait Decoder<Src: WireSource, S, B> {
    /// Decode one payload.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] variant describing the failure. Every
    /// variant carries the offending column name so the outer
    /// [`WireAdapter`](super::adapter::WireAdapter) impl does not have to wrap.
    fn decode(&self, payload: Src::Payload<'_>) -> Result<Value<S, B>, DecodeError>;
}

// ------------------------------------------------------------------
// Built-in decoder unit types. Every one is a zero-sized marker whose
// `impl Decoder<Src, S, B>` for each supported `Src` lives in the format
// module (`pg_walstream`, `wal2json`, `maxwell`).
// ------------------------------------------------------------------

/// Decoder producing `Value::Null` regardless of the payload's non-null
/// contents. Registered internally when the payload's null discriminator
/// is set. Users normally do not register this directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullDecoder;

/// Decoder for boolean columns.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoolDecoder;

/// Decoder for integer columns fitting in `i64`.
#[derive(Debug, Clone, Copy, Default)]
pub struct IntDecoder;

/// Decoder for integer columns whose wire values may exceed `i64`. Values
/// fitting `i64` produce `Value::Integer`, values above produce
/// `Value::Text` with the base-10 digits preserved.
#[derive(Debug, Clone, Copy, Default)]
pub struct Int64OverflowToTextDecoder;

/// Decoder for floating-point columns.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealDecoder;

/// Decoder for text-affinity columns.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextDecoder;

/// Decoder for pg_walstream binary-mode BYTEA.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgByteaBinaryDecoder;

/// Decoder for pg_walstream text-mode BYTEA (`\xHEX` form).
#[derive(Debug, Clone, Copy, Default)]
pub struct PgByteaTextModeDecoder;

/// Decoder for MySQL binary-family columns (base64 on the wire).
#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlBinaryDecoder;

/// Decoder rendering UUID wire text into a 16-byte `Value::Blob`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidBlob16Decoder;

/// Decoder rendering UUID wire text into a verbatim `Value::Text(36)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidText36Decoder;

/// Decoder for `numeric`/`decimal` columns, storing Postgres's `numeric`
/// text (`-5000.0000`, `NaN`) as `Value::Text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecimalTextDecoder;

/// Decoder for [`WireType::ScaledDecimal`](super::WireType::ScaledDecimal)
/// columns, storing `value × 10^scale` as `Value::Integer` with the scale
/// read from the payload, so `12.50` at scale 2 is `1250`.
///
/// A value that is not a whole number of minor units fitting an `i64`,
/// `NaN` and an infinity are refused with
/// [`DecodeError::UnrepresentableDecimal`], never rounded.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecimalMinorUnitsDecoder;

/// Decoder for `timestamp` (without time zone), storing
/// `YYYY-MM-DD HH:MM:SS.ffffff` as `Value::Text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimestampDecoder;

/// Decoder for `timestamptz`, storing the instant in UTC as
/// `YYYY-MM-DD HH:MM:SS.ffffff+00:00`, which orders as text and which
/// SQLite's date functions read.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimestampTzDecoder;

/// Decoder for `date`, storing `YYYY-MM-DD` as `Value::Text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DateDecoder;

/// Decoder for `time`, storing `HH:MM:SS.ffffff`, and for `timetz`,
/// storing the UTC time `HH:MM:SS.ffffff+00:00`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeDecoder;

/// Decoder for `interval`, storing the ISO 8601 duration Postgres prints
/// under `IntervalStyle = iso_8601`, such as `P1Y2M3DT4H5M6.789S`.
#[derive(Debug, Clone, Copy, Default)]
pub struct IntervalDecoder;

/// Decoder for `json`/`jsonb`. Preserves the wire form verbatim as
/// `Value::Text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonVerbatimDecoder;

/// Decoder for `json`/`jsonb` that canonicalizes into sorted-key compact
/// JSON text.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonCanonicalDecoder;
