//! Replica text of the temporal decoders on the text-carrying sources.
//!
//! `TimestampTzDecoder` stores UTC as `YYYY-MM-DD HH:MM:SS.ffffff+00:00`,
//! `TimestampDecoder` stores `YYYY-MM-DD HH:MM:SS.ffffff`, `DateDecoder`
//! stores `YYYY-MM-DD`, `TimeDecoder` stores `HH:MM:SS.ffffff` or, for a
//! `timetz`, UTC `HH:MM:SS.ffffff+00:00`. `IntervalIso8601Decoder` stores
//! the ISO 8601 duration Postgres prints under `IntervalStyle = iso_8601`,
//! and `IntervalPostgresDecoder` the text it prints under the default
//! `IntervalStyle = postgres`.

#![cfg(all(feature = "wal2json", feature = "pg-walstream", feature = "maxwell"))]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{Maxwell, MaxwellColumn};
use sqlite_diff_rs::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
use sqlite_diff_rs::wal2json::{Wal2Json, Wal2JsonColumn};
use sqlite_diff_rs::{
    DecodeError, Decoder, IntervalIso8601Decoder, IntervalPostgresDecoder, TypeMap, Value,
    WireAdapter, WireType,
};

type Decoded = Result<Value<String, Vec<u8>>, DecodeError>;

fn pgoutput(wire_type: WireType, text: &str) -> Decoded {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    types.decode(PgWalstreamColumn {
        column_name: "c",
        wire_type,
        data: &ColumnValue::text(text),
    })
}

fn wal2json(wire_type: WireType, text: &str) -> Decoded {
    let types: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::defaults();
    types.decode(Wal2JsonColumn {
        column_name: "c",
        wire_type,
        value: &serde_json::Value::String(text.into()),
    })
}

fn maxwell(wire_type: WireType, text: &str) -> Decoded {
    let types: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::defaults();
    types.decode(MaxwellColumn {
        column_name: "c",
        wire_type,
        value: &serde_json::Value::String(text.into()),
    })
}

fn text(s: &str) -> Value<String, Vec<u8>> {
    Value::Text(String::from(s))
}

fn is_malformed(got: &Decoded) -> bool {
    matches!(got, Err(DecodeError::WrongPayloadKind { column, .. }) if column == "c")
}

fn is_unrepresentable(got: &Decoded) -> bool {
    matches!(got, Err(DecodeError::UnrepresentableTemporal { column, .. }) if column == "c")
}

/// `text` decoded by `decoder` as an `interval` from pgoutput, wal2json
/// and Maxwell.
fn interval_by<D>(decoder: &D, text: &str) -> [Decoded; 3]
where
    D: Decoder<PgWalstream, String, Vec<u8>>
        + Decoder<Wal2Json, String, Vec<u8>>
        + Decoder<Maxwell, String, Vec<u8>>,
{
    let json = serde_json::Value::String(text.into());
    [
        PgWalstreamColumn {
            column_name: "c",
            wire_type: WireType::Interval,
            data: &ColumnValue::text(text),
        }
        .decoded_by(decoder),
        Wal2JsonColumn {
            column_name: "c",
            wire_type: WireType::Interval,
            value: &json,
        }
        .decoded_by(decoder),
        MaxwellColumn {
            column_name: "c",
            wire_type: WireType::Interval,
            value: &json,
        }
        .decoded_by(decoder),
    ]
}

