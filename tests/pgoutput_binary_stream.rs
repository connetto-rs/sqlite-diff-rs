//! A pgoutput stream started with `binary on` decodes through
//! [`PgBinaryStream`], which hands each cell to the registry of its own
//! format: a binary cell to the `PgBinary` decoder of its kind and a text
//! cell, which PostgreSQL sends for a type without a binary send function,
//! to the `PgWalstream` decoder of its kind.

#![cfg(feature = "pg-walstream")]

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use bytes::Bytes;
use sqlite_diff_rs::pg_walstream::{
    ColumnValue, EventType, PgBinaryStream, PgWalstream, PgWalstreamColumn, RowData,
};
use sqlite_diff_rs::{
    ArrayElement, DecodeError, Decoder, PatchSet, PgBinary, PgBinaryColumn, TypeMap,
    UuidBlob16Decoder, Value, WireAdapter, WireKind, WireType,
};

mod common;
use common::{TestUsersTable, test_schema};

type V = Value<String, Vec<u8>>;

/// The 16 raw bytes of `550e8400-e29b-41d4-a716-446655440000`.
const UUID_BYTES: [u8; 16] = [
    0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
];

fn binary_stream() -> PgBinaryStream<String, Vec<u8>> {
    PgBinaryStream::new(TypeMap::defaults(), TypeMap::defaults())
}

