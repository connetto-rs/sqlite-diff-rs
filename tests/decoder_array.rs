//! Tests for `ArrayJsonDecoder`, the default decoder for `WireType::Array`.
//!
//! A PostgreSQL array decodes to the JSON array text a replica stores, the
//! same whether it arrives as pgoutput text, a wal2json string, pgoutput
//! binary or a `PgBinary` query result. Each element decodes through the
//! decoder its type has in the `TypeMap`, so it is stored as a scalar column
//! of that type is.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "testing"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    ArrayElement, ArrayJsonDecoder, DecodeError, Decoder, Int64OverflowToTextDecoder,
    IntervalPostgresDecoder, PgBinary, PgBinaryColumn, TypeMap, UuidBlob16Decoder, Value,
    WireAdapter, WireKind, WireType,
};

type V = Value<String, Vec<u8>>;

const INT4: u32 = 23;
const FLOAT8: u32 = 701;

fn array(element: ArrayElement) -> WireType {
    WireType::Array { element }
}

fn json(s: &str) -> V {
    Value::Text(String::from(s))
}

/// PostgreSQL's binary `array_send` form: the dimension count, the
/// has-nulls flag and the element OID, a length and lower bound per
/// dimension, then each element's length, `-1` for `NULL`, and bytes.
fn pg_array(oid: u32, dims: &[(i32, i32)], elements: &[Option<&[u8]>]) -> Vec<u8> {
    let has_nulls = elements.iter().any(Option::is_none);
    let mut out = Vec::new();
    out.extend_from_slice(&i32::try_from(dims.len()).unwrap().to_be_bytes());
    out.extend_from_slice(&i32::from(has_nulls).to_be_bytes());
    out.extend_from_slice(&oid.to_be_bytes());
    for (length, lower) in dims {
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&lower.to_be_bytes());
    }
    for element in elements {
        match element {
            Some(bytes) => {
                out.extend_from_slice(&i32::try_from(bytes.len()).unwrap().to_be_bytes());
                out.extend_from_slice(bytes);
            }
            None => out.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    out
}

/// Decode array text through pgoutput text mode and a wal2json string,
/// which must agree, and return the agreed result.
fn decode_text_everywhere(element: ArrayElement, s: &str) -> Result<V, DecodeError> {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let cv = ColumnValue::text(s);
    let expected = pg.decode(PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(element),
        data: &cv,
    });
    let value = serde_json::Value::String(s.into());
    let got = w2j.decode(Wal2JsonColumn {
        column_name: "tags",
        wire_type: array(element),
        value: &value,
    });
    assert_eq!(got, expected, "wal2json {s:?}");
    expected
}

/// Decode `array_send` bytes through `PgBinary` and pgoutput binary mode,
/// which must agree, and return the agreed result.
fn decode_binary_everywhere(element: ArrayElement, raw: &[u8]) -> Result<V, DecodeError> {
    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let pgw: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let expected = pgb.decode(PgBinaryColumn {
        column_name: "tags",
        wire_type: array(element),
        raw: Some(raw),
    });
    let cv = ColumnValue::binary_bytes(bytes::Bytes::copy_from_slice(raw));
    let got = pgw.decode(PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(element),
        data: &cv,
    });
    assert_eq!(got, expected, "pg_walstream binary {raw:?}");
    expected
}

fn assert_malformed_text(result: Result<V, DecodeError>) {
    match result {
        Err(DecodeError::WrongPayloadKind { column, .. }) => assert_eq!(column, "tags"),
        other => panic!("expected a refusal naming the column, got {other:?}"),
    }
}

fn assert_unrepresentable(result: Result<V, DecodeError>, reason: &str) {
    let expected = DecodeError::UnrepresentableArray {
        column: String::from("tags"),
        reason: match reason {
            "lower bound" => "a lower bound other than 1",
            _ => "an element decoding to a blob",
        },
    };
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(decoded) => panic!("decoded to {decoded:?}, expected {expected:?}"),
    }
}

