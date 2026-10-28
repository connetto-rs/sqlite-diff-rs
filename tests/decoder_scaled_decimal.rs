//! Tests for `DecimalMinorUnitsDecoder`, the default decoder for
//! `WireType::ScaledDecimal`.
//!
//! A decimal decodes to `Value::Integer` holding `value × 10^scale`, the
//! minor units a `NUMERIC(p,s)` replica column stores. The value has to be
//! an exact whole number of minor units that fits an `i64`, whatever the
//! source's spelling, and anything else is refused, never rounded.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    DecimalMinorUnitsDecoder, DecodeError, PgBinary, PgBinaryColumn, TypeMap, Value, WireAdapter,
    WireType,
};

type V = Value<String, Vec<u8>>;

const CENTS: WireType = WireType::ScaledDecimal { scale: 2 };

/// PostgreSQL binary `numeric`: `ndigits`, `weight`, `sign` and `dscale`
/// as big-endian `u16`, then the base-10000 digits.
fn pg_numeric(weight: i16, sign: u16, dscale: u16, digits: &[u16]) -> Vec<u8> {
    let ndigits = u16::try_from(digits.len()).unwrap();
    let mut out = Vec::new();
    for field in [ndigits, weight.cast_unsigned(), sign, dscale] {
        out.extend_from_slice(&field.to_be_bytes());
    }
    for digit in digits {
        out.extend_from_slice(&digit.to_be_bytes());
    }
    out
}

const POSITIVE: u16 = 0x0000;
const NEGATIVE: u16 = 0x4000;
const NAN: u16 = 0xC000;
const INFINITY: u16 = 0xD000;
const NEGATIVE_INFINITY: u16 = 0xF000;

/// Decode `text` at `wire_type` through every text-carrying path: a
/// pgoutput text column, a wal2json and a Maxwell JSON string, and, when
/// the text is a JSON number, a wal2json and a Maxwell JSON number. Every
/// path must agree, and the agreed result is returned.
fn decode_text_everywhere(wire_type: WireType, text: &str) -> Result<V, DecodeError> {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let mx: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();

    let cv = ColumnValue::text(text);
    let expected = pg.decode(PgWalstreamColumn {
        column_name: "amount",
        wire_type,
        data: &cv,
    });

    let mut json_values = vec![serde_json::Value::String(text.into())];
    if let Ok(number @ serde_json::Value::Number(_)) = serde_json::from_str(text) {
        json_values.push(number);
    }
    for value in &json_values {
        let got = w2j.decode(Wal2JsonColumn {
            column_name: "amount",
            wire_type,
            value,
        });
        assert_eq!(got, expected, "wal2json {value:?}");
        let got = mx.decode(MaxwellColumn {
            column_name: "amount",
            wire_type,
            value,
        });
        assert_eq!(got, expected, "maxwell {value:?}");
    }
    expected
}

/// Decode PostgreSQL binary `numeric` bytes through `PgBinary` and the
/// binary-mode `pg_walstream` path, which must agree.
fn decode_binary_everywhere(wire_type: WireType, raw: &[u8]) -> Result<V, DecodeError> {
    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let pgw: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let expected = pgb.decode(PgBinaryColumn {
        column_name: "amount",
        wire_type,
        raw: Some(raw),
    });
    let cv = ColumnValue::binary_bytes(bytes::Bytes::copy_from_slice(raw));
    let got = pgw.decode(PgWalstreamColumn {
        column_name: "amount",
        wire_type,
        data: &cv,
    });
    assert_eq!(got, expected, "pg_walstream binary {raw:?}");
    expected
}

fn assert_unrepresentable(result: Result<V, DecodeError>, value: &str, scale: u32) {
    let expected = DecodeError::UnrepresentableDecimal {
        column: String::from("amount"),
        value: String::from(value),
        scale,
    };
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(decoded) => panic!("{value} decoded to {decoded:?}, expected {expected:?}"),
    }
}

#[test]
fn pg_binary_reproduction_decodes_minor_units() {
    let raw: &[u8] = &[0, 2, 0, 0, 0, 0, 0, 2, 0, 12, 0x13, 0x88];
    let adapter: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let value = adapter
        .decode(PgBinaryColumn {
            column_name: "amount",
            wire_type: CENTS,
            raw: Some(raw),
        })
        .unwrap();
    assert_eq!(value, Value::Integer(1250));
}

