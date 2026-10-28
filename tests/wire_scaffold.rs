//! Smoke tests for the `wire` module scaffold.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{DecodeError, TypeMap, Value, WireAdapter, WireKind, WireSource, WireType};

/// `WireAdapter<Src, S, B>` is object-safe: `dyn WireAdapter<..>` compiles.
#[test]
fn wire_adapter_is_object_safe_for_every_source() {
    fn _assert_pg(_: &dyn WireAdapter<PgWalstream, String, Vec<u8>>) {}
    fn _assert_wal2json(_: &dyn WireAdapter<Wal2Json, String, Vec<u8>>) {}
    fn _assert_maxwell(_: &dyn WireAdapter<Maxwell, String, Vec<u8>>) {}
}

/// `TypeMap<Src, S, B>::defaults()` is callable for every source and
/// grows monotonically as later phases populate more decoders.
#[test]
fn type_map_defaults_are_callable_for_every_source() {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let wal2json: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let maxwell: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();

    // Assert only that the map is non-empty.
    assert!(!pg.is_empty(), "pg_walstream defaults should carry bool");
    assert!(!wal2json.is_empty(), "wal2json defaults should carry bool");
    assert!(!maxwell.is_empty(), "maxwell defaults should carry bool");
}

/// Empty `TypeMap` returns `NoDecoderForType` for any lookup, using the
/// column name from the payload's `column_name` field.
#[test]
fn empty_type_map_reports_missing_decoder_by_column_name() {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    let value = serde_json::Value::Null;
    let payload = Wal2JsonColumn {
        column_name: "my_column",
        wire_type: WireType::Int,
        value: &value,
    };
    let err = types.decode(payload).unwrap_err();
    assert!(matches!(
        err,
        DecodeError::NoDecoderForType { ref column } if column == "my_column"
    ));
}

/// `WireSource::wire_type` extracts the semantic type from a
/// synthesized payload for each source.
#[test]
fn wire_source_wire_type_matches_payload_metadata() {
    let cv = sqlite_diff_rs::pg_walstream::ColumnValue::Null;
    let pg_payload = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    };
    assert_eq!(PgWalstream::wire_type(&pg_payload), WireType::Uuid);

    let val = serde_json::Value::Null;
    let w2j_payload = Wal2JsonColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        value: &val,
    };
    assert_eq!(Wal2Json::wire_type(&w2j_payload), WireType::Uuid);

    let mx_payload = MaxwellColumn {
        column_name: "id",
        wire_type: WireType::Text,
        value: &val,
    };
    assert_eq!(Maxwell::wire_type(&mx_payload), WireType::Text);
}

/// `TypeMap::register` accepts any `Decoder<Src, S, B> + Send + Sync +
/// 'static` and the resulting entry decodes via the registered decoder.
#[test]
fn type_map_register_and_dispatch_via_null_decoder() {
    use sqlite_diff_rs::NullDecoder;

    let mut types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    types.register(WireKind::Int, NullDecoder);

    let value = serde_json::Value::Number(42.into());
    let payload = Wal2JsonColumn {
        column_name: "n",
        wire_type: WireType::Int,
        value: &value,
    };
    let got = types.decode(payload).unwrap();
    assert!(matches!(got, Value::Null));
}
