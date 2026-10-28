//! `Decoder` implementations and `TypeMapDefaults` for the `PgWalstream` source.

use alloc::borrow::Cow;
use alloc::string::ToString;
use alloc::vec::Vec;

use super::adapter::WireAdapter;
use super::array::{decode_binary_array, decode_text_array, payload_element};
use super::decoder::{
    ArrayJsonDecoder, BoolDecoder, DateDecoder, DecimalMinorUnitsDecoder, DecimalTextDecoder,
    Decoder, Int64OverflowToTextDecoder, IntDecoder, IntervalIso8601Decoder,
    IntervalPostgresDecoder, JsonCanonicalDecoder, JsonVerbatimDecoder, MySqlBinaryDecoder,
    NullDecoder, PgByteaBinaryDecoder, PgByteaTextModeDecoder, RealDecoder, TextDecoder,
    TimeDecoder, TimestampDecoder, TimestampTzDecoder, UuidBlob16Decoder, UuidText36Decoder,
};
use super::error::DecodeError;
use super::minor_units::{
    decode_pg_numeric_binary_minor_units, decode_text_minor_units, payload_scale,
};
use super::scalar_helpers::{
    decode_pg_bool_binary, decode_pg_int_binary, decode_pg_json_binary, decode_pg_numeric_binary,
    decode_pg_real_binary, normalize_real, pg_json_binary_text,
};
use super::temporal::{IntervalStyle, Temporal, decode_temporal_binary, decode_temporal_text};
use super::type_map::{TypeMap, TypeMapDefaults};
use super::wire_type::WireKind;
use crate::encoding::Value;
use crate::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};

impl<S, B> Decoder<PgWalstream, S, B> for NullDecoder {
    fn decode(&self, _payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        Ok(Value::Null)
    }
}

// ------------------------------------------------------------------
// BoolDecoder
//
// pg_walstream text mode: `"t"` -> 1, `"f"` -> 0.
// pg_walstream binary mode: single byte 0x01 -> 1, 0x00 -> 0.
// Null pass-through.
// Anything else -> WrongPayloadKind.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for BoolDecoder {
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => match payload.data.as_str() {
                Some("t") => Ok(Value::Integer(1)),
                Some("f") => Ok(Value::Integer(0)),
                other => Err(DecodeError::WrongPayloadKind {
                    column: payload.column_name.to_string(),
                    expected: "\"t\" or \"f\"",
                    actual: match other {
                        Some(_) => "arbitrary text",
                        None => "non-utf8 bytes",
                    },
                }),
            },
            ColumnValue::Binary(b) => decode_pg_bool_binary(payload.column_name, b.as_ref()),
        }
    }
}

// ------------------------------------------------------------------
// IntDecoder
//
// Text mode: base-10 parse via `str::parse::<i64>`. Overflow raises
// `IntegerOverflow`. Binary mode: width inferred from OID (int2 = 2
// bytes, int4 = 4, int8 = 8), big-endian. Null pass-through.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for IntDecoder {
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => {
                let s = payload
                    .data
                    .as_str()
                    .ok_or_else(|| DecodeError::InvalidUtf8 {
                        column: payload.column_name.to_string(),
                    })?;
                match s.parse::<i64>() {
                    Ok(i) => Ok(Value::Integer(i)),
                    Err(_)
                        if s.trim_start_matches('-')
                            .chars()
                            .all(|c| c.is_ascii_digit()) =>
                    {
                        Err(DecodeError::IntegerOverflow {
                            column: payload.column_name.to_string(),
                            digits: s.to_string(),
                        })
                    }
                    Err(_) => Err(DecodeError::WrongPayloadKind {
                        column: payload.column_name.to_string(),
                        expected: "base-10 signed integer",
                        actual: "non-numeric text",
                    }),
                }
            }
            ColumnValue::Binary(b) => decode_pg_int_binary(payload.column_name, b.as_ref()),
        }
    }
}

// ------------------------------------------------------------------
// Int64OverflowToTextDecoder
//
// pg_walstream rarely surfaces true bigint-unsigned overflow (Postgres
// has no such type), but this decoder tolerates the shape for
// symmetry with the Maxwell path. Wire text that does not fit i64
// stays as base-10 digits in Value::Text.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for Int64OverflowToTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => {
                let s = payload
                    .data
                    .as_str()
                    .ok_or_else(|| DecodeError::InvalidUtf8 {
                        column: payload.column_name.to_string(),
                    })?;
                match s.parse::<i64>() {
                    Ok(i) => Ok(Value::Integer(i)),
                    Err(_)
                        if s.trim_start_matches('-')
                            .chars()
                            .all(|c| c.is_ascii_digit()) =>
                    {
                        Ok(Value::Text(S::from(s.to_string())))
                    }
                    Err(_) => Err(DecodeError::WrongPayloadKind {
                        column: payload.column_name.to_string(),
                        expected: "base-10 integer text",
                        actual: "non-numeric text",
                    }),
                }
            }
            ColumnValue::Binary(_) => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "text-mode integer",
                actual: "binary payload",
            }),
        }
    }
}

