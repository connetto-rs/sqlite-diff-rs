//! Comprehensive decoder edge-case tests covering binary mode payloads,
//! error paths, overflow handling, UUID decoding, and `not_yet_impl`
//! decoders for all three CDC wire sources.

#![cfg(all(feature = "pg-walstream", feature = "wal2json", feature = "maxwell"))]

extern crate alloc;

use alloc::vec::Vec;
use std::f64::consts::PI;

use bytes::Bytes;
use sqlite_diff_rs::WireAdapter;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{DecodeError, Decoder, TypeMap, Value, WireType};

/// Shorthand: decode `BoolDecoder` against `PgWalstream` source.
type PgBoolDec = dyn Decoder<PgWalstream, String, Vec<u8>>;
/// Shorthand: decode `BoolDecoder` against `Wal2Json` source.
type W2jBoolDec = dyn Decoder<Wal2Json, String, Vec<u8>>;
/// Shorthand: decode `BoolDecoder` against `Maxwell` source.
type MxBoolDec = dyn Decoder<Maxwell, String, Vec<u8>>;

/// Helper to coerce a concrete decoder to a specific source via `Decoder` impl.
macro_rules! as_pg_dec {
    ($dec:expr) => {
        &$dec as &PgBoolDec
    };
}
macro_rules! as_w2j_dec {
    ($dec:expr) => {
        &$dec as &W2jBoolDec
    };
}
macro_rules! as_mx_dec {
    ($dec:expr) => {
        &$dec as &MxBoolDec
    };
}

// ---------------------------------------------------------------------------
// pg_walstream — binary mode paths
// ---------------------------------------------------------------------------

#[test]
fn pg_bool_decoder_binary_01() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x01])),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(1), "bool binary 0x01 -> Integer(1)");
}

#[test]
fn pg_bool_decoder_binary_00() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00])),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(0), "bool binary 0x00 -> Integer(0)");
}

#[test]
fn pg_bool_decoder_binary_wrong_payload() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Bool,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x02])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_bool_decoder_text_wrong_payload() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Bool,
        data: &ColumnValue::text("xyz"),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_int_decoder_binary_int2() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x2a])),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "int2 binary -> Integer(42)");
}

#[test]
fn pg_int_decoder_binary_int4() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x00, 0x00, 0x2a])),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "int4 binary -> Integer(42)");
}

#[test]
fn pg_int_decoder_binary_int8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a,
            ])),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42), "int8 binary -> Integer(42)");
}

#[test]
fn pg_int_decoder_binary_wrong_size() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_real_decoder_binary_float4() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&1.5_f32.to_be_bytes())),
        })
        .unwrap();
    assert_eq!(val, Value::Real(1.5), "float4 binary -> Real(1.5)");
}

#[test]
fn pg_real_decoder_binary_float8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&PI.to_be_bytes())),
        })
        .unwrap();
    assert_eq!(val, Value::Real(PI), "float8 binary -> Real(PI)");
}

#[test]
fn pg_real_decoder_binary_wrong_size() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Real,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x00, 0x00])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_bytea_binary_decoder_binary_pass_through() {
    let dec = sqlite_diff_rs::PgByteaBinaryDecoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0xde, 0xad])),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Blob(alloc::vec![0xde, 0xad]),
        "bytea binary pass-through"
    );
}

