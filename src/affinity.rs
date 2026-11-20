//! SQLite column type affinity: the conversion SQLite applies to a value
//! before it stores the value in a column, and so the value a session records.

mod float;

pub(crate) use float::NumberText;

use crate::encoding::Value;

/// The type affinity SQLite derives from a column's declared type.
///
/// ```rust
/// use sqlite_diff_rs::{Affinity, Value};
///
/// let text = Affinity::from_declared_type("VARCHAR(10)");
/// assert_eq!(text, Affinity::Text);
/// let stored: Value<String, Vec<u8>> = text.apply(Value::Real(1e-5));
/// assert_eq!(stored, Value::Text("1.0e-05".to_owned()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Affinity {
    /// Numbers are stored as their text.
    Text,
    /// Values are stored unchanged. The affinity of a column without a declared type.
    Blob,
    /// Text that reads as a number is stored as an integer when that loses
    /// nothing, else as a real, and an integral real becomes an integer.
    Numeric,
    /// Behaves as [`Affinity::Numeric`] when storing a value.
    Integer,
    /// As [`Affinity::Numeric`], after which an integer is stored as a real.
    Real,
}

impl Affinity {
    /// The affinity of a column declared with `declared`, by SQLite's rules.
    ///
    /// The first match wins: `INT` anywhere gives [`Affinity::Integer`], then
    /// `CHAR`, `CLOB` or `TEXT` give [`Affinity::Text`], then `BLOB` or an
    /// empty type give [`Affinity::Blob`], then `REAL`, `FLOA` or `DOUB` give
    /// [`Affinity::Real`], and anything else gives [`Affinity::Numeric`].
    #[must_use]
    pub fn from_declared_type(declared: &str) -> Self {
        const fn key(name: [u8; 4]) -> u32 {
            u32::from_be_bytes(name)
        }
        // SQLite reads the type as a C string, which ends at the first NUL.
        let declared = declared.split('\0').next().unwrap_or_default();
        if declared.is_empty() {
            return Self::Blob;
        }
        let mut h: u32 = 0;
        let mut affinity = Self::Numeric;
        for &byte in declared.as_bytes() {
            h = (h << 8).wrapping_add(u32::from(byte.to_ascii_lowercase()));
            if h == key(*b"char") || h == key(*b"clob") || h == key(*b"text") {
                affinity = Self::Text;
            } else if h == key(*b"blob") && matches!(affinity, Self::Numeric | Self::Real) {
                affinity = Self::Blob;
            } else if (h == key(*b"real") || h == key(*b"floa") || h == key(*b"doub"))
                && affinity == Self::Numeric
            {
                affinity = Self::Real;
            } else if h & 0x00ff_ffff == key(*b"\0int") {
                return Self::Integer;
            }
        }
        affinity
    }

    /// The value SQLite stores when `value` is written to a column of this affinity.
    ///
    /// Numbers render as text the way the bundled SQLite does, text that
    /// reads as a number converts as SQLite parses it, and NaN becomes `NULL`
    /// because SQLite never stores it. Applying twice gives the same value as
    /// applying once.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "SQLite stores an integer in a REAL column as the nearest real"
    )]
    pub fn apply<S, B>(self, value: Value<S, B>) -> Value<S, B>
    where
        S: AsRef<str> + for<'a> From<&'a str>,
    {
        match value {
            Value::Real(r) if r.is_nan() => Value::Null,
            Value::Integer(i) => match self {
                Self::Text => Value::Text(S::from(float::integer_text(i).as_str())),
                Self::Real => Value::Real(i as f64),
                Self::Blob | Self::Numeric | Self::Integer => Value::Integer(i),
            },
            Value::Real(r) => match self {
                Self::Text => Value::Text(S::from(float::real_text(r).as_str())),
                Self::Blob => Value::Real(r),
                Self::Real => Value::Real(float::integral(r).map_or(r, |i| i as f64)),
                Self::Numeric | Self::Integer => {
                    float::integral(r).map_or(Value::Real(r), Value::Integer)
                }
            },
            Value::Text(text) => match self {
                Self::Text | Self::Blob => Value::Text(text),
                Self::Numeric | Self::Integer | Self::Real => match numeric(text.as_ref()) {
                    None => Value::Text(text),
                    Some(Number::Integer(i)) if self == Self::Real => Value::Real(i as f64),
                    Some(Number::Integer(i)) => Value::Integer(i),
                    Some(Number::Real(r)) => Value::Real(r),
                },
            },
            other => other,
        }
    }
}

