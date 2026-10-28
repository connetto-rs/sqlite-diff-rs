//! Comprehensive tests for `TypeMap::defaults()` registration.
//!
//! Verifies that every registered key produces the expected `Value` variant
//! for a known input, confirming that the crate's self-evident mappings are
//! correctly wired for all three sources.

#![cfg(all(feature = "pg-walstream", feature = "wal2json", feature = "maxwell"))]

extern crate alloc;

use alloc::vec::Vec;
use std::f64::consts::PI;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{DecodeError, TypeMap, Value, WireAdapter, WireType};

// -- pg_walstream -----------------------------------------------------------

#[test]
fn pg_defaults_bool_key_produces_integer() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Bool -> BoolDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            data: &ColumnValue::text("t"),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(1), "PG bool 't' -> Integer(1)");
}

#[test]
fn pg_defaults_int2_produces_integer() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Int -> IntDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::text("42"),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "PG int2 text '42' -> Integer(42)");
}

#[test]
fn pg_defaults_int4_produces_integer() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Int -> IntDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::text("42"),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "PG int4 text '42' -> Integer(42)");
}

#[test]
fn pg_defaults_int8_produces_integer() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Int -> IntDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::text("42"),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "PG int8 text '42' -> Integer(42)");
}

#[test]
fn pg_defaults_float4_produces_real() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Real -> RealDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::text("3.141592653589793"),
        })
        .unwrap();
    assert_eq!(val, Value::Real(PI), "PG float4 text -> Real(PI)");
}

#[test]
fn pg_defaults_float8_produces_real() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Real -> RealDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::text("3.141592653589793"),
        })
        .unwrap();
    assert_eq!(val, Value::Real(PI), "PG float8 text -> Real(PI)");
}

#[test]
fn pg_defaults_text_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Text -> TextDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Text,
            data: &ColumnValue::text("hello"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("hello".into()), "PG text -> Text(hello)");
}

#[test]
fn pg_defaults_varchar_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Text -> TextDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Text,
            data: &ColumnValue::text("hello"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("hello".into()),
        "PG varchar -> Text(hello)"
    );
}

#[test]
fn pg_defaults_bpchar_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Text -> TextDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Text,
            data: &ColumnValue::text("hello"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("hello".into()), "PG bpchar -> Text(hello)");
}

#[test]
fn pg_defaults_name_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Text -> TextDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Text,
            data: &ColumnValue::text("hello"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("hello".into()), "PG name -> Text(hello)");
}

#[test]
fn pg_defaults_bytea_produces_blob() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Bytes -> PgByteaBinaryDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            data: &ColumnValue::text("\\xdeadbeef"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Blob(alloc::vec![0xde, 0xad, 0xbe, 0xef]),
        "PG bytea \\xdeadbeef -> Blob"
    );
}

#[test]
fn pg_defaults_numeric_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Decimal -> DecimalTextDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Decimal,
            data: &ColumnValue::text("123.456"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("123.456".into()),
        "PG numeric -> Text(123.456)"
    );
}

#[test]
fn pg_defaults_timestamp_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Timestamp -> TimestampDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Timestamp,
            data: &ColumnValue::text("2024-01-15 10:30:00"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("2024-01-15 10:30:00.000000".into()),
        "PG timestamp -> microseconds"
    );
}

#[test]
fn pg_defaults_timestamptz_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::TimestampTz -> TimestampTzDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::TimestampTz,
            data: &ColumnValue::text("2024-01-15 10:30:00+00"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("2024-01-15 10:30:00.000000+00:00".into()),
        "PG timestamptz -> UTC"
    );
}

#[test]
fn pg_defaults_date_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Date -> DateDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Date,
            data: &ColumnValue::text("2024-01-15"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("2024-01-15".into()), "PG date -> ISO date");
}

#[test]
fn pg_defaults_time_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Time -> TimeDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Time,
            data: &ColumnValue::text("10:30:00"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("10:30:00.000000".into()),
        "PG time -> microseconds"
    );
}

#[test]
fn pg_defaults_interval_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Interval -> IntervalIso8601Decoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Interval,
            data: &ColumnValue::text("1 day"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("P1D".into()), "PG interval -> ISO 8601");
}

#[test]
fn pg_defaults_json_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Json -> JsonVerbatimDecoder
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Json,
            data: &ColumnValue::text("{\"k\": 1}"),
        })
        .unwrap();
    assert_eq!(val, Value::Text("{\"k\": 1}".into()), "PG json -> verbatim");
}

#[test]
fn pg_defaults_jsonb_produces_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    // WireType::Jsonb -> JsonVerbatimDecoder (verbatim; serde_json Map is sorted)
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Jsonb,
            data: &ColumnValue::text("{\"k\": 1}"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("{\"k\": 1}".into()),
        "PG jsonb -> verbatim (canonical=verbatim for pg_walstream)"
    );
}

// -- wal2json ---------------------------------------------------------------

#[test]
fn w2j_defaults_boolean_produces_integer() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            value: &serde_json::Value::Bool(true),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Integer(1),
        "wal2json boolean true -> Integer(1)"
    );
}

#[test]
fn w2j_defaults_integer_produces_integer() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Integer(42),
        "wal2json integer 42 -> Integer(42)"
    );
}

#[test]
fn w2j_defaults_bigint_produces_integer() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "wal2json bigint 42 -> Integer(42)");
}

#[test]
fn w2j_defaults_smallint_produces_integer() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Integer(42),
        "wal2json smallint 42 -> Integer(42)"
    );
}

#[test]
fn w2j_defaults_real_produces_real() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Real,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(PI).unwrap()),
        })
        .unwrap();
    assert_eq!(val, Value::Real(PI), "wal2json real PI -> Real(PI)");
}