#[test]
fn timestamptz_offsets_become_utc_on_every_source() {
    for (wire, replica) in [
        (
            "2026-09-23 13:42:07.957522+00",
            "2026-09-23 13:42:07.957522+00:00",
        ),
        ("2026-09-23 15:42:07+02", "2026-09-23 13:42:07.000000+00:00"),
        (
            "2026-09-23 19:12:07.5+05:30",
            "2026-09-23 13:42:07.500000+00:00",
        ),
        ("2026-09-23 13:42:07Z", "2026-09-23 13:42:07.000000+00:00"),
        (
            "2026-09-23 08:42:07-05:00",
            "2026-09-23 13:42:07.000000+00:00",
        ),
        (
            "2026-09-24 01:12:07+11:30",
            "2026-09-23 13:42:07.000000+00:00",
        ),
        ("2026-01-01 00:30:00+01", "2025-12-31 23:30:00.000000+00:00"),
    ] {
        assert_eq!(
            pgoutput(WireType::TimestampTz, wire),
            Ok(text(replica)),
            "{wire}"
        );
        assert_eq!(
            wal2json(WireType::TimestampTz, wire),
            Ok(text(replica)),
            "{wire}"
        );
        assert_eq!(
            maxwell(WireType::TimestampTz, wire),
            Ok(text(replica)),
            "{wire}"
        );
    }
}

#[test]
fn timestamptz_without_offset_is_utc_only_on_maxwell() {
    let bare = "2026-09-23 13:42:07.5";
    assert_eq!(
        maxwell(WireType::TimestampTz, bare),
        Ok(text("2026-09-23 13:42:07.500000+00:00"))
    );
    assert!(is_malformed(&pgoutput(WireType::TimestampTz, bare)));
    assert!(is_malformed(&wal2json(WireType::TimestampTz, bare)));
}

#[test]
fn timestamp_fraction_is_padded_to_microseconds() {
    for (wire, replica) in [
        ("2024-01-15 10:30:00", "2024-01-15 10:30:00.000000"),
        ("2024-01-15 10:30:00.123456", "2024-01-15 10:30:00.123456"),
        ("2024-01-15 10:30:00.1", "2024-01-15 10:30:00.100000"),
    ] {
        assert_eq!(
            pgoutput(WireType::Timestamp, wire),
            Ok(text(replica)),
            "{wire}"
        );
        assert_eq!(
            wal2json(WireType::Timestamp, wire),
            Ok(text(replica)),
            "{wire}"
        );
        assert_eq!(
            maxwell(WireType::Timestamp, wire),
            Ok(text(replica)),
            "{wire}"
        );
    }
}

#[test]
fn date_time_and_timetz_on_every_source() {
    for (wire_type, wire, replica) in [
        (WireType::Date, "2024-05-14", "2024-05-14"),
        (WireType::Time, "15:30:45.123", "15:30:45.123000"),
        (WireType::Time, "15:30:45", "15:30:45.000000"),
        (WireType::Time, "15:30:45+02", "13:30:45.000000+00:00"),
    ] {
        assert_eq!(pgoutput(wire_type, wire), Ok(text(replica)), "{wire}");
        assert_eq!(wal2json(wire_type, wire), Ok(text(replica)), "{wire}");
        assert_eq!(maxwell(wire_type, wire), Ok(text(replica)), "{wire}");
    }
}

#[test]
fn interval_postgres_style_becomes_iso_8601() {
    for (wire, replica) in [
        ("1 year 2 mons 3 days 04:05:06.789", "P1Y2M3DT4H5M6.789S"),
        ("1 day", "P1D"),
        ("2 years", "P2Y"),
        ("00:00:00", "PT0S"),
    ] {
        assert_eq!(
            pgoutput(WireType::Interval, wire),
            Ok(text(replica)),
            "{wire}"
        );
        assert_eq!(
            wal2json(WireType::Interval, wire),
            Ok(text(replica)),
            "{wire}"
        );
        for got in interval_by(&IntervalIso8601Decoder, wire) {
            assert_eq!(got, Ok(text(replica)), "{wire}");
        }
    }
}

#[test]
fn interval_postgres_decoder_stores_postgres_text_on_every_source() {
    for wire in [
        "00:15:00",
        "1 year 2 mons 3 days 04:05:06.789",
        "1 day",
        "00:00:00",
        "-1 days +02:00:00",
        "-1 years +2 days 03:00:00",
        "-1 years -2 mons -3 days -04:05:06.789",
    ] {
        for got in interval_by(&IntervalPostgresDecoder, wire) {
            assert_eq!(got, Ok(text(wire)), "{wire}");
        }
    }
}