#[test]
fn text_decodes_exact_minor_units() {
    for (text, minor) in [
        ("0.14", 14),
        ("12.50", 1250),
        ("7", 700),
        ("-3.5", -350),
        ("0", 0),
        ("-0.00", 0),
        ("12.500", 1250),
        ("0.0000000", 0),
        ("0E-7", 0),
        ("1.25E+1", 1250),
        ("125e-1", 1250),
        ("0.0125e3", 1250),
        ("00012.50", 1250),
        ("1234567890123456.78", 123_456_789_012_345_678),
        ("92233720368547758.07", i64::MAX),
        ("-92233720368547758.08", i64::MIN),
        ("10000000000000000000000000000000000000000e-40", 100),
        ("00000000000000000000000000000000000000001", 100),
        ("0e99999999999999999999999", 0),
        ("0e-99999999999999999999999", 0),
    ] {
        assert_eq!(
            decode_text_everywhere(CENTS, text),
            Ok(Value::Integer(minor)),
            "{text}"
        );
    }
}

#[test]
fn binary_decodes_exact_minor_units() {
    for (raw, minor) in [
        (pg_numeric(-1, POSITIVE, 2, &[1400]), 14),
        (pg_numeric(0, POSITIVE, 2, &[12, 5000]), 1250),
        (pg_numeric(0, POSITIVE, 0, &[7]), 700),
        (pg_numeric(0, NEGATIVE, 1, &[3, 5000]), -350),
        (pg_numeric(0, POSITIVE, 0, &[]), 0),
        (pg_numeric(0, POSITIVE, 3, &[12, 5000]), 1250),
        (pg_numeric(2, POSITIVE, 0, &[1]), 10_000_000_000),
        (pg_numeric(-3, POSITIVE, 10, &[0, 0, 0]), 0),
        (
            pg_numeric(4, POSITIVE, 2, &[9, 2233, 7203, 6854, 7758, 700]),
            i64::MAX,
        ),
        (
            pg_numeric(4, NEGATIVE, 2, &[9, 2233, 7203, 6854, 7758, 800]),
            i64::MIN,
        ),
    ] {
        assert_eq!(
            decode_binary_everywhere(CENTS, &raw),
            Ok(Value::Integer(minor)),
            "{raw:?}"
        );
    }
}

#[test]
fn scale_zero_and_large_scale() {
    assert_eq!(
        decode_text_everywhere(WireType::ScaledDecimal { scale: 0 }, "42"),
        Ok(Value::Integer(42))
    );
    assert_eq!(
        decode_text_everywhere(
            WireType::ScaledDecimal { scale: 20 },
            "0.00000000000000000001"
        ),
        Ok(Value::Integer(1))
    );
    assert_eq!(
        decode_binary_everywhere(
            WireType::ScaledDecimal { scale: 20 },
            &pg_numeric(-5, POSITIVE, 20, &[1])
        ),
        Ok(Value::Integer(1))
    );
}

#[test]
fn null_decodes_to_null() {
    assert_eq!(
        TypeMap::<PgBinary, String, Vec<u8>>::defaults().decode(PgBinaryColumn {
            column_name: "amount",
            wire_type: CENTS,
            raw: None,
        }),
        Ok(Value::Null)
    );
    assert_eq!(
        TypeMap::<PgWalstream, String, Vec<u8>>::defaults().decode(PgWalstreamColumn {
            column_name: "amount",
            wire_type: CENTS,
            data: &ColumnValue::Null,
        }),
        Ok(Value::Null)
    );
    let null = serde_json::Value::Null;
    assert_eq!(
        TypeMap::<Wal2Json, String, Vec<u8>>::defaults().decode(Wal2JsonColumn {
            column_name: "amount",
            wire_type: CENTS,
            value: &null,
        }),
        Ok(Value::Null)
    );
    assert_eq!(
        TypeMap::<Maxwell, String, Vec<u8>>::defaults().decode(MaxwellColumn {
            column_name: "amount",
            wire_type: CENTS,
            value: &null,
        }),
        Ok(Value::Null)
    );
}

#[test]
fn text_needing_rounding_is_refused() {
    for text in [
        "12.505",
        "0.001",
        "-0.009",
        "1.2345e+1",
        "1e-3",
        "1e-99999999999999999999999",
    ] {
        assert_unrepresentable(decode_text_everywhere(CENTS, text), text, 2);
    }
}

#[test]
fn binary_needing_rounding_is_refused() {
    assert_unrepresentable(
        decode_binary_everywhere(CENTS, &pg_numeric(0, POSITIVE, 3, &[12, 5050])),
        "12.505",
        2,
    );
}

