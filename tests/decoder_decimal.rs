//! Tests for `DecimalTextDecoder`.
//!
//! Cross-format contract: `DecimalTextDecoder` preserves the wire
//! decimal verbatim as `Value::Text`, a JSON number included, since the
//! crate keeps a JSON number's digits exactly. Null pass-through.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{DecimalTextDecoder, TypeMap, Value, WireAdapter, WireType};

// -- pg_walstream ------------------------------------------------------------

#[test]
fn decimal_pg_walstream_verbatim() {
    let cases = [
        "1234567890.12345678",
        "0",
        "-9999.99",
        "1e100",
        "1.0000000000000001", // near f64 precision boundary
    ];
    for wire in &cases {
        let cv = ColumnValue::text(wire);
        let got: Value<String, Vec<u8>> = PgWalstreamColumn {
            column_name: "n",
            wire_type: WireType::Decimal,
            data: &cv,
        }
        .decoded_by(&DecimalTextDecoder)
        .unwrap();
        assert_eq!(got, Value::Text(String::from(*wire)));
    }
}

#[test]
fn decimal_pg_walstream_null() {
    let cv = ColumnValue::Null;
    let got: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "n",
        wire_type: WireType::Decimal,
        data: &cv,
    }
    .decoded_by(&DecimalTextDecoder)
    .unwrap();
    assert_eq!(got, Value::Null);
}

// -- wal2json ----------------------------------------------------------------

#[test]
fn decimal_wal2json_string_verbatim() {
    let s = serde_json::Value::String("1234567890.12345678".into());
    let got: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "n",
        wire_type: WireType::Decimal,
        value: &s,
    }
    .decoded_by(&DecimalTextDecoder)
    .unwrap();
    assert_eq!(got, Value::Text(String::from("1234567890.12345678")));
}

#[test]
fn decimal_wal2json_number_keeps_its_digits() {
    for literal in ["1234", "12.50", "1234567890123456.78", "1.0000000000000001"] {
        let n: serde_json::Value = serde_json::from_str(literal).unwrap();
        let got: Value<String, Vec<u8>> = Wal2JsonColumn {
            column_name: "n",
            wire_type: WireType::Decimal,
            value: &n,
        }
        .decoded_by(&DecimalTextDecoder)
        .unwrap();
        assert_eq!(got, Value::Text(String::from(literal)));
    }
}

// -- maxwell -----------------------------------------------------------------

#[test]
fn decimal_maxwell_string_verbatim() {
    let s = serde_json::Value::String("-5000.0000".into());
    let got: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "n",
        wire_type: WireType::Decimal,
        value: &s,
    }
    .decoded_by(&DecimalTextDecoder)
    .unwrap();
    assert_eq!(got, Value::Text(String::from("-5000.0000")));
}

// -- Defaults ----------------------------------------------------------------

#[test]
fn defaults_route_decimal_types() {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let mx: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();

    let cv = ColumnValue::text("100.25");
    let got = pg
        .decode(PgWalstreamColumn {
            column_name: "n",
            wire_type: WireType::Decimal,
            data: &cv,
        })
        .unwrap();
    assert_eq!(got, Value::Text(String::from("100.25")));

    let s = serde_json::Value::String("100.25".into());
    let got = w2j
        .decode(Wal2JsonColumn {
            column_name: "n",
            wire_type: WireType::Decimal,
            value: &s,
        })
        .unwrap();
    assert_eq!(got, Value::Text(String::from("100.25")));

    let got = mx
        .decode(MaxwellColumn {
            column_name: "n",
            wire_type: WireType::Decimal,
            value: &s,
        })
        .unwrap();
    assert_eq!(got, Value::Text(String::from("100.25")));
}