#[test]
fn infinite_interval_is_refused_by_both_decoders() {
    for wire in ["infinity", "-infinity"] {
        let iso = interval_by(&IntervalIso8601Decoder, wire);
        let postgres = interval_by(&IntervalPostgresDecoder, wire);
        for got in iso.iter().chain(&postgres) {
            assert!(is_unrepresentable(got), "{wire} gave {got:?}");
        }
    }
}

#[test]
fn interval_postgres_decoder_passes_null_through() {
    let got: Decoded = PgWalstreamColumn {
        column_name: "c",
        wire_type: WireType::Interval,
        data: &ColumnValue::Null,
    }
    .decoded_by(&IntervalPostgresDecoder);
    assert_eq!(got, Ok(Value::Null));
}

#[test]
fn interval_in_another_interval_style_is_refused() {
    for wire in [
        "P1Y2M3DT4H5M6.789S",
        "PT15M",
        "@ 1 year 2 mons",
        "@ 15 mins",
        "1-2 3 4:05:06.789",
        "1 year 2 months",
        "1 day ago",
    ] {
        let iso = interval_by(&IntervalIso8601Decoder, wire);
        let postgres = interval_by(&IntervalPostgresDecoder, wire);
        for got in iso.iter().chain(&postgres) {
            assert!(is_malformed(got), "{wire} gave {got:?}");
        }
    }
}

#[test]
fn malformed_temporal_text_is_refused() {
    for (wire_type, wire) in [
        (WireType::Date, "2024-02-30"),
        (WireType::Date, "2024-13-01"),
        (WireType::Date, "24-01-01"),
        (WireType::Date, "2024-01-01 "),
        (WireType::Timestamp, "2024-01-15T10:30:00"),
        (WireType::Timestamp, "2024-01-15 10:60:00"),
        (WireType::Timestamp, "2024-01-15 10:30:00.1234567"),
        (WireType::Timestamp, "2024-01-15 10:30:00+00"),
        (WireType::TimestampTz, "2024-01-15 10:30:00+24"),
        (WireType::TimestampTz, "2024-01-15 10:30:00+0200"),
        (WireType::Time, "25:00:00"),
        (WireType::Time, "-01:00:00"),
        (WireType::Time, "10:30"),
    ] {
        assert!(
            is_malformed(&pgoutput(wire_type, wire)),
            "{wire_type:?} {wire}"
        );
    }
}

#[test]
fn temporal_values_a_replica_cannot_hold_are_refused() {
    for (wire_type, wire) in [
        (WireType::Date, "infinity"),
        (WireType::Date, "-infinity"),
        (WireType::Date, "0044-03-15 BC"),
        (WireType::Date, "10000-01-01"),
        (WireType::Date, "0000-01-01"),
        (WireType::Timestamp, "infinity"),
        (WireType::Timestamp, "10000-01-01 00:00:00"),
        (WireType::TimestampTz, "0001-01-01 00:30:00+01"),
        (WireType::TimestampTz, "9999-12-31 23:30:00-01"),
        (WireType::TimestampTz, "-infinity"),
        (WireType::Time, "24:00:00"),
    ] {
        assert!(
            is_unrepresentable(&pgoutput(wire_type, wire)),
            "{wire_type:?} {wire}"
        );
    }
}

#[test]
fn temporal_null_passes_through() {
    let types: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::defaults();
    for wire_type in [
        WireType::Date,
        WireType::Time,
        WireType::Timestamp,
        WireType::TimestampTz,
        WireType::Interval,
    ] {
        let got = types.decode(PgWalstreamColumn {
            column_name: "c",
            wire_type,
            data: &ColumnValue::Null,
        });
        assert_eq!(got, Ok(Value::Null), "{wire_type:?}");
    }
}
