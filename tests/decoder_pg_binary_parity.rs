//! Cross-source parity guard: the whole reason `PgBinary` exists.
//!
//! A value decoded through `PgBinary` must equal the same value decoded
//! through `PgWalstream`. Where both sources carry the value in Postgres
//! binary send format (bool, int, real, bytea) the comparison is literal
//! binary-mode-vs-binary-mode on identical bytes. Text and uuid arrive over
//! CDC as text, so parity is asserted on the logical value. The numeric,
//! temporal and JSON fixtures pair Postgres's text output for a value with
//! its `*_send` binary form, and pin the replica text that the binary
//! field, the pgoutput text and the pgoutput binary payload all decode to.
//!
//! This test is what keeps a future edit to one source from silently
//! diverging from the other.

#![cfg(feature = "pg-walstream")]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use bytes::Bytes;
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::{
    BoolDecoder, DecodeError, Decoder, IntDecoder, IntervalPostgresDecoder, PgBinary,
    PgBinaryColumn, PgByteaBinaryDecoder, RealDecoder, TextDecoder, TypeMap, UuidBlob16Decoder,
    Value, WireAdapter, WireKind, WireType,
};

const UUID_BYTES: [u8; 16] = [
    0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
];
const UUID_TEXT: &str = "550e8400-e29b-41d4-a716-446655440000";

/// Decode `bytes` through both sources' binary path with `decoder` and
/// assert the two `Value`s are identical.
fn assert_binary_parity<D>(wire_type: WireType, bytes: &[u8], decoder: &D)
where
    D: Decoder<PgBinary, String, Vec<u8>> + Decoder<PgWalstream, String, Vec<u8>>,
{
    let pg_binary: Value<String, Vec<u8>> = PgBinaryColumn {
        column_name: "c",
        wire_type,
        raw: Some(bytes),
    }
    .decoded_by(decoder)
    .unwrap();

    let cv = ColumnValue::Binary(Bytes::copy_from_slice(bytes));
    let pg_walstream: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "c",
        wire_type,
        data: &cv,
    }
    .decoded_by(decoder)
    .unwrap();

    assert_eq!(
        pg_binary, pg_walstream,
        "{wire_type:?} diverges between PgBinary and PgWalstream binary mode"
    );
}

#[test]
fn bool_parity() {
    assert_binary_parity(WireType::Bool, &[0x01], &BoolDecoder);
    assert_binary_parity(WireType::Bool, &[0x00], &BoolDecoder);
}

#[test]
fn int_parity() {
    assert_binary_parity(WireType::Int, &[0x00, 0x05], &IntDecoder);
    assert_binary_parity(WireType::Int, &[0, 0, 0, 42], &IntDecoder);
    assert_binary_parity(WireType::Int, &(-7_i64).to_be_bytes(), &IntDecoder);
}

#[test]
fn real_parity() {
    assert_binary_parity(WireType::Real, &1.5_f32.to_be_bytes(), &RealDecoder);
    assert_binary_parity(WireType::Real, &3.5_f64.to_be_bytes(), &RealDecoder);
}

#[test]
fn bytes_parity() {
    assert_binary_parity(
        WireType::Bytes,
        &[0xDE, 0xAD, 0xBE, 0xEF],
        &PgByteaBinaryDecoder,
    );
}

#[test]
fn text_parity_on_logical_value() {
    // PgBinary carries text as raw UTF-8 bytes; PgWalstream carries it as
    // text-mode ColumnValue. Both must land on the same Value::Text.
    let pg_binary: Value<String, Vec<u8>> = PgBinaryColumn {
        column_name: "c",
        wire_type: WireType::Text,
        raw: Some(b"hello"),
    }
    .decoded_by(&TextDecoder)
    .unwrap();

    let cv = ColumnValue::text("hello");
    let pg_walstream: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Text,
        data: &cv,
    }
    .decoded_by(&TextDecoder)
    .unwrap();

    assert_eq!(pg_binary, pg_walstream);
    assert_eq!(pg_binary, Value::Text(String::from("hello")));
}

