//! `Decoder` implementations and `TypeMapDefaults` for the `Wal2Json` source.

use alloc::borrow::Cow;
use alloc::string::ToString;
use alloc::vec::Vec;

use super::adapter::WireAdapter;
use super::array::{decode_text_array, payload_element};
use super::decoder::{
    ArrayJsonDecoder, BoolDecoder, DateDecoder, DecimalMinorUnitsDecoder, DecimalTextDecoder,
    Decoder, Int64OverflowToTextDecoder, IntDecoder, IntervalIso8601Decoder,
    IntervalPostgresDecoder, JsonCanonicalDecoder, JsonVerbatimDecoder, MySqlBinaryDecoder,
    NullDecoder, PgByteaBinaryDecoder, PgByteaTextModeDecoder, RealDecoder, TextDecoder,
    TimeDecoder, TimestampDecoder, TimestampTzDecoder, UuidBlob16Decoder, UuidText36Decoder,
};
use super::error::DecodeError;
use super::temporal::{IntervalStyle, Temporal, decode_temporal_text};
use super::type_map::{TypeMap, TypeMapDefaults};
use super::wire_type::{ArrayElement, WireKind};
use crate::encoding::Value;
use crate::wal2json::{Wal2Json, Wal2JsonColumn};

impl super::json_decoders::AsJsonValue for crate::wal2json::Wal2JsonColumn<'_> {
    fn json_value(&self) -> &serde_json::Value {
        self.value
    }

    fn column_name(&self) -> &str {
        self.column_name
    }
}

// ------------------------------------------------------------------
// NullDecoder: trivial, always Null.
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for NullDecoder {
    fn decode(&self, _payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        Ok(Value::Null)
    }
}

// ------------------------------------------------------------------
// BoolDecoder
//
// wal2json v2 delivers PG booleans as JSON `true`/`false`. `null` maps
// to Value::Null. Anything else -> WrongPayloadKind.
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for BoolDecoder {
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.value {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::Bool(b) => Ok(Value::Integer(i64::from(*b))),
            serde_json::Value::Number(_) => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "JSON boolean",
                actual: "JSON number",
            }),
            serde_json::Value::String(_) => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "JSON boolean",
                actual: "JSON string",
            }),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Err(DecodeError::WrongPayloadKind {
                    column: payload.column_name.to_string(),
                    expected: "JSON boolean",
                    actual: "JSON array or object",
                })
            }
        }
    }
}

// ------------------------------------------------------------------
// IntDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for IntDecoder {
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_int(&payload)
    }
}

// ------------------------------------------------------------------
// Int64OverflowToTextDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for Int64OverflowToTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_int64_or_text(&payload)
    }
}

// ------------------------------------------------------------------
// RealDecoder
//
// NaN becomes Null, matching the crate's `decode_value`.
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for RealDecoder {
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_real(&payload)
    }
}

// ------------------------------------------------------------------
// TextDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for TextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_text(&payload)
    }
}

// ------------------------------------------------------------------
// Skeleton impls for the schema-aware decoders. Populated per phase.
// ------------------------------------------------------------------

macro_rules! not_yet_impl {
    ($decoder:ty) => {
        impl<S, B> Decoder<Wal2Json, S, B> for $decoder {
            fn decode(&self, _payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                Err(DecodeError::NotYetImplemented {
                    decoder: stringify!($decoder),
                })
            }
        }
    };
}

// ------------------------------------------------------------------
// PgByteaTextModeDecoder
//
// wal2json v2 emits PG BYTEA as a JSON string of bare lowercase hex
// (no `\x` prefix). An optional `\x` prefix is also accepted.
// Null pass-through.
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for PgByteaTextModeDecoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.value {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::String(s) => {
                match super::bytes_helpers::decode_wal2json_bytea_hex(s) {
                    Ok(bytes) => Ok(Value::Blob(B::from(bytes))),
                    Err(at) => Err(DecodeError::InvalidHexEscape {
                        column: payload.column_name.to_string(),
                        at,
                    }),
                }
            }
            _ => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "JSON string of hex bytes",
                actual: "other JSON shape",
            }),
        }
    }
}

// ------------------------------------------------------------------
// UuidBlob16Decoder and UuidText36Decoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for UuidBlob16Decoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_uuid_blob(&payload)
    }
}

impl<S, B> Decoder<Wal2Json, S, B> for UuidText36Decoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_uuid_text(&payload)
    }
}

// ------------------------------------------------------------------
// DecimalTextDecoder
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for DecimalTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_decimal(&payload)
    }
}

impl<S, B> Decoder<Wal2Json, S, B> for DecimalMinorUnitsDecoder {
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let scale = super::minor_units::payload_scale(payload.column_name, payload.wire_type)?;
        super::json_decoders::decode_json_minor_units(&payload, scale)
    }
}