#[test]
fn reproduction_decodes_identically_from_every_source() {
    let binary = pg_array(
        INT4,
        &[(2, 1)],
        &[Some(&1i32.to_be_bytes()), Some(&2i32.to_be_bytes())],
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{1,2}"),
        Ok(json("[1,2]"))
    );
    assert_eq!(
        decode_binary_everywhere(ArrayElement::Int, &binary),
        Ok(json("[1,2]"))
    );
}

#[test]
fn null_elements_and_null_arrays() {
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{1,NULL,3}"),
        Ok(json("[1,null,3]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Text, "{null,\"NULL\"}"),
        Ok(json("[null,\"NULL\"]"))
    );
    let binary = pg_array(
        INT4,
        &[(3, 1)],
        &[Some(&1i32.to_be_bytes()), None, Some(&3i32.to_be_bytes())],
    );
    assert_eq!(
        decode_binary_everywhere(ArrayElement::Int, &binary),
        Ok(json("[1,null,3]"))
    );

    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let null = pgb.decode(PgBinaryColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        raw: None,
    });
    assert_eq!(null, Ok(Value::Null));
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let null = w2j.decode(Wal2JsonColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        value: &serde_json::Value::Null,
    });
    assert_eq!(null, Ok(Value::Null));
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let null = pg.decode(PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        data: &ColumnValue::Null,
    });
    assert_eq!(null, Ok(Value::Null));
}