#[test]
fn uuid_parity_on_logical_value() {
    // The motivating bug: a uuid must decode to the SAME 16-byte blob
    // whether it arrives as PgBinary raw bytes or PgWalstream text.
    let pg_binary: Value<String, Vec<u8>> = PgBinaryColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        raw: Some(&UUID_BYTES),
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();

    let cv = ColumnValue::text(UUID_TEXT);
    let pg_walstream: Value<String, Vec<u8>> = PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Uuid,
        data: &cv,
    }
    .decoded_by(&UuidBlob16Decoder)
    .unwrap();

    assert_eq!(pg_binary, pg_walstream);
    assert_eq!(pg_binary, Value::Blob(UUID_BYTES.to_vec()));
}

/// `(pgoutput text, binary send form as hex, replica text)`.
type Fixture = (&'static str, &'static str, &'static str);

/// The `TypeMap` of each Postgres source a fixture is decoded through.
struct Sources {
    pg_binary: TypeMap<PgBinary, String, Vec<u8>>,
    walstream: TypeMap<PgWalstream, String, Vec<u8>>,
}

impl Sources {
    fn defaults() -> Self {
        Self {
            pg_binary: TypeMap::defaults(),
            walstream: TypeMap::defaults(),
        }
    }

    /// The defaults with `decoder` registered under `kind` on both sources.
    fn with<D>(kind: WireKind, decoder: D) -> Self
    where
        D: Decoder<PgBinary, String, Vec<u8>>
            + Decoder<PgWalstream, String, Vec<u8>>
            + Copy
            + Send
            + Sync
            + 'static,
    {
        Self {
            pg_binary: TypeMap::defaults().with(kind, decoder),
            walstream: TypeMap::defaults().with(kind, decoder),
        }
    }

    /// Decode one value through `PgBinary`, pgoutput text and pgoutput binary.
    fn decode_all(
        &self,
        wire_type: WireType,
        text: &str,
        hex_bytes: &str,
    ) -> [Result<Value<String, Vec<u8>>, DecodeError>; 3] {
        let binary = Bytes::from(hex::decode(hex_bytes).unwrap());
        let snapshot = self.pg_binary.decode(PgBinaryColumn {
            column_name: "c",
            wire_type,
            raw: Some(&binary),
        });
        let cdc_text = self.walstream.decode(PgWalstreamColumn {
            column_name: "c",
            wire_type,
            data: &ColumnValue::text(text),
        });
        let cdc_binary = self.walstream.decode(PgWalstreamColumn {
            column_name: "c",
            wire_type,
            data: &ColumnValue::Binary(binary),
        });
        [snapshot, cdc_text, cdc_binary]
    }

    fn assert_parity(&self, wire_type: WireType, fixtures: &[Fixture]) {
        for &(text, hex_bytes, replica) in fixtures {
            for (path, got) in ["PgBinary", "pgoutput text", "pgoutput binary"]
                .into_iter()
                .zip(self.decode_all(wire_type, text, hex_bytes))
            {
                assert_eq!(
                    got,
                    Ok(Value::Text(String::from(replica))),
                    "{wire_type:?} {text:?} through {path}"
                );
            }
        }
    }

    fn assert_unrepresentable(&self, wire_type: WireType, fixtures: &[(&str, &str)]) {
        for &(text, hex_bytes) in fixtures {
            for (path, got) in ["PgBinary", "pgoutput text", "pgoutput binary"]
                .into_iter()
                .zip(self.decode_all(wire_type, text, hex_bytes))
            {
                assert!(
                    matches!(got, Err(DecodeError::UnrepresentableTemporal { ref column, .. }) if column == "c"),
                    "{wire_type:?} {text:?} through {path} gave {got:?}"
                );
            }
        }
    }
}

fn assert_parity(wire_type: WireType, fixtures: &[Fixture]) {
    Sources::defaults().assert_parity(wire_type, fixtures);
}

fn assert_unrepresentable(wire_type: WireType, fixtures: &[(&str, &str)]) {
    Sources::defaults().assert_unrepresentable(wire_type, fixtures);
}

#[test]
fn timestamptz_is_utc_with_microseconds() {
    assert_parity(
        WireType::TimestampTz,
        &[
            (
                "2026-09-23 13:42:07.957522+00",
                "0002ff256b07a612",
                "2026-09-23 13:42:07.957522+00:00",
            ),
            (
                "2026-09-23 15:42:07.957522+02",
                "0002ff256b07a612",
                "2026-09-23 13:42:07.957522+00:00",
            ),
            (
                "2026-09-23 15:42:07+02",
                "0002ff256af909c0",
                "2026-09-23 13:42:07.000000+00:00",
            ),
            (
                "2026-09-23 13:42:07.5+00",
                "0002ff256b00aae0",
                "2026-09-23 13:42:07.500000+00:00",
            ),
            (
                "0001-01-01 00:49:56+00:49:56",
                "ff1fe2ffc59c6000",
                "0001-01-01 00:00:00.000000+00:00",
            ),
            (
                "10000-01-01 00:59:59.999999+01",
                "0380e70b913b7fff",
                "9999-12-31 23:59:59.999999+00:00",
            ),
            (
                "2000-01-01 00:59:59.000001+01",
                "fffffffffff0bdc1",
                "1999-12-31 23:59:59.000001+00:00",
            ),
        ],
    );
}

#[test]
fn timestamptz_outside_the_replica_form_is_refused() {
    assert_unrepresentable(
        WireType::TimestampTz,
        &[
            ("infinity", "7fffffffffffffff"),
            ("-infinity", "8000000000000000"),
            ("0044-03-15 12:49:56+00:49:56 BC", "ff1af9e8fb46d000"),
        ],
    );
}

#[test]
fn timestamp_carries_six_fraction_digits() {
    assert_parity(
        WireType::Timestamp,
        &[
            (
                "2026-09-23 13:42:07.957522",
                "0002ff256b07a612",
                "2026-09-23 13:42:07.957522",
            ),
            (
                "2026-09-23 13:42:07",
                "0002ff256af909c0",
                "2026-09-23 13:42:07.000000",
            ),
            (
                "2000-01-01 00:00:00",
                "0000000000000000",
                "2000-01-01 00:00:00.000000",
            ),
            (
                "1970-01-01 00:00:00.5",
                "fffca2fec4cfc120",
                "1970-01-01 00:00:00.500000",
            ),
            (
                "0001-01-01 00:00:00",
                "ff1fe2ffc59c6000",
                "0001-01-01 00:00:00.000000",
            ),
            (
                "9999-12-31 23:59:59.999999",
                "0380e70b913b7fff",
                "9999-12-31 23:59:59.999999",
            ),
        ],
    );
}

#[test]
fn timestamp_outside_the_replica_form_is_refused() {
    assert_unrepresentable(
        WireType::Timestamp,
        &[
            ("infinity", "7fffffffffffffff"),
            ("-infinity", "8000000000000000"),
            ("10000-01-01 00:00:00", "0380e70b913b8000"),
            ("0044-03-15 12:00:00 BC", "ff1af9e8fb46d000"),
        ],
    );
}

#[test]
fn date_is_iso_calendar_date() {
    assert_parity(
        WireType::Date,
        &[
            ("2026-09-23", "00002622", "2026-09-23"),
            ("2000-01-01", "00000000", "2000-01-01"),
            ("1999-12-31", "ffffffff", "1999-12-31"),
            ("0001-01-01", "fff4dbf9", "0001-01-01"),
            ("9999-12-31", "002c95d3", "9999-12-31"),
            ("2024-02-29", "00002279", "2024-02-29"),
        ],
    );
}

#[test]
fn date_outside_the_replica_form_is_refused() {
    assert_unrepresentable(
        WireType::Date,
        &[
            ("infinity", "7fffffff"),
            ("-infinity", "80000000"),
            ("0044-03-15 BC", "fff49d7b"),
            ("10000-01-01", "002c95d4"),
        ],
    );
}

#[test]
fn time_carries_six_fraction_digits() {
    assert_parity(
        WireType::Time,
        &[
            ("13:42:07.957522", "0000000b7c2ce612", "13:42:07.957522"),
            ("00:00:00", "0000000000000000", "00:00:00.000000"),
            ("23:59:59.999999", "000000141dd75fff", "23:59:59.999999"),
            ("13:42:07.5", "0000000b7c25eae0", "13:42:07.500000"),
        ],
    );
}

#[test]
fn time_at_end_of_day_is_refused() {
    assert_unrepresentable(WireType::Time, &[("24:00:00", "000000141dd76000")]);
}

#[test]
fn timetz_is_utc_with_microseconds() {
    assert_parity(
        WireType::Time,
        &[
            (
                "13:42:07.957522+00",
                "0000000b7c2ce61200000000",
                "13:42:07.957522+00:00",
            ),
            (
                "15:42:07+02",
                "0000000d294591c0ffffe3e0",
                "13:42:07.000000+00:00",
            ),
            (
                "00:30:00+05:30",
                "000000006b49d200ffffb2a8",
                "19:00:00.000000+00:00",
            ),
            (
                "23:30:00-01",
                "00000013b28d8e0000000e10",
                "00:30:00.000000+00:00",
            ),
            (
                "12:00:00+05:53:28",
                "0000000a0eebb000ffffad28",
                "06:06:32.000000+00:00",
            ),
        ],
    );
}

/// Postgres's `interval` text under `IntervalStyle = postgres`, its
/// `interval_send` bytes, and its text under `IntervalStyle = iso_8601`.
const INTERVALS: &[Fixture] = &[
    (
        "1 year 2 mons 3 days 04:05:06.789",
        "000000036c97ca88000000030000000e",
        "P1Y2M3DT4H5M6.789S",
    ),
    ("00:00:00", "00000000000000000000000000000000", "PT0S"),
    ("00:15:00", "0000000035a4e9000000000000000000", "PT15M"),
    ("1 day", "00000000000000000000000100000000", "P1D"),
    (
        "-1 years -2 mons",
        "000000000000000000000000fffffff2",
        "P-1Y-2M",
    ),
    (
        "1 day -01:00:00",
        "ffffffff296c5c000000000100000000",
        "P1DT-1H",
    ),
    (
        "-1 days +02:03:00",
        "00000001b7e1dd00ffffffff00000000",
        "P-1DT2H3M",
    ),
    (
        "-1 years +2 days 03:00:00",
        "0000000283baec0000000002fffffff4",
        "P-1Y2DT3H",
    ),
    (
        "1 year -2 days",
        "0000000000000000fffffffe0000000c",
        "P1Y-2D",
    ),
    (
        "-1 mons +1 day -00:00:01",
        "fffffffffff0bdc000000001ffffffff",
        "P-1M1DT-1S",
    ),
    (
        "-1 mons -1 days",
        "0000000000000000ffffffffffffffff",
        "P-1M-1D",
    ),
    (
        "100000:00:00",
        "0001476b081e80000000000000000000",
        "PT100000H",
    ),
    ("26:00:00", "00000015cafea8000000000000000000", "PT26H"),
    (
        "00:00:00.000001",
        "00000000000000010000000000000000",
        "PT0.000001S",
    ),
    ("-00:00:05.5", "ffffffffffac13a00000000000000000", "PT-5.5S"),
    ("-00:00:00.5", "fffffffffff85ee00000000000000000", "PT-0.5S"),
    ("1 mon", "00000000000000000000000000000001", "P1M"),
    ("2 mons", "00000000000000000000000000000002", "P2M"),
    ("1 year", "0000000000000000000000000000000c", "P1Y"),
    ("-1 years", "000000000000000000000000fffffff4", "P-1Y"),
    ("1 year 1 mon", "0000000000000000000000000000000d", "P1Y1M"),
    (
        "1 year 1 mon 1 day 00:00:01",
        "00000000000f4240000000010000000d",
        "P1Y1M1DT1S",
    ),
    ("-3 days", "0000000000000000fffffffd00000000", "P-3D"),
    ("00:01:00", "00000000039387000000000000000000", "PT1M"),
    ("-00:30:00", "ffffffff94b62e000000000000000000", "PT-30M"),
    (
        "-1 years -2 mons -3 days -04:05:06.789",
        "fffffffc93683578fffffffdfffffff2",
        "P-1Y-2M-3DT-4H-5M-6.789S",
    ),
    (
        "178000000 years",
        "0000000000000000000000007f50c600",
        "P178000000Y",
    ),
    (
        "2147483647 days 2562047788:00:54.775807",
        "7fffffffffffffff7fffffff00000000",
        "P2147483647DT2562047788H54.775807S",
    ),
    (
        "-2147483647 days -2562047788:00:54.775807",
        "80000000000000018000000100000000",
        "P-2147483647DT-2562047788H-54.775807S",
    ),
];

const INFINITE_INTERVALS: &[(&str, &str)] = &[
    ("infinity", "7fffffffffffffff7fffffff7fffffff"),
    ("-infinity", "80000000000000008000000080000000"),
];

#[test]
fn interval_by_default_is_iso_8601_duration() {
    assert_parity(WireType::Interval, INTERVALS);
}

#[test]
fn interval_postgres_decoder_stores_postgres_text() {
    let fixtures: Vec<Fixture> = INTERVALS
        .iter()
        .map(|&(text, hex_bytes, _)| (text, hex_bytes, text))
        .collect();
    Sources::with(WireKind::Interval, IntervalPostgresDecoder)
        .assert_parity(WireType::Interval, &fixtures);
}

#[test]
fn infinite_interval_is_refused() {
    assert_unrepresentable(WireType::Interval, INFINITE_INTERVALS);
    Sources::with(WireKind::Interval, IntervalPostgresDecoder)
        .assert_unrepresentable(WireType::Interval, INFINITE_INTERVALS);
}

#[test]
fn numeric_is_postgres_text() {
    assert_parity(
        WireType::Decimal,
        &[
            (
                "1234567890.12345678",
                "0005000200000008000c0d801ed204d2162e",
                "1234567890.12345678",
            ),
            ("-5000.0000", "00010000400000041388", "-5000.0000"),
            ("0", "0000000000000000", "0"),
            ("0.00", "0000000000000002", "0.00"),
            ("0.0001", "0001ffff000000040001", "0.0001"),
            ("-0.000012", "0001fffe4000000604b0", "-0.000012"),
            ("12.34", "0002000000000002000c0d48", "12.34"),
            (
                "100000000000000000000",
                "00010005000000000001",
                "100000000000000000000",
            ),
            ("NaN", "00000000c0000000", "NaN"),
            ("Infinity", "00000000d0000020", "Infinity"),
            ("-Infinity", "00000000f0000020", "-Infinity"),
            ("10000", "00010001000000000001", "10000"),
            ("1.10", "0002000000000002000103e8", "1.10"),
            (
                "99999999.999",
                "0003000100000003270f270f2706",
                "99999999.999",
            ),
        ],
    );
}

#[test]
fn json_keeps_its_text() {
    assert_parity(
        WireType::Json,
        &[(
            r#"{"b": 1,  "a": [1, 2]}"#,
            "7b2262223a20312c20202261223a205b312c20325d7d",
            r#"{"b": 1,  "a": [1, 2]}"#,
        )],
    );
}

#[test]
fn jsonb_is_its_normalized_text() {
    assert_parity(
        WireType::Jsonb,
        &[(
            r#"{"a": [1, 2], "b": 1}"#,
            "017b2261223a205b312c20325d2c202262223a20317d",
            r#"{"a": [1, 2], "b": 1}"#,
        )],
    );
}