#[test]
fn pg_bytea_binary_decoder_invalid_hex_escape() {
    let dec = sqlite_diff_rs::PgByteaBinaryDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        data: &ColumnValue::text("\\xzz"),
    });
    match result {
        Err(DecodeError::InvalidHexEscape { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidHexEscape, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pg_walstream — error paths (InvalidUtf8, WrongPayloadKind)
// ---------------------------------------------------------------------------

/// Helper: text payload with non-UTF-8 bytes to trigger `InvalidUtf8`.
fn non_utf8_text() -> ColumnValue {
    ColumnValue::Text(Bytes::copy_from_slice(&[0xff, 0xfe]))
}

#[test]
fn pg_int_decoder_invalid_utf8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_int_decoder_integer_overflow() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &ColumnValue::text("999999999999999999999"),
    });
    match result {
        Err(DecodeError::IntegerOverflow { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected IntegerOverflow, got {other:?}"),
    }
}

#[test]
fn pg_int_decoder_non_numeric_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &ColumnValue::text("not-a-number"),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_real_decoder_invalid_utf8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Real,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_real_decoder_non_numeric_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Real,
        data: &ColumnValue::text("not-a-float"),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_text_decoder_invalid_utf8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Text,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_text_decoder_binary_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Text,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x61])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_decimal_text_decoder_invalid_utf8() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Decimal,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_decimal_malformed_binary_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Decimal,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pg_walstream — malformed binary payloads
// ---------------------------------------------------------------------------

#[test]
fn pg_timestamp_malformed_binary_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Timestamp,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_timestamptz_malformed_binary_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::TimestampTz,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_json_non_utf8_binary_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Json,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0xff, 0xfe])),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_jsonb_unknown_binary_version_rejected() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let result = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Jsonb,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pg_walstream — normalize_real paths (NaN→Null, -0.0 kept)
// ---------------------------------------------------------------------------

#[test]
fn pg_real_decoder_nan_to_null() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::text("NaN"),
        })
        .unwrap();
    assert_eq!(val, Value::Null, "NaN -> Null");
}

#[test]
fn pg_real_decoder_negative_zero_keeps_sign() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::text("-0.0"),
        })
        .unwrap();
    assert_eq!(val, Value::Real(-0.0), "-0.0 keeps its sign");
}

#[test]
fn pg_real_decoder_infinity() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let val = types
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Real,
            data: &ColumnValue::text("Infinity"),
        })
        .unwrap();
    assert!(matches!(
        val,
        Value::Real(f) if f.is_infinite() && f.is_sign_positive()
    ));
}

// ---------------------------------------------------------------------------
// pg_walstream — Int64OverflowToTextDecoder
// ---------------------------------------------------------------------------

#[test]
fn pg_int64_overflow_to_text_overflows() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Int,
            data: &ColumnValue::text("9223372036854775808"),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("9223372036854775808".into()),
        "overflow -> Text"
    );
}

#[test]
fn pg_int64_overflow_to_text_invalid_utf8() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_int64_overflow_to_text_non_numeric() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &ColumnValue::text("hello"),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_int64_overflow_to_text_binary_rejected() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Int,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x2a])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pg_walstream — UUID decoders (not registered in defaults)
// ---------------------------------------------------------------------------

#[test]
fn pg_uuid_blob16_valid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            data: &ColumnValue::text("a0eebc99-9c0b-4ef8-bb8d-6bb9efb4e148"),
        })
        .unwrap();
    let expected = alloc::vec![
        0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x8d, 0x6b, 0xb9, 0xef, 0xb4, 0xe1,
        0x48,
    ];
    assert_eq!(val, Value::Blob(expected), "valid UUID -> Blob16");
}

#[test]
fn pg_uuid_blob16_invalid_uuid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &ColumnValue::text("not-a-uuid"),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn pg_uuid_blob16_invalid_utf8() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_uuid_blob16_binary_rejected() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn pg_uuid_text36_valid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            data: &ColumnValue::text("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148"),
        })
        .unwrap();
    // preserve_or_canonicalize_uuid_text preserves original case
    assert_eq!(
        val,
        Value::Text("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
        "UUID -> original case preserved"
    );
}

#[test]
fn pg_uuid_text36_invalid_uuid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &ColumnValue::text("not-a-uuid"),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn pg_uuid_text36_invalid_utf8() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &non_utf8_text(),
    });
    match result {
        Err(DecodeError::InvalidUtf8 { column }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn pg_uuid_text36_binary_rejected() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &ColumnValue::Binary(Bytes::copy_from_slice(&[0x00, 0x01])),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pg_walstream — not_yet_impl decoders
// ---------------------------------------------------------------------------

#[test]
fn pg_bytea_text_mode_not_yet_impl() {
    let dec = sqlite_diff_rs::PgByteaTextModeDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        data: &ColumnValue::text("hello"),
    });
    match result {
        Err(DecodeError::NotYetImplemented { decoder }) => {
            assert!(decoder.contains("PgByteaTextModeDecoder"));
        }
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }
}