enum Number {
    Integer(i64),
    Real(f64),
}

/// `applyNumericAffinity(pRec, 1)`: the number a text denotes, if all of it is one.
fn numeric(text: &str) -> Option<Number> {
    let bytes = text.as_bytes();
    let (flags, r) = float::ato_f(bytes);
    if flags <= 0 {
        return None;
    }
    if flags & 2 == 0
        && let Some(i) = float::also_an_int(bytes, r)
    {
        return Some(Number::Integer(i));
    }
    Some(float::integral(r).map_or(Number::Real(r), Number::Integer))
}

/// The real SQLite reads from the text of a numeric literal, or `None` when
/// the text is not entirely a number.
pub(crate) fn literal_real(text: &str) -> Option<f64> {
    let (flags, r) = float::ato_f(text.as_bytes());
    (flags > 0).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::Affinity;
    use crate::encoding::Value;
    use alloc::borrow::ToOwned;
    use alloc::string::String;
    use alloc::vec::Vec;

    type Val = Value<String, Vec<u8>>;

    #[test]
    fn declared_types() {
        let cases = [
            ("", Affinity::Blob),
            ("INTEGER", Affinity::Integer),
            ("int", Affinity::Integer),
            ("BIGINT", Affinity::Integer),
            ("FLOATING POINT", Affinity::Integer),
            ("CHARINT", Affinity::Integer),
            ("VARCHAR(10)", Affinity::Text),
            ("NATIVE CHARACTER(70)", Affinity::Text),
            ("CLOB", Affinity::Text),
            ("text", Affinity::Text),
            ("TEXTBLOB", Affinity::Text),
            ("BLOB", Affinity::Blob),
            ("REAL BLOB", Affinity::Blob),
            ("REAL", Affinity::Real),
            ("DOUBLE PRECISION", Affinity::Real),
            ("FLOAT", Affinity::Real),
            ("NUMERIC", Affinity::Numeric),
            ("DECIMAL(10,2)", Affinity::Numeric),
            ("BOOLEAN", Affinity::Numeric),
            ("DATETIME", Affinity::Numeric),
            ("STRING", Affinity::Numeric),
            ("X\0INT", Affinity::Numeric),
            ("CLOB\0INT", Affinity::Text),
        ];
        for (declared, expected) in cases {
            assert_eq!(
                Affinity::from_declared_type(declared),
                expected,
                "{declared:?}"
            );
        }
    }

    fn text(s: &str) -> Val {
        Value::Text(s.to_owned())
    }

    #[test]
    fn stored_values() {
        use Affinity::{Blob, Integer, Numeric, Real, Text};
        let cases: [(Val, Affinity, Val); 20] = [
            (Value::Integer(5), Text, text("5")),
            (Value::Integer(5), Real, Value::Real(5.0)),
            (Value::Real(5.0), Integer, Value::Integer(5)),
            (Value::Real(5.0), Text, text("5.0")),
            (Value::Real(5.0), Blob, Value::Real(5.0)),
            (Value::Real(1e5), Text, text("100000.0")),
            (Value::Real(1e-5), Text, text("1.0e-05")),
            (Value::Real(1e16), Text, text("10000000000000000.0")),
            (Value::Real(1e17), Text, text("1.0e+17")),
            (
                Value::Real(9_223_372_036_854_775_808.0),
                Text,
                text("9.2233720368547758e+18"),
            ),
            (
                Value::Real(9_223_372_036_854_775_808.0),
                Integer,
                Value::Real(9_223_372_036_854_775_808.0),
            ),
            (Value::Real(-0.0), Numeric, Value::Integer(0)),
            (Value::Real(-0.0), Text, text("0.0")),
            (Value::Real(f64::NEG_INFINITY), Text, text("-Inf")),
            (text(" 5 "), Integer, Value::Integer(5)),
            (text("3.0e+5"), Numeric, Value::Integer(300_000)),
            (text(".5"), Real, Value::Real(0.5)),
            (text("0x10"), Integer, text("0x10")),
            (text("5"), Blob, text("5")),
            (Value::Real(f64::NAN), Blob, Value::Null),
        ];
        for (value, affinity, expected) in cases {
            assert_eq!(
                affinity.apply(value.clone()),
                expected,
                "{value:?} {affinity:?}"
            );
        }
    }
}