// ------------------------------------------------------------------
// RealDecoder
//
// Text mode: `str::parse::<f64>` accepts "NaN"/"Infinity"/"-Infinity"
// and standard decimal / exponential forms. NaN becomes Null, matching
// the crate's `decode_value`.
// Binary mode: float4 = 4-byte big-endian IEEE 754, float8 = 8-byte.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for RealDecoder {
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => {
                let s = payload
                    .data
                    .as_str()
                    .ok_or_else(|| DecodeError::InvalidUtf8 {
                        column: payload.column_name.to_string(),
                    })?;
                match s.parse::<f64>() {
                    Ok(f) => Ok(normalize_real(f)),
                    Err(_) => Err(DecodeError::WrongPayloadKind {
                        column: payload.column_name.to_string(),
                        expected: "IEEE 754 float text",
                        actual: "non-numeric text",
                    }),
                }
            }
            ColumnValue::Binary(b) => decode_pg_real_binary(payload.column_name, b.as_ref()),
        }
    }
}

// ------------------------------------------------------------------
// TextDecoder
//
// Text mode: UTF-8 validate, pass through as Value::Text. Invalid
// UTF-8 raises `InvalidUtf8` rather than silently coercing to a Blob.
// Binary mode is rejected for text columns.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for TextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => {
                let s = payload
                    .data
                    .as_str()
                    .ok_or_else(|| DecodeError::InvalidUtf8 {
                        column: payload.column_name.to_string(),
                    })?;
                Ok(Value::Text(S::from(s.to_string())))
            }
            ColumnValue::Binary(_) => Err(DecodeError::WrongPayloadKind {
                column: payload.column_name.to_string(),
                expected: "UTF-8 text",
                actual: "binary payload",
            }),
        }
    }
}

// ------------------------------------------------------------------
// PgByteaBinaryDecoder
//
// Handles both `ColumnValue::Binary` (pass-through) and text-mode
// `\xHEX` (decoded via the vendored helper). Null pass-through.
// This is the recommended default for PG_BYTEA columns since it
// tolerates both transport modes.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for PgByteaBinaryDecoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Binary(b) => Ok(Value::Blob(B::from(b.to_vec()))),
            ColumnValue::Text(_) => {
                let s = payload
                    .data
                    .as_str()
                    .ok_or_else(|| DecodeError::InvalidUtf8 {
                        column: payload.column_name.to_string(),
                    })?;
                match super::bytes_helpers::decode_pg_hex_escape(s) {
                    Ok(bytes) => Ok(Value::Blob(B::from(bytes))),
                    Err(at) => Err(DecodeError::InvalidHexEscape {
                        column: payload.column_name.to_string(),
                        at,
                    }),
                }
            }
        }
    }
}

// ------------------------------------------------------------------
// UuidBlob16Decoder and UuidText36Decoder
//
// Both accept 36-character hyphenated and braced forms of a UUID
// wire text and produce `Value::Blob([u8; 16])` or `Value::Text(36)`
// respectively. Neither is registered in `defaults()`. Users pick
// per column.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgWalstream, S, B> for UuidBlob16Decoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        decode_pg_uuid_to_blob(payload)
    }
}

impl<S, B> Decoder<PgWalstream, S, B> for UuidText36Decoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        decode_pg_uuid_to_text(payload)
    }
}

fn decode_pg_uuid_to_blob<S, B>(payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError>
where
    B: From<Vec<u8>>,
{
    match payload.data {
        ColumnValue::Null => Ok(Value::Null),
        ColumnValue::Text(_) => {
            let s = payload
                .data
                .as_str()
                .ok_or_else(|| DecodeError::InvalidUtf8 {
                    column: payload.column_name.to_string(),
                })?;
            match super::uuid_helpers::parse_uuid(s) {
                Ok(bytes) => Ok(Value::Blob(B::from(bytes.to_vec()))),
                Err(source_len) => Err(DecodeError::InvalidUuid {
                    column: payload.column_name.to_string(),
                    source_len,
                }),
            }
        }
        ColumnValue::Binary(_) => Err(DecodeError::WrongPayloadKind {
            column: payload.column_name.to_string(),
            expected: "UUID text form",
            actual: "binary payload",
        }),
    }
}

fn decode_pg_uuid_to_text<S, B>(payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError>
where
    S: From<alloc::string::String>,
{
    match payload.data {
        ColumnValue::Null => Ok(Value::Null),
        ColumnValue::Text(_) => {
            let s = payload
                .data
                .as_str()
                .ok_or_else(|| DecodeError::InvalidUtf8 {
                    column: payload.column_name.to_string(),
                })?;
            match super::uuid_helpers::preserve_or_canonicalize_uuid_text(s) {
                Ok(canonical) => Ok(Value::Text(S::from(canonical))),
                Err(source_len) => Err(DecodeError::InvalidUuid {
                    column: payload.column_name.to_string(),
                    source_len,
                }),
            }
        }
        ColumnValue::Binary(_) => Err(DecodeError::WrongPayloadKind {
            column: payload.column_name.to_string(),
            expected: "UUID text form",
            actual: "binary payload",
        }),
    }
}

/// The UTF-8 text of a text-mode payload.
fn text_of<'a>(payload: &PgWalstreamColumn<'a>) -> Result<&'a str, DecodeError> {
    payload
        .data
        .as_str()
        .ok_or_else(|| DecodeError::InvalidUtf8 {
            column: payload.column_name.to_string(),
        })
}

