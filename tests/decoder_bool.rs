//! Tests for `BoolDecoder`.
//!
//! Cross-format contract: `BoolDecoder::decode(payload)` returns
//! `Value::Integer(1)` for boolean-true wire values and
//! `Value::Integer(0)` for boolean-false ones, uniformly across
//! `pg_walstream`, `wal2json`, and `maxwell`.
//!
//! Also asserts that `TypeMap::<Src, S, B>::defaults()` registers the
//! boolean `WireType` for each source so users who
//! pass `TypeMap::defaults()` get bool decoding without any explicit
//! registration.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use bytes::Bytes;
use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{BoolDecoder, DecodeError, TypeMap, Value, WireAdapter, WireKind, WireType};

// -- Standalone decoder contract ---------------------------------------------

#[test]
fn bool_decoder_pg_walstream_text_t_and_f() {
    let cv_t = ColumnValue::text("t");
    let cv_f = ColumnValue::text("f");

    let payload_t = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv_t,
    };
    let payload_f = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv_f,
    };

    let got_t: Value<String, Vec<u8>> = payload_t.decoded_by(&BoolDecoder).unwrap();
    let got_f: Value<String, Vec<u8>> = payload_f.decoded_by(&BoolDecoder).unwrap();
    assert_eq!(got_t, Value::Integer(1));
    assert_eq!(got_f, Value::Integer(0));
}

#[test]
fn bool_decoder_pg_walstream_binary_one_and_zero() {
    let cv_true = ColumnValue::binary_bytes(Bytes::from_static(&[0x01]));
    let cv_false = ColumnValue::binary_bytes(Bytes::from_static(&[0x00]));

    let payload_true = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv_true,
    };
    let payload_false = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv_false,
    };

    let got_true: Value<String, Vec<u8>> = payload_true.decoded_by(&BoolDecoder).unwrap();
    let got_false: Value<String, Vec<u8>> = payload_false.decoded_by(&BoolDecoder).unwrap();
    assert_eq!(got_true, Value::Integer(1));
    assert_eq!(got_false, Value::Integer(0));
}

#[test]
fn bool_decoder_pg_walstream_null() {
    let cv_null = ColumnValue::Null;
    let payload = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv_null,
    };
    let got: Value<String, Vec<u8>> = payload.decoded_by(&BoolDecoder).unwrap();
    assert_eq!(got, Value::Null);
}

#[test]
fn bool_decoder_pg_walstream_rejects_arbitrary_text() {
    let cv = ColumnValue::text("maybe");
    let payload = PgWalstreamColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        data: &cv,
    };
    let result: Result<Value<String, Vec<u8>>, _> = payload.decoded_by(&BoolDecoder);
    let err = result.unwrap_err();
    assert!(matches!(err, DecodeError::WrongPayloadKind { .. }));
}

#[test]
fn bool_decoder_wal2json_true_and_false() {
    let true_json = serde_json::Value::Bool(true);
    let false_json = serde_json::Value::Bool(false);

    let got_true: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &true_json,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    let got_false: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &false_json,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    assert_eq!(got_true, Value::Integer(1));
    assert_eq!(got_false, Value::Integer(0));
}

#[test]
fn bool_decoder_wal2json_null() {
    let null_json = serde_json::Value::Null;
    let got: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &null_json,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    assert_eq!(got, Value::Null);
}

#[test]
fn bool_decoder_wal2json_rejects_non_bool_shapes() {
    let s = serde_json::Value::String("nope".into());
    let n = serde_json::Value::Number(42.into());

    for value in [&s, &n] {
        let payload = Wal2JsonColumn {
            column_name: "active",
            wire_type: WireType::Bool,
            value,
        };
        let result: Result<Value<String, Vec<u8>>, _> = payload.decoded_by(&BoolDecoder);
        let err = result.unwrap_err();
        assert!(matches!(err, DecodeError::WrongPayloadKind { .. }));
    }
}

#[test]
fn bool_decoder_maxwell_true_and_false() {
    let true_json = serde_json::Value::Bool(true);
    let false_json = serde_json::Value::Bool(false);

    let got_true: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &true_json,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    let got_false: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &false_json,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    assert_eq!(got_true, Value::Integer(1));
    assert_eq!(got_false, Value::Integer(0));
}

#[test]
fn bool_decoder_maxwell_accepts_int_zero_and_one() {
    // MySQL `tinyint(1)` bool wire values often arrive as 0/1 integer
    // JSON, not booleans. The decoder normalizes both shapes.
    let one = serde_json::Value::Number(1.into());
    let zero = serde_json::Value::Number(0.into());

    let got_one: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &one,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    let got_zero: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "active",
        wire_type: WireType::Bool,
        value: &zero,
    }
    .decoded_by(&BoolDecoder)
    .unwrap();
    assert_eq!(got_one, Value::Integer(1));
    assert_eq!(got_zero, Value::Integer(0));
}

// -- Defaults registration ---------------------------------------------------

#[test]
fn type_map_defaults_route_bool_key_to_bool_decoder() {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let mx: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();

    let cv_t = ColumnValue::text("t");
    let got_pg = pg
        .decode(PgWalstreamColumn {
            column_name: "active",
            wire_type: WireType::Bool,
            data: &cv_t,
        })
        .unwrap();
    assert_eq!(got_pg, Value::Integer(1));

    let true_json = serde_json::Value::Bool(true);
    let got_w2j = w2j
        .decode(Wal2JsonColumn {
            column_name: "active",
            wire_type: WireType::Bool,
            value: &true_json,
        })
        .unwrap();
    assert_eq!(got_w2j, Value::Integer(1));

    let got_mx = mx
        .decode(MaxwellColumn {
            column_name: "active",
            wire_type: WireType::Bool,
            value: &true_json,
        })
        .unwrap();
    assert_eq!(got_mx, Value::Integer(1));
}

/// A `TypeMap` with only `BoolDecoder` registered under
/// `WireKind::Bool` still declines unregistered types with
/// `NoDecoderForType`.
#[test]
fn user_registered_bool_still_declines_unknown_keys() {
    let mut types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    types.register(WireKind::Bool, BoolDecoder);

    let val = serde_json::Value::Bool(true);
    let got_bool = types
        .decode(Wal2JsonColumn {
            column_name: "active",
            wire_type: WireType::Bool,
            value: &val,
        })
        .unwrap();
    assert_eq!(got_bool, Value::Integer(1));

    let err = types
        .decode(Wal2JsonColumn {
            column_name: "other",
            wire_type: WireType::Text,
            value: &val,
        })
        .unwrap_err();
    assert!(matches!(err, DecodeError::NoDecoderForType { .. }));
}
