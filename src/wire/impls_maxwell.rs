//! `Decoder` implementations and `TypeMapDefaults` for the `Maxwell` source.

use alloc::string::ToString;
use alloc::vec::Vec;

use super::decoder::{
    BoolDecoder, DateDecoder, DecimalTextDecoder, Decoder, Int64OverflowToTextDecoder, IntDecoder,
    IntervalDecoder, JsonCanonicalDecoder, JsonVerbatimDecoder, MySqlBinaryDecoder, NullDecoder,
    PgByteaBinaryDecoder, PgByteaTextModeDecoder, RealDecoder, TextDecoder, TimeDecoder,
    TimestampDecoder, TimestampTzDecoder, UuidBlob16Decoder, UuidText36Decoder,
};
use super::error::DecodeError;
use super::temporal::{Temporal, decode_temporal_text, decode_utc_timestamptz_text};
use super::type_map::{TypeMap, TypeMapDefaults};
use super::wire_type::WireType;
use crate::encoding::Value;
use crate::maxwell::{Maxwell, MaxwellColumn};

impl super::json_decoders::AsJsonValue for crate::maxwell::MaxwellColumn<'_> {
    fn json_value(&self) -> &serde_json::Value {
        self.value
    }

    fn column_name(&self) -> &str {
        self.column_name
    }
}

impl<S, B> Decoder<Maxwell, S, B> for NullDecoder {
    fn decode(&self, _payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        Ok(Value::Null)
    }
}

// ------------------------------------------------------------------
// BoolDecoder
//
// Maxwell delivers MySQL `tinyint(1)` bool values as either JSON
// `true`/`false` or as integer 0/1 (config-dependent). Both are
// accepted. Null pass-through. Anything else -> WrongPayloadKind.
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for BoolDecoder {
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.value {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::Bool(b) => Ok(Value::Integer(i64::from(*b))),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(0) => Ok(Value::Integer(0)),
                Some(1) => Ok(Value::Integer(1)),
                _ => Err(DecodeError::WrongPayloadKind {
                    column: payload.column_name.to_string(),
                    expected: "JSON bool or number 0/1",
                    actual: "number outside {0, 1}",
                }),
            },
            serde_json::Value::String(_) => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "JSON bool or number 0/1",
                actual: "JSON string",
            }),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Err(DecodeError::WrongPayloadKind {
                    column: payload.column_name.to_string(),
                    expected: "JSON bool or number 0/1",
                    actual: "JSON array or object",
                })
            }
        }
    }
}

// ------------------------------------------------------------------
// IntDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for IntDecoder {
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_int(&payload)
    }
}

// ------------------------------------------------------------------
// Int64OverflowToTextDecoder
//
// Load-bearing for MySQL `bigint unsigned` columns whose wire values
// can exceed `i64::MAX`.
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for Int64OverflowToTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_int64_or_text(&payload)
    }
}

// ------------------------------------------------------------------
// RealDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for RealDecoder {
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_real(&payload)
    }
}

// ------------------------------------------------------------------
// TextDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for TextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_text(&payload)
    }
}

macro_rules! not_yet_impl {
    ($decoder:ty) => {
        impl<S, B> Decoder<Maxwell, S, B> for $decoder {
            fn decode(&self, _payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                Err(DecodeError::NotYetImplemented {
                    decoder: stringify!($decoder),
                })
            }
        }
    };
}

// ------------------------------------------------------------------
// MySqlBinaryDecoder
//
// Maxwell delivers MySQL binary-family columns as base64-encoded
// JSON strings. Base64 decode via the vendored helper.
// Null pass-through.
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for MySqlBinaryDecoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.value {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::String(s) => match super::bytes_helpers::decode_base64(s) {
                Ok(bytes) => Ok(Value::Blob(B::from(bytes))),
                Err(()) => Err(DecodeError::WrongPayloadKind {
                    column: payload.column_name.to_string(),
                    expected: "base64 string",
                    actual: "malformed base64",
                }),
            },
            _ => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "JSON base64 string",
                actual: "other JSON shape",
            }),
        }
    }
}

// ------------------------------------------------------------------
// UuidBlob16Decoder and UuidText36Decoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for UuidBlob16Decoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_uuid_blob(&payload)
    }
}

impl<S, B> Decoder<Maxwell, S, B> for UuidText36Decoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_uuid_text(&payload)
    }
}

// ------------------------------------------------------------------
// DecimalTextDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for DecimalTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_decimal(&payload)
    }
}

macro_rules! temporal_impl {
    ($decoder:ty, $kind:expr) => {
        impl<S, B> Decoder<Maxwell, S, B> for $decoder
        where
            S: From<alloc::string::String>,
        {
            fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                super::json_decoders::decode_json_str_with(&payload, |text| {
                    decode_temporal_text(payload.column_name, $kind, text)
                })
            }
        }
    };
}

temporal_impl!(DateDecoder, Temporal::Date);
temporal_impl!(TimeDecoder, Temporal::Time);
temporal_impl!(TimestampDecoder, Temporal::Timestamp);

impl<S, B> Decoder<Maxwell, S, B> for TimestampTzDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        // Maxwell prints a MySQL `TIMESTAMP` in UTC without an offset.
        super::json_decoders::decode_json_str_with(&payload, |text| {
            decode_utc_timestamptz_text(payload.column_name, text)
        })
    }
}

temporal_impl!(IntervalDecoder, Temporal::Interval);

// ------------------------------------------------------------------
// JsonVerbatimDecoder / JsonCanonicalDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Maxwell, S, B> for JsonVerbatimDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_verbatim(&payload)
    }
}

impl<S, B> Decoder<Maxwell, S, B> for JsonCanonicalDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: MaxwellColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_canonical(&payload)
    }
}

not_yet_impl!(PgByteaBinaryDecoder);
not_yet_impl!(PgByteaTextModeDecoder);

impl<S, B> TypeMapDefaults<S, B> for Maxwell
where
    S: From<alloc::string::String>,
    B: From<Vec<u8>>,
{
    fn defaults() -> TypeMap<Self, S, B> {
        // `Int` routes through `Int64OverflowToTextDecoder` so MySQL
        // `bigint unsigned` values above `i64::MAX` are preserved as
        // base-10 text rather than erroring. In-range integers still
        // produce `Value::Integer`.
        TypeMap::new()
            .with(WireType::Bool, BoolDecoder)
            .with(WireType::Int, Int64OverflowToTextDecoder)
            .with(WireType::Real, RealDecoder)
            .with(WireType::Text, TextDecoder)
            .with(WireType::Bytes, MySqlBinaryDecoder)
            .with(WireType::Uuid, UuidText36Decoder)
            .with(WireType::Decimal, DecimalTextDecoder)
            .with(WireType::Timestamp, TimestampDecoder)
            .with(WireType::TimestampTz, TimestampTzDecoder)
            .with(WireType::Date, DateDecoder)
            .with(WireType::Time, TimeDecoder)
            .with(WireType::Interval, IntervalDecoder)
            .with(WireType::Json, JsonVerbatimDecoder)
            .with(WireType::Jsonb, JsonVerbatimDecoder)
    }
}