#[test]
fn text_overflowing_i64_is_refused() {
    for text in [
        "92233720368547758.08",
        "-92233720368547758.09",
        "100000000000000000000",
        "1e+100",
        "123456789012345678901234",
        "1e+99999999999999999999999",
    ] {
        assert_unrepresentable(decode_text_everywhere(CENTS, text), text, 2);
    }
}

#[test]
fn binary_overflowing_i64_is_refused() {
    assert_unrepresentable(
        decode_binary_everywhere(
            CENTS,
            &pg_numeric(4, POSITIVE, 2, &[9, 2233, 7203, 6854, 7758, 800]),
        ),
        "92233720368547758.08",
        2,
    );
}

#[test]
fn nan_and_infinities_are_refused() {
    for text in ["NaN", "Infinity", "-Infinity"] {
        assert_unrepresentable(decode_text_everywhere(CENTS, text), text, 2);
    }
    for (sign, text) in [
        (NAN, "NaN"),
        (INFINITY, "Infinity"),
        (NEGATIVE_INFINITY, "-Infinity"),
    ] {
        assert_unrepresentable(
            decode_binary_everywhere(CENTS, &pg_numeric(0, sign, 0, &[])),
            text,
            2,
        );
    }
}

#[test]
fn malformed_text_is_wrong_payload_kind() {
    for text in [
        "", "-", "+1", "1.", ".5", "1.2.3", "12,50", "1e", "1e+", "abc", "1x", "nan",
    ] {
        let result = decode_text_everywhere(CENTS, text);
        assert!(
            matches!(result, Err(DecodeError::WrongPayloadKind { ref column, .. }) if column == "amount"),
            "{text:?} -> {result:?}"
        );
    }
}

#[test]
fn malformed_binary_is_wrong_payload_kind() {
    for raw in [
        Vec::new(),
        pg_numeric(0, POSITIVE, 0, &[10_000]),
        pg_numeric(0, 0x1234, 0, &[1]),
        pg_numeric(0, NAN, 0, &[1]),
        pg_numeric(0, POSITIVE, 0x4000, &[1]),
    ] {
        let result = decode_binary_everywhere(CENTS, &raw);
        assert!(
            matches!(result, Err(DecodeError::WrongPayloadKind { ref column, .. }) if column == "amount"),
            "{raw:?} -> {result:?}"
        );
    }
}

#[test]
fn json_shapes_other_than_number_or_string_are_wrong_payload_kind() {
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    for value in [
        serde_json::json!(true),
        serde_json::json!([1]),
        serde_json::json!({}),
    ] {
        let result = w2j.decode(Wal2JsonColumn {
            column_name: "amount",
            wire_type: CENTS,
            value: &value,
        });
        assert!(
            matches!(result, Err(DecodeError::WrongPayloadKind { .. })),
            "{value:?} -> {result:?}"
        );
    }
}

/// The decoder reads the scale from the payload's wire type, so under a
/// type that names no scale it refuses rather than guess one.
#[test]
fn decoder_under_a_type_without_scale_is_wrong_payload_kind() {
    let cv = ColumnValue::text("12.50");
    let result: Result<V, _> = PgWalstreamColumn {
        column_name: "amount",
        wire_type: WireType::Decimal,
        data: &cv,
    }
    .decoded_by(&DecimalMinorUnitsDecoder);
    assert!(
        matches!(result, Err(DecodeError::WrongPayloadKind { .. })),
        "{result:?}"
    );
}

/// One registration under the `ScaledDecimal` kind serves every scale.
#[test]
fn one_registration_serves_every_scale() {
    let mut types: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::new();
    types.register(CENTS.kind(), DecimalMinorUnitsDecoder);
    let raw = pg_numeric(0, POSITIVE, 2, &[12, 5000]);
    for (scale, minor) in [(0, None), (2, Some(1250)), (4, Some(125_000))] {
        let result = types.decode(PgBinaryColumn {
            column_name: "amount",
            wire_type: WireType::ScaledDecimal { scale },
            raw: Some(&raw),
        });
        match minor {
            Some(minor) => assert_eq!(result, Ok(Value::Integer(minor))),
            None => assert_unrepresentable(result, "12.50", scale),
        }
    }
}

/// `WireType::Decimal` keeps decoding to the decimal text.
#[test]
fn unscaled_decimal_keeps_text() {
    assert_eq!(
        decode_text_everywhere(WireType::Decimal, "12.50"),
        Ok(Value::Text(String::from("12.50")))
    );
    assert_eq!(
        decode_binary_everywhere(WireType::Decimal, &pg_numeric(0, POSITIVE, 2, &[12, 5000])),
        Ok(Value::Text(String::from("12.50")))
    );
}