#[test]
fn w2j_defaults_double_precision_produces_real() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Real,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(PI).unwrap()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Real(PI),
        "wal2json double precision PI -> Real(PI)"
    );
}

#[test]
fn w2j_defaults_text_produces_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Text,
            value: &serde_json::Value::String("hello".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("hello".into()),
        "wal2json text -> Text(hello)"
    );
}

#[test]
fn w2j_defaults_varchar_produces_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Text,
            value: &serde_json::Value::String("hello".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("hello".into()),
        "wal2json varchar -> Text(hello)"
    );
}

#[test]
fn w2j_defaults_bytea_produces_blob() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            value: &serde_json::Value::String("\\xdeadbeef".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Blob(alloc::vec![0xde, 0xad, 0xbe, 0xef]),
        "wal2json bytea \\xdeadbeef -> Blob"
    );
}

#[test]
fn w2j_defaults_numeric_produces_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Decimal,
            value: &serde_json::Value::String("123.456".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("123.456".into()),
        "wal2json numeric -> Text(123.456)"
    );
}

#[test]
fn w2j_defaults_timestamp_produces_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Timestamp,
            value: &serde_json::Value::String("2024-01-15 10:30:00".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("2024-01-15 10:30:00.000000".into()),
        "wal2json timestamp -> microseconds"
    );
}

#[test]
fn w2j_defaults_json_produces_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::String("{\"k\": 1}".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("{\"k\": 1}".into()),
        "wal2json json string -> verbatim"
    );
}

#[test]
fn w2j_defaults_jsonb_produces_canonical_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    // JSON object as parsed value — jsonb canonicalizes
    let mut map = serde_json::Map::new();
    map.insert(
        "b".to_string(),
        serde_json::Value::Number(serde_json::Number::from(2)),
    );
    map.insert(
        "a".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    let val = types
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Jsonb,
            value: &serde_json::Value::Object(map),
        })
        .unwrap();
    // Keys should be sorted: a then b
    assert_eq!(
        val,
        Value::Text(r#"{"a":1,"b":2}"#.into()),
        "wal2json jsonb -> canonical sorted keys"
    );
}

// -- maxwell ----------------------------------------------------------------

#[test]
fn maxwell_defaults_tinyint1_produces_integer() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            value: &serde_json::Value::Bool(true),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Integer(1),
        "maxwell tinyint(1) true -> Integer(1)"
    );
}

#[test]
fn maxwell_defaults_int_produces_integer() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "maxwell int 42 -> Integer(42)");
}

#[test]
fn maxwell_defaults_bigint_produces_integer() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "maxwell bigint 42 -> Integer(42)");
}

#[test]
fn maxwell_defaults_bigint_unsigned_overflows_to_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    // This value exceeds i64::MAX
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("9223372036854775808".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("9223372036854775808".into()),
        "maxwell bigint unsigned overflow -> Text"
    );
}

#[test]
fn maxwell_defaults_float_produces_real() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Real,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(PI).unwrap()),
        })
        .unwrap();
    assert_eq!(val, Value::Real(PI), "maxwell float PI -> Real(PI)");
}

#[test]
fn maxwell_defaults_varchar_produces_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Text,
            value: &serde_json::Value::String("hello".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("hello".into()),
        "maxwell varchar -> Text(hello)"
    );
}

#[test]
fn maxwell_defaults_blob_produces_blob() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    // MySqlBinaryDecoder base64-decodes
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            value: &serde_json::Value::String("3q2+7w==".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Blob(alloc::vec![0xde, 0xad, 0xbe, 0xef]),
        "maxwell blob base64 -> Blob"
    );
}

#[test]
fn maxwell_defaults_decimal_produces_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Decimal,
            value: &serde_json::Value::String("123.456".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("123.456".into()),
        "maxwell decimal -> Text(123.456)"
    );
}

#[test]
fn maxwell_defaults_datetime_produces_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Timestamp,
            value: &serde_json::Value::String("2024-01-15 10:30:00".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("2024-01-15 10:30:00.000000".into()),
        "maxwell datetime -> microseconds"
    );
}

#[test]
fn maxwell_defaults_json_produces_canonical_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    // maxwell json default serializes verbatim (serde_json Map is sorted)
    let mut map = serde_json::Map::new();
    map.insert(
        "b".to_string(),
        serde_json::Value::Number(serde_json::Number::from(2)),
    );
    map.insert(
        "a".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    let val = types
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::Object(map),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text(r#"{"a":1,"b":2}"#.into()),
        "maxwell json -> canonical sorted keys"
    );
}

// -- Empty TypeMap reports NoDecoderForType ---------------------------------

#[test]
fn empty_type_map_reports_no_decoder_for_any_pg_key() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::new();
    let result = types.decode(PgWalstreamColumn {
        column_name: "test_col",
        wire_type: WireType::Int,
        data: &ColumnValue::text("42"),
    });
    match result {
        Err(DecodeError::NoDecoderForType { column }) => {
            assert_eq!(column, "test_col");
        }
        Err(other) => panic!("expected NoDecoderForType, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn empty_type_map_reports_no_decoder_for_any_w2j_key() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    let result = types.decode(Wal2JsonColumn {
        column_name: "test_col",
        wire_type: WireType::Int,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::NoDecoderForType { column }) => {
            assert_eq!(column, "test_col");
        }
        Err(other) => panic!("expected NoDecoderForType, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn empty_type_map_reports_no_decoder_for_any_maxwell_key() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::new();
    let result = types.decode(MaxwellColumn {
        column_name: "test_col",
        wire_type: WireType::Int,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::NoDecoderForType { column }) => {
            assert_eq!(column, "test_col");
        }
        Err(other) => panic!("expected NoDecoderForType, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}
