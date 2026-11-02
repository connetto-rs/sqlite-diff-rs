//! `WireType::Opaque` holds a column as its type's text output. The text
//! carrying sources decode it as text, and the `PgBinary` defaults have no
//! decoder for it, since no client can turn a type's send bytes into its
//! text output.

#![cfg(all(feature = "pg-walstream", feature = "wal2json", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use bytes::Bytes;
use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    DecodeError, PgBinary, PgBinaryColumn, TypeMap, Value, WireAdapter, WireType,
};

type V = Value<String, Vec<u8>>;

/// `point` `(1,2)` in its send form, two big-endian doubles.
const POINT_SEND: [u8; 16] = [0x3f, 0xf0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0];

#[test]
fn pgoutput_text_cell_is_text() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let got = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Opaque,
        data: &ColumnValue::text("(1,2)"),
    });
    assert_eq!(got, Ok(Value::Text(String::from("(1,2)"))));
}

#[test]
fn pgoutput_binary_cell_is_refused() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let got = types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Opaque,
        data: &ColumnValue::Binary(Bytes::from_static(&POINT_SEND)),
    });
    assert!(
        matches!(got, Err(DecodeError::WrongPayloadKind { .. })),
        "{got:?}"
    );
}

#[test]
fn pg_binary_has_no_decoder() {
    let types: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    for raw in [Some(&POINT_SEND[..]), None] {
        let got: Result<V, _> = types.decode(PgBinaryColumn {
            column_name: "c",
            wire_type: WireType::Opaque,
            raw,
        });
        assert!(
            matches!(got, Err(DecodeError::NoDecoderForType { .. })),
            "{raw:?} -> {got:?}"
        );
    }
}

#[test]
fn wal2json_string_is_text() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let got = types.decode(Wal2JsonColumn {
        column_name: "c",
        wire_type: WireType::Opaque,
        value: &serde_json::Value::String("10.0.0.1".into()),
    });
    assert_eq!(got, Ok(Value::Text(String::from("10.0.0.1"))));
}

#[test]
fn maxwell_string_is_text() {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let got = types.decode(MaxwellColumn {
        column_name: "c",
        wire_type: WireType::Opaque,
        value: &serde_json::Value::String("a,b".into()),
    });
    assert_eq!(got, Ok(Value::Text(String::from("a,b"))));
}