impl<S, B> Decoder<Wal2Json, S, B> for ArrayJsonDecoder
where
    S: From<alloc::string::String> + AsRef<str>,
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        self.decode_within(payload, &Wal2Json::defaults())
    }

    fn decode_within(
        &self,
        payload: Wal2JsonColumn<'_>,
        adapter: &dyn WireAdapter<Wal2Json, S, B>,
    ) -> Result<Value<S, B>, DecodeError> {
        let element = payload_element(payload.column_name, payload.wire_type)?;
        super::json_decoders::decode_json_str_with(&payload, |text| {
            decode_text_array(payload.column_name, text, |text| {
                adapter.decode(Wal2JsonColumn {
                    column_name: payload.column_name,
                    wire_type: element.wire_type(),
                    value: &scalar_json(element, text),
                })
            })
        })
    }
}

/// The JSON wal2json writes for a scalar column of `element`'s type whose
/// text output is `text`: a boolean as `true` or `false`, a number bare
/// unless it is `NaN` or an infinity, which become `null`, and anything else
/// as a string.
fn scalar_json(element: ArrayElement, text: Cow<'_, str>) -> serde_json::Value {
    let starts_with = |prefix: &str| {
        text.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    match element {
        ArrayElement::Bool if text == "t" => serde_json::Value::Bool(true),
        ArrayElement::Bool if text == "f" => serde_json::Value::Bool(false),
        ArrayElement::Int | ArrayElement::Real | ArrayElement::Decimal
            if ["NaN", "Infinity", "-Infinity"]
                .into_iter()
                .any(starts_with) =>
        {
            serde_json::Value::Null
        }
        ArrayElement::Int | ArrayElement::Real | ArrayElement::Decimal => text.parse().map_or_else(
            |_| serde_json::Value::String(text.into_owned()),
            serde_json::Value::Number,
        ),
        _ => serde_json::Value::String(text.into_owned()),
    }
}

macro_rules! temporal_impl {
    ($decoder:ty, $kind:expr) => {
        impl<S, B> Decoder<Wal2Json, S, B> for $decoder
        where
            S: From<alloc::string::String>,
        {
            fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
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
temporal_impl!(TimestampTzDecoder, Temporal::TimestampTz);
temporal_impl!(
    IntervalIso8601Decoder,
    Temporal::Interval(IntervalStyle::Iso8601)
);
temporal_impl!(
    IntervalPostgresDecoder,
    Temporal::Interval(IntervalStyle::Postgres)
);

// ------------------------------------------------------------------
// JsonVerbatimDecoder / JsonCanonicalDecoder
//
// Verbatim: serialize Object/Array via serde_json::to_string (compact)
// or pass string sources through unchanged. Canonical: sort keys
// recursively, then serialize compactly.
// ------------------------------------------------------------------

impl<S, B> Decoder<Wal2Json, S, B> for JsonVerbatimDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_verbatim(&payload)
    }
}

impl<S, B> Decoder<Wal2Json, S, B> for JsonCanonicalDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: Wal2JsonColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        super::json_decoders::decode_json_canonical(&payload)
    }
}

not_yet_impl!(PgByteaBinaryDecoder);
not_yet_impl!(MySqlBinaryDecoder);

// ------------------------------------------------------------------
// TypeMapDefaults.
// ------------------------------------------------------------------

impl<S, B> TypeMapDefaults<S, B> for Wal2Json
where
    S: From<alloc::string::String> + AsRef<str>,
    B: From<Vec<u8>>,
{
    fn defaults() -> TypeMap<Self, S, B> {
        TypeMap::new()
            .with(WireKind::Bool, BoolDecoder)
            .with(WireKind::Int, IntDecoder)
            .with(WireKind::Real, RealDecoder)
            .with(WireKind::Text, TextDecoder)
            .with(WireKind::Bytes, PgByteaTextModeDecoder)
            .with(WireKind::Uuid, UuidText36Decoder)
            .with(WireKind::Decimal, DecimalTextDecoder)
            .with(WireKind::ScaledDecimal, DecimalMinorUnitsDecoder)
            .with(WireKind::Timestamp, TimestampDecoder)
            .with(WireKind::TimestampTz, TimestampTzDecoder)
            .with(WireKind::Date, DateDecoder)
            .with(WireKind::Time, TimeDecoder)
            .with(WireKind::Interval, IntervalIso8601Decoder)
            .with(WireKind::Json, JsonVerbatimDecoder)
            .with(WireKind::Jsonb, JsonVerbatimDecoder)
            .with(WireKind::Array, ArrayJsonDecoder)
    }
}