impl<S, B> Decoder<PgWalstream, S, B> for DecimalTextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => Ok(Value::Text(S::from(text_of(&payload)?.to_string()))),
            ColumnValue::Binary(b) => decode_pg_numeric_binary(payload.column_name, b),
        }
    }
}

impl<S, B> Decoder<PgWalstream, S, B> for DecimalMinorUnitsDecoder {
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let scale = payload_scale(payload.column_name, payload.wire_type)?;
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => {
                decode_text_minor_units(payload.column_name, text_of(&payload)?, scale)
            }
            ColumnValue::Binary(b) => {
                decode_pg_numeric_binary_minor_units(payload.column_name, b, scale)
            }
        }
    }
}

impl<S, B> Decoder<PgWalstream, S, B> for ArrayJsonDecoder
where
    S: From<alloc::string::String> + AsRef<str>,
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        self.decode_within(payload, &PgWalstream::defaults())
    }

    fn decode_within(
        &self,
        payload: PgWalstreamColumn<'_>,
        adapter: &dyn WireAdapter<PgWalstream, S, B>,
    ) -> Result<Value<S, B>, DecodeError> {
        let wire_type = payload_element(payload.column_name, payload.wire_type)?.wire_type();
        let element = |data: &ColumnValue| {
            adapter.decode(PgWalstreamColumn {
                column_name: payload.column_name,
                wire_type,
                data,
            })
        };
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(b) => {
                decode_text_array(payload.column_name, text_of(&payload)?, |text| {
                    element(&match text {
                        Cow::Borrowed(text) => {
                            ColumnValue::text_bytes(b.slice_ref(text.as_bytes()))
                        }
                        Cow::Owned(text) => ColumnValue::text(&text),
                    })
                })
            }
            ColumnValue::Binary(b) => decode_binary_array(payload.column_name, b, |raw| {
                element(&ColumnValue::binary_bytes(b.slice_ref(raw)))
            }),
        }
    }
}

// PgByteaTextModeDecoder and MySqlBinaryDecoder are wire-format
// specific to wal2json / maxwell respectively; on pg_walstream they
// stay NotYetImplemented.

macro_rules! not_yet_impl {
    ($decoder:ty) => {
        impl<S, B> Decoder<PgWalstream, S, B> for $decoder {
            fn decode(&self, _payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                Err(DecodeError::NotYetImplemented {
                    decoder: stringify!($decoder),
                })
            }
        }
    };
}

macro_rules! temporal_impl {
    ($decoder:ty, $kind:expr) => {
        impl<S, B> Decoder<PgWalstream, S, B> for $decoder
        where
            S: From<alloc::string::String>,
        {
            fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                match payload.data {
                    ColumnValue::Null => Ok(Value::Null),
                    ColumnValue::Text(_) => {
                        decode_temporal_text(payload.column_name, $kind, text_of(&payload)?)
                    }
                    ColumnValue::Binary(b) => decode_temporal_binary(payload.column_name, $kind, b),
                }
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

impl<S, B> Decoder<PgWalstream, S, B> for JsonVerbatimDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.data {
            ColumnValue::Null => Ok(Value::Null),
            ColumnValue::Text(_) => Ok(Value::Text(S::from(text_of(&payload)?.to_string()))),
            ColumnValue::Binary(b) => {
                decode_pg_json_binary(payload.column_name, payload.wire_type, b)
            }
        }
    }
}

impl<S, B> Decoder<PgWalstream, S, B> for JsonCanonicalDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgWalstreamColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let text = match payload.data {
            ColumnValue::Null => return Ok(Value::Null),
            ColumnValue::Text(_) => text_of(&payload)?,
            ColumnValue::Binary(b) => {
                pg_json_binary_text(payload.column_name, payload.wire_type, b)?
            }
        };
        let canon = crate::wire::json_helpers::canonicalize_string(text);
        Ok(Value::Text(S::from(canon)))
    }
}

not_yet_impl!(PgByteaTextModeDecoder);
not_yet_impl!(MySqlBinaryDecoder);

impl<S, B> TypeMapDefaults<S, B> for PgWalstream
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
            .with(WireKind::Bytes, PgByteaBinaryDecoder)
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
