//! Tests for `VectorFloat32Decoder`, the default decoder for
//! `WireType::Vector` on the PostgreSQL sources.
//!
//! A pgvector `vector` decodes to sqlite-vec's float32 layout, a `BLOB` of
//! the elements as little-endian IEEE 754 single-precision floats with no
//! header, whether it arrives as pgvector text or as `vector_send` binary.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    DecodeError, PgBinary, PgBinaryColumn, TypeMap, Value, VectorFloat32Decoder, WireAdapter,
    WireType,
};

type V = Value<String, Vec<u8>>;

const ANY: WireType = WireType::Vector { dimensions: None };
const DIM3: WireType = WireType::Vector {
    dimensions: Some(3),
};

/// `[1,2,3]` in sqlite-vec's float32 layout.
const ONE_TWO_THREE: [u8; 12] = [
    0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x40,
];

/// pgvector's `vector_send`: the dimension and an unused field as
/// big-endian `u16`, then the elements as big-endian `float4`.
fn pg_vector(unused: u16, elements: &[f32]) -> Vec<u8> {
    let dim = u16::try_from(elements.len()).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&dim.to_be_bytes());
    out.extend_from_slice(&unused.to_be_bytes());
    for element in elements {
        out.extend_from_slice(&element.to_be_bytes());
    }
    out
}

fn blob(elements: &[f32]) -> V {
    Value::Blob(elements.iter().flat_map(|e| e.to_le_bytes()).collect())
}

/// Decode pgvector text through a pgoutput text column and a wal2json
/// JSON string, which must agree, and return the agreed result.
fn decode_text_everywhere(wire_type: WireType, text: &str) -> Result<V, DecodeError> {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let cv = ColumnValue::text(text);
    let expected = pg.decode(PgWalstreamColumn {
        column_name: "emb",
        wire_type,
        data: &cv,
    });
    let value = serde_json::Value::String(text.into());
    let got = w2j.decode(Wal2JsonColumn {
        column_name: "emb",
        wire_type,
        value: &value,
    });
    assert_eq!(got, expected, "wal2json {text:?}");
    expected
}

/// Decode `vector_send` bytes through `PgBinary` and a binary-mode
/// pgoutput column, which must agree, and return the agreed result.
fn decode_binary_everywhere(wire_type: WireType, raw: &[u8]) -> Result<V, DecodeError> {
    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let pgw: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let expected = pgb.decode(PgBinaryColumn {
        column_name: "emb",
        wire_type,
        raw: Some(raw),
    });
    let cv = ColumnValue::binary_bytes(bytes::Bytes::copy_from_slice(raw));
    let got = pgw.decode(PgWalstreamColumn {
        column_name: "emb",
        wire_type,
        data: &cv,
    });
    assert_eq!(got, expected, "pg_walstream binary {raw:?}");
    expected
}

fn assert_wrong_payload_kind(result: &Result<V, DecodeError>, input: &dyn core::fmt::Debug) {
    assert!(
        matches!(result, Err(DecodeError::WrongPayloadKind { column, .. }) if column == "emb"),
        "{input:?} -> {result:?}"
    );
}

#[test]
fn reproduction_decodes_to_the_float32_blob_on_every_path() {
    let expected = Ok(Value::Blob(ONE_TWO_THREE.to_vec()));
    for wire_type in [ANY, DIM3] {
        assert_eq!(decode_text_everywhere(wire_type, "[1,2,3]"), expected);
        assert_eq!(
            decode_binary_everywhere(wire_type, &pg_vector(0, &[1.0, 2.0, 3.0])),
            expected
        );
    }
}

#[test]
fn null_is_null_on_every_path() {
    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let pgw: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    assert_eq!(
        pgb.decode(PgBinaryColumn {
            column_name: "emb",
            wire_type: DIM3,
            raw: None,
        }),
        Ok(Value::Null)
    );
    assert_eq!(
        pgw.decode(PgWalstreamColumn {
            column_name: "emb",
            wire_type: DIM3,
            data: &ColumnValue::Null,
        }),
        Ok(Value::Null)
    );
    assert_eq!(
        w2j.decode(Wal2JsonColumn {
            column_name: "emb",
            wire_type: DIM3,
            value: &serde_json::Value::Null,
        }),
        Ok(Value::Null)
    );
}

/// Text and binary carry the same `f32` bits: the shortest decimal
/// pgvector prints reads back to the exact element, and `-0` keeps its sign.
#[test]
fn text_and_binary_agree_on_exact_elements() {
    let elements = [0.1_f32, -0.0, f32::MAX, f32::MIN_POSITIVE, 1e-45, -2.5];
    let text = "[0.1,-0,3.4028235e+38,1.1754944e-38,1e-45,-2.5]";
    let expected = Ok(blob(&elements));
    assert_eq!(decode_text_everywhere(ANY, text), expected);
    assert_eq!(
        decode_binary_everywhere(ANY, &pg_vector(0, &elements)),
        expected
    );
    assert_eq!(
        decode_text_everywhere(ANY, "[-0]"),
        Ok(Value::Blob(vec![0x00, 0x00, 0x00, 0x80]))
    );
}

