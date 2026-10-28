//! Postgres's text output under `DateStyle = ISO` and
//! `IntervalStyle = postgres`, the server defaults, parsed into replica text.

use alloc::string::{String, ToString};

use super::{
    Failure, IntervalStyle, MAX_OFFSET_SECS, PG_EPOCH_UNIX_DAYS, Temporal, USECS_PER_DAY,
    USECS_PER_HOUR, USECS_PER_MINUTE, USECS_PER_SEC, render_date, render_interval, render_time,
    render_timestamp, render_timetz,
};
use crate::encoding::Value;
use crate::wire::error::DecodeError;

/// Render Postgres's text output of a temporal value as replica text.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for text Postgres would not print,
/// [`DecodeError::UnrepresentableTemporal`] for a value outside the
/// replica form.
pub(crate) fn decode_temporal_text<S, B>(
    column: &str,
    kind: Temporal,
    text: &str,
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String>,
{
    let rendered = match kind {
        Temporal::Date => date_from_text(text),
        Temporal::Time => time_from_text(text),
        Temporal::Timestamp => timestamp_from_text(text),
        Temporal::TimestampTz => timestamptz_from_text(text),
        Temporal::Interval(style) => interval_from_text(text, style),
    };
    finish(column, kind, text, rendered)
}

/// [`decode_temporal_text`] for a `timestamptz` whose text may omit the
/// offset to mean UTC, as Maxwell prints a MySQL `TIMESTAMP`.
///
/// # Errors
///
/// As [`decode_temporal_text`].
#[cfg(feature = "maxwell")]
pub(crate) fn decode_utc_timestamptz_text<S, B>(
    column: &str,
    text: &str,
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String>,
{
    let rendered = parse_timestamp(text).and_then(|(local, offset)| match offset {
        Some(offset) => render_utc(local, offset),
        None => render_timestamp(local, true),
    });
    finish(column, Temporal::TimestampTz, text, rendered)
}

fn finish<S, B>(
    column: &str,
    kind: Temporal,
    text: &str,
    rendered: Result<String, Failure>,
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String>,
{
    match rendered {
        Ok(s) => Ok(Value::Text(S::from(s))),
        Err(Failure::Malformed) => Err(DecodeError::WrongPayloadKind {
            column: column.to_string(),
            expected: match kind {
                Temporal::Date => "ISO date text",
                Temporal::Time => "ISO time or timetz text",
                Temporal::Timestamp => "ISO timestamp text",
                Temporal::TimestampTz => "ISO timestamptz text",
                Temporal::Interval(_) => "postgres-style interval text",
            },
            actual: "malformed text",
        }),
        Err(Failure::Unrepresentable) => Err(DecodeError::UnrepresentableTemporal {
            column: column.to_string(),
            value: text.to_string(),
        }),
    }
}

/// Day count from the Unix epoch of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Postgres's spelling of values no replica form holds.
fn is_unrepresentable_text(text: &str) -> bool {
    text == "infinity" || text == "-infinity" || text.ends_with(" BC")
}

/// A run of ASCII digits of exactly `len` bytes, when `len` is nonzero,
/// or of any length from 1 to 18 otherwise.
fn digits(text: &str, len: usize) -> Result<i64, Failure> {
    let ok_len = if len == 0 {
        (1..=18).contains(&text.len())
    } else {
        text.len() == len
    };
    if !ok_len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Failure::Malformed);
    }
    Ok(text
        .bytes()
        .fold(0, |acc, b| acc * 10 + i64::from(b - b'0')))
}

/// `YYYY-MM-DD` as a day count from the Postgres epoch. A five-digit year
/// parses because a `timestamptz` in year 10000 can still be 9999 in UTC.
fn parse_date(text: &str) -> Result<i64, Failure> {
    let mut fields = text.splitn(3, '-');
    let (Some(year), Some(month), Some(day)) = (fields.next(), fields.next(), fields.next()) else {
        return Err(Failure::Malformed);
    };
    let year = match year.len() {
        4 | 5 => digits(year, 0)?,
        6.. if year.bytes().all(|b| b.is_ascii_digit()) => return Err(Failure::Unrepresentable),
        _ => return Err(Failure::Malformed),
    };
    let (month, day) = (digits(month, 2)?, digits(day, 2)?);
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return Err(Failure::Malformed);
    }
    Ok(days_from_civil(year, month, day) - PG_EPOCH_UNIX_DAYS)
}

/// `HH:MM:SS[.f]` with one to six fraction digits, as microseconds from
/// midnight. `24:00:00` is the end of the day.
fn parse_time_of_day(text: &str) -> Result<i64, Failure> {
    let (hours, rest) = text.split_once(':').ok_or(Failure::Malformed)?;
    let micros = digits(hours, 2)? * USECS_PER_HOUR + parse_minutes_seconds(rest)?;
    if micros > USECS_PER_DAY {
        return Err(Failure::Malformed);
    }
    Ok(micros)
}

/// `MM:SS[.f]` with one to six fraction digits, as microseconds.
fn parse_minutes_seconds(text: &str) -> Result<i64, Failure> {
    let (whole, frac) = match text.split_once('.') {
        Some((whole, frac)) if (1..=6).contains(&frac.len()) => (whole, frac),
        Some(_) => return Err(Failure::Malformed),
        None => (text, ""),
    };
    let (minutes, secs) = whole.split_once(':').ok_or(Failure::Malformed)?;
    let (minutes, secs) = (digits(minutes, 2)?, digits(secs, 2)?);
    if minutes > 59 || secs > 59 {
        return Err(Failure::Malformed);
    }
    let frac_micros = if frac.is_empty() {
        0
    } else {
        digits(frac, frac.len())? * 10_i64.pow(6 - u32::try_from(frac.len()).unwrap())
    };
    Ok(minutes * USECS_PER_MINUTE + secs * USECS_PER_SEC + frac_micros)
}