#[test]
fn text_elements_lose_array_quoting() {
    assert_eq!(
        decode_text_everywhere(ArrayElement::Text, "{\"a b\",c}"),
        Ok(json(r#"["a b","c"]"#))
    );
    assert_eq!(
        decode_text_everywhere(
            ArrayElement::Text,
            r#"{"say \"hi\"","back\\slash",x\,y,"",  pad  }"#
        ),
        Ok(json(r#"["say \"hi\"","back\\slash","x,y","","pad"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Text, "{a\\ ,\" b \"}"),
        Ok(json(r#"["a "," b "]"#))
    );
    let pgb: TypeMap<PgBinary, String, Vec<u8>> = TypeMap::defaults();
    let raw = pg_array(25, &[(1, 1)], &[Some("é\n".as_bytes())]);
    let binary = pgb.decode(PgBinaryColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Text),
        raw: Some(&raw),
    });
    assert_eq!(binary, Ok(json("[\"é\\n\"]")));
}

#[test]
fn json_text_matches_sqlite_json_array() {
    let strings = [
        "plain",
        "a \"quoted\" \\ word",
        "tab\tnew\nline\r\u{8}\u{c}",
        "\u{1}\u{1f}\u{7f}",
        "é ∑ 𝄞",
        "",
    ];
    let mut literal = String::from("{");
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            literal.push(',');
        }
        literal.push('"');
        for c in s.chars() {
            if matches!(c, '"' | '\\') {
                literal.push('\\');
            }
            literal.push(c);
        }
        literal.push('"');
    }
    literal.push('}');
    let Ok(Value::Text(ours)) = decode_text_everywhere(ArrayElement::Text, &literal) else {
        panic!("text array refused");
    };

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let sqlite: String = conn
        .query_row(
            "SELECT json_array(?1, ?2, ?3, ?4, ?5, ?6)",
            strings,
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ours, sqlite);

    let nested: String = conn
        .query_row(
            "SELECT json_array(json_array(1, NULL), json_array(-3, 4))",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{{1,NULL},{-3,4}}"),
        Ok(Value::Text(nested))
    );
}

#[test]
fn multidimensional_arrays_nest() {
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{{1,2},{3,4}}"),
        Ok(json("[[1,2],[3,4]]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, " { { 1 , 2 } , { 3 , 4 } } "),
        Ok(json("[[1,2],[3,4]]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "[1:2][1:1]={{1},{2}}"),
        Ok(json("[[1],[2]]"))
    );
    let four: Vec<[u8; 4]> = (1..=4i32).map(i32::to_be_bytes).collect();
    let elements: Vec<Option<&[u8]>> = four.iter().map(|b| Some(&b[..])).collect();
    let binary = pg_array(INT4, &[(2, 1), (2, 1)], &elements);
    assert_eq!(
        decode_binary_everywhere(ArrayElement::Int, &binary),
        Ok(json("[[1,2],[3,4]]"))
    );
    let binary = pg_array(INT4, &[(1, 1), (1, 1), (4, 1)], &elements);
    assert_eq!(
        decode_binary_everywhere(ArrayElement::Int, &binary),
        Ok(json("[[[1,2,3,4]]]"))
    );
}

#[test]
fn empty_arrays_are_empty_json_arrays() {
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{}"),
        Ok(json("[]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{{},{}}"),
        Ok(json("[]"))
    );
    assert_eq!(
        decode_binary_everywhere(ArrayElement::Int, &pg_array(INT4, &[], &[])),
        Ok(json("[]"))
    );
}

#[test]
fn malformed_text_is_refused_naming_the_column() {
    for s in [
        "",
        "1,2",
        "{1,2",
        "{1,,2}",
        "{1,}",
        "{,1}",
        "{{1},2}",
        "{1,{2}}",
        "{{1,2},{3}}",
        "{{1},{}}",
        "{\"a\"b}",
        "{a\"b}",
        "{a{b}",
        "{1}x",
        "{\"open}",
        "{\"open\\",
        "{1\\",
        "[1:2]{1,2}",
        "[1:0]={}",
        "[x]={1}",
        "[99999999999]={1}",
        "[1:1][1:1][1:1][1:1][1:1][1:1][1:1]={{{{{{{1}}}}}}}",
        "{",
        "{ ",
        "{{1} 2}",
        "[1:3]={1,2}",
        "[1:2][1:2]={1,2}",
        "{{{{{{{1}}}}}}}",
    ] {
        assert_malformed_text(decode_text_everywhere(ArrayElement::Int, s));
    }
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    for (element, s) in [
        (ArrayElement::Int, "{1,two}"),
        (ArrayElement::Int, "{1 2}"),
        (ArrayElement::Bool, "{t,maybe}"),
    ] {
        let cv = ColumnValue::text(s);
        assert_malformed_text(pg.decode(PgWalstreamColumn {
            column_name: "tags",
            wire_type: array(element),
            data: &cv,
        }));
        assert_malformed_text(w2j.decode(Wal2JsonColumn {
            column_name: "tags",
            wire_type: array(element),
            value: &serde_json::json!(s),
        }));
    }
    assert_eq!(
        decode_text_everywhere(ArrayElement::Int, "{99999999999999999999}"),
        Err(DecodeError::IntegerOverflow {
            column: String::from("tags"),
            digits: String::from("99999999999999999999"),
        })
    );
    assert_malformed_text(w2j.decode(Wal2JsonColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        value: &serde_json::json!([1, 2]),
    }));
}

#[test]
fn malformed_binary_is_refused_naming_the_column() {
    let one = 1i32.to_be_bytes();
    let good = pg_array(INT4, &[(2, 1)], &[Some(&one), Some(&one)]);
    let seven_dims = pg_array(INT4, &[(1, 1); 7], &[Some(&one)]);
    let mut bad_flags = good.clone();
    bad_flags[7] = 2;
    let mut short_element = pg_array(INT4, &[(1, 1)], &[Some(&one)]);
    short_element.truncate(short_element.len() - 1);
    let mut trailing = good.clone();
    trailing.push(0);
    let mut below_minus_one = pg_array(INT4, &[(1, 1)], &[]);
    below_minus_one.extend_from_slice(&(-2i32).to_be_bytes());
    for raw in [
        &good[..11],
        &seven_dims[..],
        &bad_flags[..],
        &short_element[..],
        &trailing[..],
        &below_minus_one[..],
        &pg_array(INT4, &[(-1, 1)], &[])[..],
        &pg_array(INT4, &[(3, 1)], &[Some(&one)])[..],
        &pg_array(INT4, &[(i32::MAX, 1), (i32::MAX, 1), (i32::MAX, 1)], &[])[..],
        &pg_array(INT4, &[(1, 1)], &[Some(&[1, 2, 3])])[..],
        &[pg_array(INT4, &[], &[]), vec![0]].concat()[..],
        &[pg_array(INT4, &[(0, 1)], &[]), vec![0]].concat()[..],
    ] {
        assert_malformed_text(decode_binary_everywhere(ArrayElement::Int, raw));
    }
}

#[test]
fn lower_bounds_other_than_one_are_refused() {
    assert_unrepresentable(
        decode_text_everywhere(ArrayElement::Int, "[0:1]={1,2}"),
        "lower bound",
    );
    assert_unrepresentable(
        decode_text_everywhere(ArrayElement::Int, "[1:1][-1:0]={{1,2}}"),
        "lower bound",
    );
    let one = 1i32.to_be_bytes();
    let binary = pg_array(INT4, &[(2, 0)], &[Some(&one), Some(&one)]);
    assert_unrepresentable(
        decode_binary_everywhere(ArrayElement::Int, &binary),
        "lower bound",
    );
}

#[test]
fn elements_decode_through_the_registered_scalar_decoder() {
    let cv = ColumnValue::text("{99999999999999999999,1}");
    let payload = PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        data: &cv,
    };
    let overflow_to_text: TypeMap<PgWalstream, String, Vec<u8>> =
        TypeMap::defaults().with(WireKind::Int, Int64OverflowToTextDecoder);
    assert_eq!(
        overflow_to_text.decode(payload),
        Ok(json(r#"["99999999999999999999",1]"#))
    );

    let cv = ColumnValue::text(r#"{"1 year 2 mons 3 days 04:05:06.789","-1 days +02:00:00"}"#);
    let postgres_style: TypeMap<PgWalstream, String, Vec<u8>> =
        TypeMap::defaults().with(WireKind::Interval, IntervalPostgresDecoder);
    assert_eq!(
        postgres_style.decode(PgWalstreamColumn {
            column_name: "tags",
            wire_type: array(ArrayElement::Interval),
            data: &cv,
        }),
        Ok(json(
            r#"["1 year 2 mons 3 days 04:05:06.789","-1 days +02:00:00"]"#
        ))
    );

    let uuid = "{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11,NULL}";
    assert_eq!(
        decode_text_everywhere(ArrayElement::Uuid, uuid),
        Ok(json(r#"["a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",null]"#))
    );
    let cv = ColumnValue::text(uuid);
    let as_blob: TypeMap<PgWalstream, String, Vec<u8>> =
        TypeMap::defaults().with(WireKind::Uuid, UuidBlob16Decoder);
    assert_unrepresentable(
        as_blob.decode(PgWalstreamColumn {
            column_name: "tags",
            wire_type: array(ArrayElement::Uuid),
            data: &cv,
        }),
        "blob",
    );
    assert_unrepresentable(
        decode_text_everywhere(ArrayElement::Bytes, "{\"\\\\x00ff\"}"),
        "blob",
    );
}

#[test]
fn elements_take_their_scalar_storage() {
    assert_eq!(
        decode_text_everywhere(ArrayElement::Bool, "{t,f,NULL}"),
        Ok(json("[1,0,null]"))
    );
    assert_eq!(
        decode_binary_everywhere(
            ArrayElement::Bool,
            &pg_array(16, &[(2, 1)], &[Some(&[1]), Some(&[0])])
        ),
        Ok(json("[1,0]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Real, "{1.5,0.1,-0,1e+300,0.3333333333333333}"),
        Ok(json("[1.5,0.1,-0.0,1e300,0.3333333333333333]"))
    );
    let third = (1.0f64 / 3.0).to_be_bytes();
    assert_eq!(
        decode_binary_everywhere(
            ArrayElement::Real,
            &pg_array(FLOAT8, &[(1, 1)], &[Some(&third)])
        ),
        Ok(json("[0.3333333333333333]"))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Decimal, "{1.50,-2}"),
        Ok(json(r#"["1.50","-2"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::TimestampTz, "{\"2024-01-02 03:04:05.6+02\"}"),
        Ok(json(r#"["2024-01-02 01:04:05.600000+00:00"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Date, "{2024-03-05}"),
        Ok(json(r#"["2024-03-05"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Jsonb, r#"{"{\"a\": 1}"}"#),
        Ok(json(r#"["{\"a\": 1}"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Json, r#"{"[1, 2]",null}"#),
        Ok(json(r#"["[1, 2]",null]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Timestamp, r#"{"2024-01-02 03:04:05"}"#),
        Ok(json(r#"["2024-01-02 03:04:05.000000"]"#))
    );
    assert_eq!(
        decode_text_everywhere(ArrayElement::Time, "{04:05:06.5}"),
        Ok(json(r#"["04:05:06.500000"]"#))
    );
    assert_eq!(
        decode_text_everywhere(
            ArrayElement::Interval,
            r#"{"1 year 2 mons 3 days 04:05:06.789"}"#
        ),
        Ok(json(r#"["P1Y2M3DT4H5M6.789S"]"#))
    );
}

/// Decodes every `float8` to NaN, which no built-in decoder yields.
struct NanDecoder;

impl Decoder<PgBinary, String, Vec<u8>> for NanDecoder {
    fn decode(&self, _payload: PgBinaryColumn<'_>) -> Result<V, DecodeError> {
        Ok(Value::Real(f64::NAN))
    }
}

#[test]
fn a_registered_decoder_yielding_nan_writes_null() {
    let raw = pg_array(FLOAT8, &[(1, 1)], &[Some(&1.5f64.to_be_bytes())]);
    let nan: TypeMap<PgBinary, String, Vec<u8>> =
        TypeMap::defaults().with(WireKind::Real, NanDecoder);
    let decoded = nan.decode(PgBinaryColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Real),
        raw: Some(&raw),
    });
    assert_eq!(decoded, Ok(json("[null]")));
}

#[test]
fn array_decoder_under_another_kind_is_refused() {
    let misregistered: TypeMap<PgBinary, String, Vec<u8>> =
        TypeMap::new().with(WireKind::Text, ArrayJsonDecoder);
    let decoded = misregistered.decode(PgBinaryColumn {
        column_name: "tags",
        wire_type: WireType::Text,
        raw: Some(b"{1}"),
    });
    assert_eq!(
        decoded,
        Err(DecodeError::WrongPayloadKind {
            column: String::from("tags"),
            expected: "an array wire type",
            actual: "a wire type that is not an array",
        })
    );
}

#[test]
fn non_finite_reals_follow_each_source() {
    let pg: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    let cv = ColumnValue::text("{NaN,Infinity,-Infinity}");
    let pgoutput = pg.decode(PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Real),
        data: &cv,
    });
    assert_eq!(pgoutput, Ok(json("[null,9.0e+999,-9.0e+999]")));

    let w2j: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    let wal2json = w2j.decode(Wal2JsonColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Real),
        value: &serde_json::json!("{NaN,Infinity,-Infinity}"),
    });
    assert_eq!(wal2json, Ok(json("[null,null,null]")));
}

#[test]
fn direct_calls_decode_elements_through_the_defaults() {
    let raw = pg_array(INT4, &[(1, 1)], &[Some(&7i32.to_be_bytes())]);
    let direct: Result<V, _> = PgBinaryColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        raw: Some(&raw),
    }
    .decoded_by(&ArrayJsonDecoder);
    assert_eq!(direct, Ok(json("[7]")));

    let cv = ColumnValue::text("{7}");
    let direct: Result<V, _> = PgWalstreamColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        data: &cv,
    }
    .decoded_by(&ArrayJsonDecoder);
    assert_eq!(direct, Ok(json("[7]")));

    let value = serde_json::json!("{7}");
    let direct: Result<V, _> = Wal2JsonColumn {
        column_name: "tags",
        wire_type: array(ArrayElement::Int),
        value: &value,
    }
    .decoded_by(&ArrayJsonDecoder);
    assert_eq!(direct, Ok(json("[7]")));
}