/// pgvector's `vector_in` takes whitespace around the brackets and the
/// elements, though `vector_out` never prints any.
#[test]
fn text_accepts_whitespace_around_brackets_and_elements() {
    assert_eq!(
        decode_text_everywhere(DIM3, " [ 1 , 2,3 ] "),
        Ok(Value::Blob(ONE_TWO_THREE.to_vec()))
    );
}

#[test]
fn declared_dimension_refuses_other_lengths() {
    for elements in [&[1.0_f32, 2.0][..], &[1.0, 2.0, 3.0, 4.0], &[7.0]] {
        let text = format!(
            "[{}]",
            elements
                .iter()
                .map(f32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let expected = Err(DecodeError::VectorDimensionMismatch {
            column: String::from("emb"),
            expected: 3,
            actual: elements.len(),
        });
        assert_eq!(decode_text_everywhere(DIM3, &text), expected, "{text}");
        assert_eq!(
            decode_binary_everywhere(DIM3, &pg_vector(0, elements)),
            expected,
            "{elements:?}"
        );
    }
}

#[test]
fn undeclared_dimension_accepts_any_length() {
    for elements in [&[7.0_f32][..], &[1.0, 2.0, 3.0, 4.0, 5.0]] {
        let text = format!(
            "[{}]",
            elements
                .iter()
                .map(f32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(decode_text_everywhere(ANY, &text), Ok(blob(elements)));
        assert_eq!(
            decode_binary_everywhere(ANY, &pg_vector(0, elements)),
            Ok(blob(elements))
        );
    }
}

#[test]
fn text_that_is_not_a_pgvector_literal_is_refused() {
    for text in [
        "",
        "1,2,3",
        "[1,2,3",
        "1,2,3]",
        "{1,2,3}",
        "[]",
        "[ ]",
        "[1,,3]",
        "[1,2,]",
        "[,1]",
        "[a,b,c]",
        "[1 2 3]",
        "[1,2,3]x",
        "[[1,2,3]]",
        "[0x1p3]",
    ] {
        let result = decode_text_everywhere(ANY, text);
        assert_wrong_payload_kind(&result, &text);
    }
}

#[test]
fn non_finite_elements_are_refused() {
    for text in ["[NaN]", "[1,inf,3]", "[-Infinity]", "[1e39]", "[-3.5e38]"] {
        let result = decode_text_everywhere(ANY, text);
        assert_wrong_payload_kind(&result, &text);
    }
    for element in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let raw = pg_vector(0, &[1.0, element]);
        let result = decode_binary_everywhere(ANY, &raw);
        assert_wrong_payload_kind(&result, &raw);
    }
}

#[test]
fn malformed_binary_is_refused() {
    let mut truncated = pg_vector(0, &[1.0, 2.0, 3.0]);
    truncated.pop();
    let mut trailing = pg_vector(0, &[1.0, 2.0, 3.0]);
    trailing.push(0);
    for raw in [
        Vec::new(),
        vec![0, 1, 0],
        pg_vector(0, &[]),
        pg_vector(1, &[1.0]),
        truncated,
        trailing,
    ] {
        let result = decode_binary_everywhere(ANY, &raw);
        assert_wrong_payload_kind(&result, &raw);
    }
}

#[test]
fn wal2json_shapes_other_than_a_string_are_refused() {
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    for value in [
        serde_json::json!(1),
        serde_json::json!(true),
        serde_json::json!([1, 2, 3]),
        serde_json::json!({}),
    ] {
        let result = w2j.decode(Wal2JsonColumn {
            column_name: "emb",
            wire_type: ANY,
            value: &value,
        });
        assert_wrong_payload_kind(&result, &value);
    }
}

/// The decoder reads the dimension from the payload's wire type, so under
/// a type that is not a vector it refuses rather than guess one.
#[test]
fn decoder_under_a_type_that_is_not_a_vector_is_refused() {
    let cv = ColumnValue::text("[1,2,3]");
    let result: Result<V, _> = PgWalstreamColumn {
        column_name: "emb",
        wire_type: WireType::Text,
        data: &cv,
    }
    .decoded_by(&VectorFloat32Decoder);
    assert_wrong_payload_kind(&result, &"[1,2,3] as Text");
}

/// One registration under the `Vector` kind serves every dimension.
#[test]
fn one_registration_serves_every_dimension() {
    let mut types: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::new();
    types.register(DIM3.kind(), VectorFloat32Decoder);
    let raw = pg_vector(0, &[1.0, 2.0, 3.0]);
    for (dimensions, ok) in [(None, true), (Some(3), true), (Some(4), false)] {
        let result = types.decode(PgBinaryColumn {
            column_name: "emb",
            wire_type: WireType::Vector { dimensions },
            raw: Some(&raw),
        });
        assert_eq!(result.is_ok(), ok, "{dimensions:?} -> {result:?}");
    }
}

/// MySQL `VECTOR` has no agreed replica form, so Maxwell's defaults leave
/// the kind unregistered rather than guess one.
#[test]
fn maxwell_defaults_register_no_vector_decoder() {
    let mx: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    let value = serde_json::Value::String("[1,2,3]".into());
    let result = mx.decode(MaxwellColumn {
        column_name: "emb",
        wire_type: ANY,
        value: &value,
    });
    assert!(
        matches!(result, Err(DecodeError::NoDecoderForType { .. })),
        "{result:?}"
    );
}