/// `Z`, `+HH`, `+HH:MM` or `+HH:MM:SS` (or `-`), as seconds east of UTC.
fn parse_offset(text: &str) -> Result<i64, Failure> {
    if text == "Z" {
        return Ok(0);
    }
    let sign = match text.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(Failure::Malformed),
    };
    let mut fields = text[1..].split(':');
    let hours = digits(fields.next().unwrap_or(""), 2)?;
    let minutes = fields.next().map_or(Ok(0), |f| digits(f, 2))?;
    let secs = fields.next().map_or(Ok(0), |f| digits(f, 2))?;
    let total = hours * 3600 + minutes * 60 + secs;
    if fields.next().is_some() || minutes > 59 || secs > 59 || total > MAX_OFFSET_SECS {
        return Err(Failure::Malformed);
    }
    Ok(sign * total)
}

/// Split `HH:MM:SS[.f]<offset>` at the offset's first byte.
fn split_offset(text: &str) -> (&str, Option<&str>) {
    match text.find(['+', '-', 'Z']) {
        Some(at) => (&text[..at], Some(&text[at..])),
        None => (text, None),
    }
}

fn date_from_text(text: &str) -> Result<String, Failure> {
    if is_unrepresentable_text(text) {
        return Err(Failure::Unrepresentable);
    }
    render_date(parse_date(text)?)
}

fn time_from_text(text: &str) -> Result<String, Failure> {
    match split_offset(text) {
        (time, None) => render_time(parse_time_of_day(time)?),
        (time, Some(offset)) => Ok(render_timetz(
            parse_time_of_day(time)?,
            parse_offset(offset)?,
        )),
    }
}

/// Local microseconds from the Postgres epoch, and the offset text if any.
fn parse_timestamp(text: &str) -> Result<(i64, Option<&str>), Failure> {
    if is_unrepresentable_text(text) {
        return Err(Failure::Unrepresentable);
    }
    let (date, time) = text.split_once(' ').ok_or(Failure::Malformed)?;
    let (time, offset) = split_offset(time);
    Ok((
        parse_date(date)? * USECS_PER_DAY + parse_time_of_day(time)?,
        offset,
    ))
}

fn render_utc(local: i64, offset: &str) -> Result<String, Failure> {
    render_timestamp(local - parse_offset(offset)? * USECS_PER_SEC, true)
}

fn timestamp_from_text(text: &str) -> Result<String, Failure> {
    match parse_timestamp(text)? {
        (local, None) => render_timestamp(local, false),
        (_, Some(_)) => Err(Failure::Malformed),
    }
}

fn timestamptz_from_text(text: &str) -> Result<String, Failure> {
    let (local, offset) = parse_timestamp(text)?;
    render_utc(local, offset.ok_or(Failure::Malformed)?)
}

/// Postgres's `EncodeInterval` output under `IntervalStyle = postgres`,
/// such as `-1 years -2 mons +3 days -04:05:06.789`, rendered in `style`.
fn interval_from_text(text: &str, style: IntervalStyle) -> Result<String, Failure> {
    if text == "infinity" || text == "-infinity" {
        return Err(Failure::Unrepresentable);
    }
    let mut months: i64 = 0;
    let mut days: i64 = 0;
    let mut time: i64 = 0;
    // Units must appear in Postgres's order, each at most once.
    let mut next_unit = 0;
    let mut tokens = text.split(' ').peekable();
    while let Some(token) = tokens.next() {
        if token.contains(':') {
            if tokens.peek().is_some() {
                return Err(Failure::Malformed);
            }
            time = parse_interval_time(token)?;
            break;
        }
        let value = parse_signed(token)?;
        let (rank, per_unit) = match tokens.next() {
            Some("year" | "years") => (1, 12),
            Some("mon" | "mons") => (2, 1),
            Some("day" | "days") => (3, 0),
            _ => return Err(Failure::Malformed),
        };
        if rank <= next_unit {
            return Err(Failure::Malformed);
        }
        next_unit = rank;
        if per_unit == 0 {
            days = value;
        } else {
            months += value * per_unit;
        }
    }
    let months = i32::try_from(months).map_err(|_| Failure::Malformed)?;
    let days = i32::try_from(days).map_err(|_| Failure::Malformed)?;
    render_interval(months, days, time, style)
}

/// Split a leading `+` or `-` from `text`.
fn split_sign(text: &str) -> (i64, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    }
}

/// An optionally signed run of at most ten digits.
fn parse_signed(text: &str) -> Result<i64, Failure> {
    let (sign, magnitude) = split_sign(text);
    if magnitude.len() > 10 {
        return Err(Failure::Malformed);
    }
    Ok(sign * digits(magnitude, 0)?)
}

/// `[+-]HH:MM:SS[.f]` with two or more hour digits, as signed
/// microseconds.
fn parse_interval_time(text: &str) -> Result<i64, Failure> {
    let (sign, magnitude) = split_sign(text);
    let (hours, rest) = magnitude.split_once(':').ok_or(Failure::Malformed)?;
    if hours.len() < 2 {
        return Err(Failure::Malformed);
    }
    digits(hours, 0)?
        .checked_mul(USECS_PER_HOUR)
        .and_then(|h| h.checked_add(parse_minutes_seconds(rest).ok()?))
        .map(|magnitude| sign * magnitude)
        .ok_or(Failure::Malformed)
}
