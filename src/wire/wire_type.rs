//! [`WireType`]: source-independent semantic column type, and [`WireKind`],
//! the decoder-registration key it reduces to.

/// Semantic column type used to select a decoder, independent of any
/// wire source's native type identity.
///
/// The schema side declares one of these per column (via
/// [`WireColumnTypes`](super::WireColumnTypes)) and every payload carries
/// it. [`TypeMap`](super::TypeMap) dispatches on its [`WireKind`], so a
/// variant's parameters, such as [`ScaledDecimal`](Self::ScaledDecimal)'s
/// scale, reach the decoder through the payload while one registration
/// serves every value of them.
///
/// The set covers every decoder the crate ships. `Json` and `Jsonb`
/// stay distinct so a downstream that treats them differently can
/// register different decoders. Decoder policy variants (for example
/// [`UuidText36Decoder`](super::UuidText36Decoder) versus
/// [`UuidBlob16Decoder`](super::UuidBlob16Decoder), or
/// [`JsonVerbatimDecoder`](super::JsonVerbatimDecoder) versus
/// [`JsonCanonicalDecoder`](super::JsonCanonicalDecoder)) are chosen at
/// registration under the one semantic key, not by adding more keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WireType {
    /// Boolean.
    Bool,
    /// Signed integer fitting in `i64`.
    Int,
    /// Floating-point.
    Real,
    /// UTF-8 text.
    Text,
    /// Raw bytes.
    Bytes,
    /// UUID.
    Uuid,
    /// Arbitrary-precision decimal / numeric, held as its decimal text.
    Decimal,
    /// Decimal held as an integer count of minor units, `value × 10^scale`,
    /// the way a `NUMERIC(p,s)` replica column stores `12.50` at scale 2
    /// as `1250`.
    ScaledDecimal {
        /// Power of ten the stored integer is scaled by.
        scale: u32,
    },
    /// Timestamp without time zone.
    Timestamp,
    /// Timestamp with time zone.
    TimestampTz,
    /// Calendar date.
    Date,
    /// Time of day.
    Time,
    /// Interval / duration.
    Interval,
    /// JSON.
    Json,
    /// Binary JSON (`jsonb`).
    Jsonb,
}

impl WireType {
    /// The [`WireKind`] this type registers and dispatches under.
    #[must_use]
    pub const fn kind(self) -> WireKind {
        match self {
            Self::Bool => WireKind::Bool,
            Self::Int => WireKind::Int,
            Self::Real => WireKind::Real,
            Self::Text => WireKind::Text,
            Self::Bytes => WireKind::Bytes,
            Self::Uuid => WireKind::Uuid,
            Self::Decimal => WireKind::Decimal,
            Self::ScaledDecimal { .. } => WireKind::ScaledDecimal,
            Self::Timestamp => WireKind::Timestamp,
            Self::TimestampTz => WireKind::TimestampTz,
            Self::Date => WireKind::Date,
            Self::Time => WireKind::Time,
            Self::Interval => WireKind::Interval,
            Self::Json => WireKind::Json,
            Self::Jsonb => WireKind::Jsonb,
        }
    }
}

/// A [`WireType`] without its parameters, the key
/// [`TypeMap`](super::TypeMap) registers decoders under.
///
/// Every [`WireType::ScaledDecimal`] maps to [`WireKind::ScaledDecimal`]
/// whatever its scale, and the decoder reads the scale from the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WireKind {
    /// [`WireType::Bool`].
    Bool,
    /// [`WireType::Int`].
    Int,
    /// [`WireType::Real`].
    Real,
    /// [`WireType::Text`].
    Text,
    /// [`WireType::Bytes`].
    Bytes,
    /// [`WireType::Uuid`].
    Uuid,
    /// [`WireType::Decimal`].
    Decimal,
    /// [`WireType::ScaledDecimal`] at any scale.
    ScaledDecimal,
    /// [`WireType::Timestamp`].
    Timestamp,
    /// [`WireType::TimestampTz`].
    TimestampTz,
    /// [`WireType::Date`].
    Date,
    /// [`WireType::Time`].
    Time,
    /// [`WireType::Interval`].
    Interval,
    /// [`WireType::Json`].
    Json,
    /// [`WireType::Jsonb`].
    Jsonb,
}