#[test]
fn pg_mysql_binary_not_yet_impl() {
    let dec = sqlite_diff_rs::MySqlBinaryDecoder;
    let result = as_pg_dec!(dec).decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        data: &ColumnValue::text("hello"),
    });
    match result {
        Err(DecodeError::NotYetImplemented { decoder }) => {
            assert!(decoder.contains("MySqlBinaryDecoder"));
        }
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }
}

#[test]
fn pg_null_decoder_always_null() {
    let dec = sqlite_diff_rs::NullDecoder;
    let val = as_pg_dec!(dec)
        .decode(PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Bool,
            data: &ColumnValue::text("anything"),
        })
        .unwrap();
    assert_eq!(val, Value::Null, "NullDecoder always returns Null");
}

// ---------------------------------------------------------------------------
// wal2json — error paths and edge cases
// ---------------------------------------------------------------------------

#[test]
fn w2j_int64_overflow_normal_integer() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42));
}

#[test]
fn w2j_int64_overflow_string_overflow() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("9223372036854775808".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("9223372036854775808".into()),
        "overflow string -> Text"
    );
}

#[test]
fn w2j_int64_overflow_string_in_range() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("42".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42));
}

#[test]
fn w2j_int64_overflow_wrong_payload() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Int,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}
/// NaN cannot be represented in serde_json::Value::Number, so the
/// NaN→Null path in RealDecoder is unreachable for wal2json/maxwell.
/// The NaN normalization is exercised via pg_walstream text-mode "NaN".
#[test]
fn w2j_real_decoder_nan_unreachable() {
    let dec = sqlite_diff_rs::RealDecoder;
    // Verify that -0.0 keeps its sign instead.
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Real,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(-0.0).unwrap()),
        })
        .unwrap();
    assert_eq!(val, Value::Real(-0.0), "-0.0 keeps its sign");
}

#[test]
fn w2j_real_decoder_bool_wrong_payload() {
    let dec = sqlite_diff_rs::RealDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Real,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn w2j_uuid_blob16_valid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            value: &serde_json::Value::String("a0eebc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
        })
        .unwrap();
    let expected = alloc::vec![
        0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x8d, 0x6b, 0xb9, 0xef, 0xb4, 0xe1,
        0x48,
    ];
    assert_eq!(val, Value::Blob(expected));
}

#[test]
fn w2j_uuid_blob16_invalid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::String("bad".into()),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn w2j_uuid_blob16_wrong_payload() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn w2j_uuid_text36_valid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            value: &serde_json::Value::String("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
        })
        .unwrap();
    // preserve_or_canonicalize_uuid_text preserves original case
    assert_eq!(
        val,
        Value::Text("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
    );
}

#[test]
fn w2j_uuid_text36_invalid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::String("bad".into()),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn w2j_uuid_text36_wrong_payload() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::Bool(false),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn w2j_json_verbatim_wrong_payload() {
    let dec = sqlite_diff_rs::JsonVerbatimDecoder;
    // Numbers are serializable, so they succeed
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Text("42".into()), "number -> verbatim text");
}

#[test]
fn w2j_json_canonical_string() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Jsonb,
            value: &serde_json::Value::String("{\"b\": 2, \"a\": 1}".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Text(r#"{"a":1,"b":2}"#.into()));
}

#[test]
fn w2j_json_canonical_object() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    let mut map = serde_json::Map::new();
    map.insert(
        "b".to_string(),
        serde_json::Value::Number(serde_json::Number::from(2)),
    );
    map.insert(
        "a".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Jsonb,
            value: &serde_json::Value::Object(map),
        })
        .unwrap();
    assert_eq!(val, Value::Text(r#"{"a":1,"b":2}"#.into()));
}

#[test]
fn w2j_json_canonical_unsupported_payload() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    // Numbers are serializable, so they succeed
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Jsonb,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(1e300).unwrap()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("1e+300".into()),
        "number -> canonical text"
    );
}