fn decode(
    adapter: &PgBinaryStream<String, Vec<u8>>,
    wire_type: WireType,
    data: &ColumnValue,
) -> Result<V, DecodeError> {
    adapter.decode(PgWalstreamColumn {
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

/// The same row as a text stream and as a binary stream builds the same
/// patchset, whichever format each cell of the binary stream arrives in.
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

    let mut mixed = RowData::new();
    mixed.push(
        Arc::from("id"),
        ColumnValue::Binary(Bytes::copy_from_slice(&7_i64.to_be_bytes())),
    );
    mixed.push(Arc::from("name"), ColumnValue::text("Alice"));
    mixed.push(
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
    for row in [binary, mixed] {
        let from_binary_stream: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
            .digest(&insert(row), &schema, &binary_stream())
            .unwrap();
        assert_eq!(from_binary_stream.build(), from_text.build());
    }
}

/// A binary cell decodes exactly as the same bytes do through `PgBinary`,
/// parameters of the wire type included.
#[test]
fn binary_cells_decode_as_pg_binary() {
    let pg_binary = TypeMap::<PgBinary, String, Vec<u8>>::defaults();
    // numeric 12.50: ndigits 2, weight 0, sign +, dscale 2, digits [12, 5000].
    let numeric = [0, 2, 0, 0, 0, 0, 0, 2, 0, 12, 0x13, 0x88];
    let cases: [(WireType, &[u8]); 6] = [
        (WireType::Bool, &[1]),
        (WireType::Int, &42_i32.to_be_bytes()),
        (WireType::Text, "héllo".as_bytes()),
        (WireType::Uuid, &UUID_BYTES),
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

/// One kind carries a binary cell for a type with a send function and a
/// text cell for a type without one, such as `aclitem` declared as text,
/// and each decodes by its own format.
#[test]
fn one_kind_decodes_binary_and_text_cells() {
    let stream = binary_stream();
    assert_eq!(
        decode(
            &stream,
            WireType::Text,
            &ColumnValue::binary_bytes(Bytes::from_static(b"plain"))
        ),
        Ok(Value::Text(String::from("plain")))
    );
    assert_eq!(
        decode(&stream, WireType::Text, &ColumnValue::text("=r/owner")),
        Ok(Value::Text(String::from("=r/owner")))
    );
}

/// A text cell decodes through the caller's text registry, so a text cell
/// and a binary cell of the same value store the same value when both
/// registries pick the same policy.
#[test]
fn text_cells_use_the_callers_text_registry() {
    let stream = PgBinaryStream::new(
        TypeMap::<PgBinary, String, Vec<u8>>::defaults(),
        TypeMap::<PgWalstream, String, Vec<u8>>::defaults().with(WireKind::Uuid, UuidBlob16Decoder),
    );
    let from_text = decode(
        &stream,
        WireType::Uuid,
        &ColumnValue::text("550e8400-e29b-41d4-a716-446655440000"),
    );
    let from_binary = decode(
        &stream,
        WireType::Uuid,
        &ColumnValue::Binary(Bytes::copy_from_slice(&UUID_BYTES)),
    );
    assert_eq!(from_text, Ok(Value::Blob(UUID_BYTES.to_vec())));
    assert_eq!(from_text, from_binary);
}

/// A `PgBinary` decoder storing every value as `"loud"`, telling which
/// registry decoded an array element.
struct Loud;

impl Decoder<PgBinary, String, Vec<u8>> for Loud {
    fn decode(&self, _payload: PgBinaryColumn<'_>) -> Result<V, DecodeError> {
        Ok(Value::Text(String::from("loud")))
    }
}

/// An array decodes its elements through the registry of its own format,
/// the caller's registrations included.
#[test]
fn array_elements_decode_through_the_registry_of_their_format() {
    let stream = PgBinaryStream::new(
        TypeMap::<PgBinary, String, Vec<u8>>::defaults().with(WireKind::Text, Loud),
        TypeMap::defaults(),
    );
    let text_array = WireType::Array {
        element: ArrayElement::Text,
    };
    // text[] {a,b}: one dimension, no nulls, element OID 25, length 2,
    // lower bound 1, then two one-byte elements.
    let mut array_send = Vec::new();
    for field in [1_i32, 0, 25, 2, 1, 1] {
        array_send.extend_from_slice(&field.to_be_bytes());
    }
    array_send.push(b'a');
    array_send.extend_from_slice(&1_i32.to_be_bytes());
    array_send.push(b'b');

    assert_eq!(
        decode(
            &stream,
            text_array,
            &ColumnValue::Binary(Bytes::from(array_send))
        ),
        Ok(Value::Text(String::from(r#"["loud","loud"]"#)))
    );
    assert_eq!(
        decode(&stream, text_array, &ColumnValue::text("{a,b}")),
        Ok(Value::Text(String::from(r#"["a","b"]"#)))
    );
}

/// A binary cell of a kind the `PgBinary` registry lacks is refused even
/// when the text registry has the kind, and a text cell of a kind the text
/// registry lacks is refused even when the `PgBinary` registry has it.
#[test]
fn each_format_decodes_only_through_its_own_registry() {
    let binary_only = PgBinaryStream::new(TypeMap::defaults(), TypeMap::new());
    assert!(matches!(
        decode(&binary_only, WireType::Int, &ColumnValue::text("1")),
        Err(DecodeError::NoDecoderForType { .. })
    ));
    let text_only = PgBinaryStream::new(TypeMap::new(), TypeMap::defaults());
    assert!(matches!(
        decode(
            &text_only,
            WireType::Int,
            &ColumnValue::Binary(Bytes::from_static(&[0, 0, 0, 1]))
        ),
        Err(DecodeError::NoDecoderForType { .. })
    ));
}

/// A NULL carries no format, so it decodes through the `PgBinary` registry
/// when that has its kind and through the text registry otherwise.
#[test]
fn null_decodes_through_whichever_registry_has_its_kind() {
    assert_eq!(
        decode(&binary_stream(), WireType::Text, &ColumnValue::Null),
        Ok(Value::Null)
    );
    assert_eq!(
        decode(&binary_stream(), WireType::Opaque, &ColumnValue::Null),
        Ok(Value::Null)
    );
    let empty = PgBinaryStream::<String, Vec<u8>>::new(TypeMap::new(), TypeMap::new());
    assert!(matches!(
        decode(&empty, WireType::Text, &ColumnValue::Null),
        Err(DecodeError::NoDecoderForType { .. })
    ));
}

/// A column held as its type's text output decodes from a text cell, and a
/// binary cell of it, whose send bytes no client can turn into that text,
/// is refused instead of being stored as text.
#[test]
fn opaque_column_decodes_text_and_refuses_binary() {
    let stream = binary_stream();
    assert_eq!(
        decode(&stream, WireType::Opaque, &ColumnValue::text("=r/owner")),
        Ok(Value::Text(String::from("=r/owner")))
    );
    // inet 10.0.0.1/32 in its send form, every byte ASCII.
    let inet = Bytes::from_static(&[2, 32, 0, 4, 10, 0, 0, 1]);
    assert!(matches!(
        decode(&stream, WireType::Opaque, &ColumnValue::Binary(inet)),
        Err(DecodeError::NoDecoderForType { ref column, .. }) if column == "c"
    ));
}
