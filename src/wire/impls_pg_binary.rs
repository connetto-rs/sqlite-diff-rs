//! `Decoder` implementations and `TypeMapDefaults` for the [`PgBinary`]
//! source: decoding PostgreSQL binary result fields straight into
//! [`Value`].
//!
//! The Postgres binary send format is the same whether it arrives over
//! logical replication in binary mode or as a binary query result, so
//! these decoders share the binary arms of the `PgWalstream` impls and
//! produce byte-identical [`Value`]s.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::adapter::WireAdapter;
use super::array::{decode_binary_array, payload_element};
use super::decoder::{
    ArrayJsonDecoder, BoolDecoder, DateDecoder, DecimalMinorUnitsDecoder, DecimalTextDecoder,
    Decoder, IntDecoder, IntervalIso8601Decoder, IntervalPostgresDecoder, JsonVerbatimDecoder,
    NullDecoder, PgByteaBinaryDecoder, RealDecoder, TextDecoder, TimeDecoder, TimestampDecoder,
    TimestampTzDecoder, UuidBlob16Decoder, VectorFloat32Decoder,
};
use super::error::DecodeError;
use super::minor_units::{decode_pg_numeric_binary_minor_units, payload_scale};
use super::scalar_helpers::{
    decode_pg_bool_binary, decode_pg_int_binary, decode_pg_json_binary, decode_pg_numeric_binary,
    decode_pg_real_binary,
};
use super::source::{PgBinary, PgBinaryColumn};
use super::temporal::{IntervalStyle, Temporal, decode_temporal_binary};
use super::type_map::{TypeMap, TypeMapDefaults};
use super::vector::{decode_pgvector_binary, payload_dimensions};
use super::wire_type::WireKind;
use crate::encoding::Value;

impl<S, B> Decoder<PgBinary, S, B> for NullDecoder {
    fn decode(&self, _payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        Ok(Value::Null)
    }
}

// ------------------------------------------------------------------
// BoolDecoder: single byte 0x01 -> 1, 0x00 -> 0. Null pass-through.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for BoolDecoder {
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_bool_binary(payload.column_name, bytes),
        }
    }
}

// ------------------------------------------------------------------
// IntDecoder: int2/int4/int8 as 2/4/8-byte big-endian two's complement.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for IntDecoder {
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_int_binary(payload.column_name, bytes),
        }
    }
}

// ------------------------------------------------------------------
// RealDecoder: float4/float8 as 4/8-byte big-endian IEEE 754. NaN
// becomes Null, matching `decode_value`.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for RealDecoder {
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_real_binary(payload.column_name, bytes),
        }
    }
}

// ------------------------------------------------------------------
// TextDecoder: UTF-8 bytes verbatim. Invalid UTF-8 -> InvalidUtf8.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for TextDecoder
where
    S: From<alloc::string::String>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let Some(bytes) = payload.raw else {
            return Ok(Value::Null);
        };
        match core::str::from_utf8(bytes) {
            Ok(s) => Ok(Value::Text(S::from(s.to_string()))),
            Err(_) => Err(DecodeError::InvalidUtf8 {
                column: payload.column_name.to_string(),
            }),
        }
    }
}

// ------------------------------------------------------------------
// PgByteaBinaryDecoder: raw bytes verbatim into Value::Blob.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for PgByteaBinaryDecoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let Some(bytes) = payload.raw else {
            return Ok(Value::Null);
        };
        Ok(Value::Blob(B::from(bytes.to_vec())))
    }
}

// ------------------------------------------------------------------
// UuidBlob16Decoder: the source bytes are already the 16 raw uuid
// bytes, taken verbatim when the length is 16. Any other length errors.
// The output matches the CDC path's 16-byte blob, which is the point.
// ------------------------------------------------------------------

impl<S, B> Decoder<PgBinary, S, B> for UuidBlob16Decoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let Some(bytes) = payload.raw else {
            return Ok(Value::Null);
        };
        if bytes.len() == 16 {
            Ok(Value::Blob(B::from(bytes.to_vec())))
        } else {
            Err(DecodeError::InvalidUuid {
                column: payload.column_name.to_string(),
                source_len: bytes.len(),
            })
        }
    }
}

impl<S, B> Decoder<PgBinary, S, B> for DecimalTextDecoder
where
    S: From<String>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_numeric_binary(payload.column_name, bytes),
        }
    }
}

impl<S, B> Decoder<PgBinary, S, B> for DecimalMinorUnitsDecoder {
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let scale = payload_scale(payload.column_name, payload.wire_type)?;
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_numeric_binary_minor_units(payload.column_name, bytes, scale),
        }
    }
}

impl<S, B> Decoder<PgBinary, S, B> for ArrayJsonDecoder
where
    S: From<String> + AsRef<str>,
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        self.decode_within(payload, &PgBinary::defaults())
    }

    fn decode_within(
        &self,
        payload: PgBinaryColumn<'_>,
        adapter: &dyn WireAdapter<PgBinary, S, B>,
    ) -> Result<Value<S, B>, DecodeError> {
        let wire_type = payload_element(payload.column_name, payload.wire_type)?.wire_type();
        let Some(bytes) = payload.raw else {
            return Ok(Value::Null);
        };
        decode_binary_array(payload.column_name, bytes, |raw| {
            adapter.decode(PgBinaryColumn {
                column_name: payload.column_name,
                wire_type,
                raw: Some(raw),
            })
        })
    }
}

impl<S, B> Decoder<PgBinary, S, B> for VectorFloat32Decoder
where
    B: From<Vec<u8>>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        let dimensions = payload_dimensions(payload.column_name, payload.wire_type)?;
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pgvector_binary(payload.column_name, bytes, dimensions),
        }
    }
}

impl<S, B> Decoder<PgBinary, S, B> for JsonVerbatimDecoder
where
    S: From<String>,
{
    fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
        match payload.raw {
            None => Ok(Value::Null),
            Some(bytes) => decode_pg_json_binary(payload.column_name, payload.wire_type, bytes),
        }
    }
}

macro_rules! temporal_impl {
    ($decoder:ty, $kind:expr) => {
        impl<S, B> Decoder<PgBinary, S, B> for $decoder
        where
            S: From<String>,
        {
            fn decode(&self, payload: PgBinaryColumn<'_>) -> Result<Value<S, B>, DecodeError> {
                match payload.raw {
                    None => Ok(Value::Null),
                    Some(bytes) => decode_temporal_binary(payload.column_name, $kind, bytes),
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

impl<S, B> TypeMapDefaults<S, B> for PgBinary
where
    S: From<String> + AsRef<str>,
    B: From<Vec<u8>>,
{
    fn defaults() -> TypeMap<Self, S, B> {
        TypeMap::new()
            .with(WireKind::Bool, BoolDecoder)
            .with(WireKind::Int, IntDecoder)
            .with(WireKind::Real, RealDecoder)
            .with(WireKind::Text, TextDecoder)
            .with(WireKind::Bytes, PgByteaBinaryDecoder)
            .with(WireKind::Uuid, UuidBlob16Decoder)
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
            .with(WireKind::Vector, VectorFloat32Decoder)
    }
}