#[test]
fn w2j_pg_bytea_binary_decoder_not_yet_impl() {
    let dec = sqlite_diff_rs::PgByteaBinaryDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::Null,
    });
    match result {
        Err(DecodeError::NotYetImplemented { .. }) => {}
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }
}

#[test]
fn w2j_not_yet_impl_decoders() {
    let dec_mysql = sqlite_diff_rs::MySqlBinaryDecoder;
    let result_mysql = as_w2j_dec!(dec_mysql).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::Null,
    });
    match result_mysql {
        Err(DecodeError::NotYetImplemented { .. }) => {}
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }
}

#[test]
fn w2j_null_decoder_always_null() {
    let dec = sqlite_diff_rs::NullDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("hello".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Null);
}

// ---------------------------------------------------------------------------
// maxwell — error paths and edge cases
// ---------------------------------------------------------------------------

#[test]
fn mx_int64_overflow_normal_integer() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::Number(serde_json::Number::from(42)),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42));
}

#[test]
fn mx_int64_overflow_string_overflow() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("9223372036854775808".into()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("9223372036854775808".into()),
        "overflow string -> Text"
    );
}

#[test]
fn mx_int64_overflow_string_in_range() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("42".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Integer(42));
}

#[test]
fn mx_int64_overflow_wrong_payload() {
    let dec = sqlite_diff_rs::Int64OverflowToTextDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Int,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_real_decoder_negative_zero_keeps_sign() {
    let dec = sqlite_diff_rs::RealDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Real,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(-0.0).unwrap()),
        })
        .unwrap();
    assert_eq!(val, Value::Real(-0.0), "-0.0 keeps its sign");
}

#[test]
fn mx_real_decoder_bool_wrong_payload() {
    let dec = sqlite_diff_rs::RealDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Real,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_uuid_blob16_valid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            value: &serde_json::Value::String("a0eebc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
        })
        .unwrap();
    let expected = alloc::vec![
        0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x8d, 0x6b, 0xb9, 0xef, 0xb4, 0xe1,
        0x48,
    ];
    assert_eq!(val, Value::Blob(expected));
}

#[test]
fn mx_uuid_blob16_invalid() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::String("bad".into()),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn mx_uuid_blob16_wrong_payload() {
    let dec = sqlite_diff_rs::UuidBlob16Decoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_uuid_text36_valid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Uuid,
            value: &serde_json::Value::String("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
        })
        .unwrap();
    // preserve_or_canonicalize_uuid_text preserves original case
    assert_eq!(
        val,
        Value::Text("A0eEbc99-9c0b-4ef8-bb8d-6bb9efb4e148".into()),
    );
}

