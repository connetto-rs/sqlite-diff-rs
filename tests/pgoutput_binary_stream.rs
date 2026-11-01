//! A pgoutput stream started with `binary on` decodes through
//! `TypeMap::<PgWalstream, _, _>::for_binary_stream`, which hands each
//! binary cell to the `PgBinary` decoder registered for its kind.

#![cfg(feature = "pg-walstream")]

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use bytes::Bytes;
use sqlite_diff_rs::pg_walstream::{
    ColumnValue, EventType, PgWalstream, PgWalstreamColumn, RowData,
};
use sqlite_diff_rs::{
    DecodeError, JsonVerbatimDecoder, PatchSet, PgBinary, PgBinaryColumn, TypeMap, Value,
    WireAdapter, WireKind, WireType,
};

mod common;
use common::{TestUsersTable, test_schema};

type V = Value<String, Vec<u8>>;

fn binary_stream() -> TypeMap<PgWalstream, String, Vec<u8>> {
    TypeMap::for_binary_stream(TypeMap::<PgBinary, String, Vec<u8>>::defaults())
}

fn decode(
    map: &TypeMap<PgWalstream, String, Vec<u8>>,
    wire_type: WireType,
    data: &ColumnValue,
) -> Result<V, DecodeError> {
    map.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type,
        data,
    })
}

fn insert(data: RowData) -> EventType {
    EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data,
    }
}

/// The same row as a text stream and as a binary stream builds the same patchset.
#[test]
fn binary_stream_digest_matches_text_stream() {
    let mut text = RowData::new();
    text.push(Arc::from("id"), ColumnValue::text("7"));
    text.push(Arc::from("name"), ColumnValue::text("Alice"));
    text.push(Arc::from("active"), ColumnValue::text("t"));

    let mut binary = RowData::new();
    binary.push(
        Arc::from("id"),
        ColumnValue::Binary(Bytes::copy_from_slice(&7_i64.to_be_bytes())),
    );
    binary.push(
        Arc::from("name"),
        ColumnValue::Binary(Bytes::from_static(b"Alice")),
    );
    binary.push(
        Arc::from("active"),
        ColumnValue::Binary(Bytes::from_static(&[1])),
    );

    let schema = test_schema();
    let from_text: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(
            &insert(text),
            &schema,
            &TypeMap::<PgWalstream, String, Vec<u8>>::defaults(),
        )
        .unwrap();
    let from_binary: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&insert(binary), &schema, &binary_stream())
        .unwrap();

    assert_eq!(from_binary.build(), from_text.build());
}

/// A binary cell decodes exactly as the same bytes do through `PgBinary`,
/// parameters of the wire type included.
#[test]
fn binary_cells_decode_as_pg_binary() {
    let pg_binary = TypeMap::<PgBinary, String, Vec<u8>>::defaults();
    let uuid = [
        0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00,
    ];
    // numeric 12.50: ndigits 2, weight 0, sign +, dscale 2, digits [12, 5000].
    let numeric = [0, 2, 0, 0, 0, 0, 0, 2, 0, 12, 0x13, 0x88];
    let cases: [(WireType, &[u8]); 6] = [
        (WireType::Bool, &[1]),
        (WireType::Int, &42_i32.to_be_bytes()),
        (WireType::Text, "héllo".as_bytes()),
        (WireType::Uuid, &uuid),
        (WireType::Decimal, &numeric),
        (WireType::ScaledDecimal { scale: 2 }, &numeric),
    ];
    for (wire_type, bytes) in cases {
        let expected = pg_binary.decode(PgBinaryColumn {
            column_name: "c",
            wire_type,
            raw: Some(bytes),
        });
        let got = decode(
            &binary_stream(),
            wire_type,
            &ColumnValue::Binary(Bytes::copy_from_slice(bytes)),
        );
        assert_eq!(got, expected, "{wire_type:?}");
    }
    assert_eq!(
        decode(
            &binary_stream(),
            WireType::ScaledDecimal { scale: 2 },
            &ColumnValue::Binary(Bytes::copy_from_slice(&numeric))
        ),
        Ok(Value::Integer(1250))
    );
}

#[test]
fn binary_stream_null_is_null() {
    assert_eq!(
        decode(&binary_stream(), WireType::Text, &ColumnValue::Null),
        Ok(Value::Null)
    );
}

/// A text cell where the stream's format says binary is refused, never parsed.
#[test]
fn binary_stream_refuses_text_cells() {
    let result = decode(&binary_stream(), WireType::Int, &ColumnValue::text("42"));
    assert!(
        matches!(result, Err(DecodeError::WrongPayloadKind { ref column, .. }) if column == "c"),
        "{result:?}"
    );
}

/// A kind registered after the conversion replaces the binary decoder, which
/// is how a type without a binary send function gets its text decoder.
#[test]
fn decoder_registered_after_conversion_takes_over_its_kind() {
    let mut map = binary_stream();
    map.register(WireKind::Json, JsonVerbatimDecoder);
    assert_eq!(
        decode(&map, WireType::Json, &ColumnValue::text("{\"a\":1}")),
        Ok(Value::Text(String::from("{\"a\":1}")))
    );
}

/// A kind the `PgBinary` map lacks is missing from the binary stream map too.
#[test]
fn kinds_missing_from_the_pg_binary_map_stay_missing() {
    let map = TypeMap::<PgWalstream, String, Vec<u8>>::for_binary_stream(TypeMap::new());
    assert!(matches!(
        decode(
            &map,
            WireType::Int,
            &ColumnValue::Binary(Bytes::from_static(&[0, 0, 0, 1]))
        ),
        Err(DecodeError::NoDecoderForType { .. })
    ));
}
