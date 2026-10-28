//! Tests for `UuidBlob16Decoder` and `UuidText36Decoder`.
//!
//! Cross-format contract: both decoders accept 36-character hyphenated
//! and braced `{...}` UUID forms. `UuidBlob16Decoder` produces
//! `Value::Blob(16)`; `UuidText36Decoder` produces `Value::Text(36)`
//! verbatim. Null pass-through. Malformed UUIDs raise
//! `DecodeError::InvalidUuid`.
//!
//! `TypeMap::defaults()` registers `WireKind::Uuid` with the verbatim
//! `UuidText36Decoder`. A user who wants the 16-byte blob shape
//! registers `UuidBlob16Decoder` under the same key instead: the shape
//! is a registration-time decoder-policy choice, not a separate key.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::MaxwellColumn;
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    DecodeError, TypeMap, UuidBlob16Decoder, UuidText36Decoder, Value, WireAdapter, WireKind,
    WireType,
};

const UUID_HYPHENATED: &str = "550e8400-e29b-41d4-a716-446655440000";
const UUID_BRACED: &str = "{550e8400-e29b-41d4-a716-446655440000}";
const UUID_UPPERCASE: &str = "550E8400-E29B-41D4-A716-446655440000";
const UUID_BYTES: [u8; 16] = [
    0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
];

// -- UuidBlob16Decoder: pg_walstream -----------------------------------------

#[test]
fn uuid_blob16_pg_walstream_hyphenated() {
    let cv = ColumnValue::text(UUID_HYPHENATED);
    let got: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();
    assert_eq!(got, Value::Blob(UUID_BYTES.to_vec()));
}

#[test]
fn uuid_blob16_pg_walstream_braced_and_uppercase() {
    for form in [UUID_BRACED, UUID_UPPERCASE] {
        let cv = ColumnValue::text(form);
        let got: Value<String, Vec<u8>> = PgWalstreamColumn {
            column_name: "id",
            wire_type: WireType::Uuid,
            data: &cv,
        }
        .decoded_by(&UuidBlob16Decoder)
        .unwrap();
        assert_eq!(got, Value::Blob(UUID_BYTES.to_vec()), "form {form}");
    }
}

#[test]
fn uuid_blob16_pg_walstream_null() {
    let cv = ColumnValue::Null;
    let got: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();
    assert_eq!(got, Value::Null);
}

#[test]
fn uuid_blob16_pg_walstream_rejects_bad_input() {
    let cv = ColumnValue::text("not a uuid");
    let result: Result<Value<String, Vec<u8>>, _> = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidBlob16Decoder);
    assert!(matches!(
        result.unwrap_err(),
        DecodeError::InvalidUuid { .. }
    ));
}

// -- UuidText36Decoder: pg_walstream -----------------------------------------

#[test]
fn uuid_text36_pg_walstream_hyphenated_verbatim() {
    let cv = ColumnValue::text(UUID_HYPHENATED);
    let got: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidText36Decoder)
    .unwrap();
    assert_eq!(got, Value::Text(String::from(UUID_HYPHENATED)));
}

#[test]
fn uuid_text36_pg_walstream_braced_normalized_to_36() {
    // Braced form: verifies but normalizes to the 36-char hyphenated
    // form for downstream consistency.
    let cv = ColumnValue::text(UUID_BRACED);
    let got: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidText36Decoder)
    .unwrap();
    // Verbatim if source was already 36-char, normalized otherwise.
    match got {
        Value::Text(s) => {
            assert_eq!(s.len(), 36, "expected 36-char UUID, got {}", s.len());
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

// -- Wal2Json ---------------------------------------------------------------

#[test]
fn uuid_blob16_wal2json_string() {
    let s = serde_json::Value::String(UUID_HYPHENATED.into());
    let got: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        value: &s,
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();
    assert_eq!(got, Value::Blob(UUID_BYTES.to_vec()));
}

#[test]
fn uuid_text36_wal2json_string() {
    let s = serde_json::Value::String(UUID_HYPHENATED.into());
    let got: Value<String, Vec<u8>> = Wal2JsonColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        value: &s,
    }
    .decoded_by(&UuidText36Decoder)
    .unwrap();
    assert_eq!(got, Value::Text(String::from(UUID_HYPHENATED)));
}

// -- Maxwell ----------------------------------------------------------------

#[test]
fn uuid_blob16_maxwell_string() {
    let s = serde_json::Value::String(UUID_HYPHENATED.into());
    let got: Value<String, Vec<u8>> = MaxwellColumn {
        column_name: "id",
        wire_type: WireType::Uuid,
        value: &s,
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();
    assert_eq!(got, Value::Blob(UUID_BYTES.to_vec()));
}

// -- Shape selection at registration under one semantic key ------------------

/// A single semantic key `WireKind::Uuid` selects the UUID shape at
/// registration time: one `TypeMap` registered with `UuidBlob16Decoder`
/// produces `Value::Blob`, another registered with `UuidText36Decoder`
/// produces `Value::Text`, from the identical wire payload. This proves
/// that decoder policy is chosen at registration under one key, the
/// invariant that killed the "one runtime dial" alternative.
#[test]
fn uuid_shape_chosen_at_registration() {
    let mut blob_map: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    blob_map.register(WireKind::Uuid, UuidBlob16Decoder);
    let mut text_map: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    text_map.register(WireKind::Uuid, UuidText36Decoder);

    let s = serde_json::Value::String(UUID_HYPHENATED.into());

    let as_blob = blob_map
        .decode(Wal2JsonColumn {
            column_name: "a",
            wire_type: WireType::Uuid,
            value: &s,
        })
        .unwrap();
    let as_text = text_map
        .decode(Wal2JsonColumn {
            column_name: "b",
            wire_type: WireType::Uuid,
            value: &s,
        })
        .unwrap();

    assert_eq!(as_blob, Value::Blob(UUID_BYTES.to_vec()));
    assert_eq!(as_text, Value::Text(String::from(UUID_HYPHENATED)));
    assert_ne!(as_blob, as_text);
}

// -- Defaults route UUID to verbatim Text36 ----------------------------------

/// `TypeMap::defaults()` registers `WireKind::Uuid` with the
/// verbatim `UuidText36Decoder`, so UUID columns decode without any
/// explicit registration.
#[test]
fn defaults_route_uuid_to_text36() {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let cv = ColumnValue::text(UUID_HYPHENATED);
    let got = pg
        .decode(PgWalstreamColumn {
            column_name: "id",
            wire_type: WireType::Uuid,
            data: &cv,
        })
        .unwrap();
    assert_eq!(got, Value::Text(String::from(UUID_HYPHENATED)));
}