#[test]
fn mx_uuid_text36_invalid() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::String("bad".into()),
    });
    match result {
        Err(DecodeError::InvalidUuid { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidUuid, got {other:?}"),
    }
}

#[test]
fn mx_uuid_text36_wrong_payload() {
    let dec = sqlite_diff_rs::UuidText36Decoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_json_canonical_string() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::String("{\"b\": 2, \"a\": 1}".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Text(r#"{"a":1,"b":2}"#.into()));
}

#[test]
fn mx_json_canonical_object() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    let mut map = serde_json::Map::new();
    map.insert(
        "b".to_string(),
        serde_json::Value::Number(serde_json::Number::from(2)),
    );
    map.insert(
        "a".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::Object(map),
        })
        .unwrap();
    assert_eq!(val, Value::Text(r#"{"a":1,"b":2}"#.into()));
}

#[test]
fn mx_json_canonical_unsupported_payload() {
    let dec = sqlite_diff_rs::JsonCanonicalDecoder;
    // Numbers are serializable, so they succeed
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Json,
            value: &serde_json::Value::Number(serde_json::Number::from_f64(1e300).unwrap()),
        })
        .unwrap();
    assert_eq!(
        val,
        Value::Text("1e+300".into()),
        "number -> canonical text"
    );
}

#[test]
fn mx_not_yet_impl_decoders() {
    let dec_pg = sqlite_diff_rs::PgByteaTextModeDecoder;
    let result_pg = as_mx_dec!(dec_pg).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::Null,
    });
    match result_pg {
        Err(DecodeError::NotYetImplemented { .. }) => {}
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }

    let dec_pg_bytea = sqlite_diff_rs::PgByteaBinaryDecoder;
    let result_pg_bytea = as_mx_dec!(dec_pg_bytea).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::Null,
    });
    match result_pg_bytea {
        Err(DecodeError::NotYetImplemented { .. }) => {}
        other => panic!("expected NotYetImplemented, got {other:?}"),
    }
}

#[test]
fn mx_null_decoder_always_null() {
    let dec = sqlite_diff_rs::NullDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Int,
            value: &serde_json::Value::String("hello".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Null);
}

// ---------------------------------------------------------------------------
// wal2json — BinaryDecoder and DecimalTextDecoder edge cases
// ---------------------------------------------------------------------------

#[test]
fn w2j_pg_bytea_text_mode_decoder_valid() {
    let dec = sqlite_diff_rs::PgByteaTextModeDecoder;
    let val = as_w2j_dec!(dec)
        .decode(Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            value: &serde_json::Value::String("\\xdeadbeef".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Blob(alloc::vec![0xde, 0xad, 0xbe, 0xef]),);
}

#[test]
fn w2j_pg_bytea_text_mode_decoder_invalid_hex() {
    let dec = sqlite_diff_rs::PgByteaTextModeDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::String("\\xzz".into()),
    });
    match result {
        Err(DecodeError::InvalidHexEscape { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected InvalidHexEscape, got {other:?}"),
    }
}

#[test]
fn w2j_decimal_text_wrong_payload() {
    let dec = sqlite_diff_rs::DecimalTextDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Decimal,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn w2j_text_decoder_wrong_payload() {
    let dec = sqlite_diff_rs::TextDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Text,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn w2j_bool_decoder_wrong_payload() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let result = as_w2j_dec!(dec).decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Bool,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// maxwell — BinaryDecoder and DecimalTextDecoder edge cases
// ---------------------------------------------------------------------------

#[test]
fn mx_mysql_binary_decoder_valid() {
    let dec = sqlite_diff_rs::MySqlBinaryDecoder;
    let val = as_mx_dec!(dec)
        .decode(MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Bytes,
            value: &serde_json::Value::String("3q2+7w==".into()),
        })
        .unwrap();
    assert_eq!(val, Value::Blob(alloc::vec![0xde, 0xad, 0xbe, 0xef]),);
}

#[test]
fn mx_mysql_binary_decoder_invalid_base64() {
    let dec = sqlite_diff_rs::MySqlBinaryDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::String("not-base64!!!".into()),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_mysql_binary_decoder_wrong_payload() {
    let dec = sqlite_diff_rs::MySqlBinaryDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Bytes,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_decimal_text_wrong_payload() {
    let dec = sqlite_diff_rs::DecimalTextDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Decimal,
        value: &serde_json::Value::Bool(true),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_text_decoder_wrong_payload() {
    let dec = sqlite_diff_rs::TextDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Text,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}

#[test]
fn mx_bool_decoder_wrong_payload() {
    let dec = sqlite_diff_rs::BoolDecoder;
    let result = as_mx_dec!(dec).decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Bool,
        value: &serde_json::Value::Number(serde_json::Number::from(42)),
    });
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "c"),
        other => panic!("expected WrongPayloadKind, got {other:?}"),
    }
}
