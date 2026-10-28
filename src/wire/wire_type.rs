//! [`WireType`]: source-independent semantic column type, and [`WireKind`],
//! the decoder-registration key it reduces to.

/// Semantic column type used to select a decoder, independent of any
/// wire source's native type identity.
///
/// The schema side declares one of these per column (via
/// [`WireColumnTypes`](super::WireColumnTypes)) and every payload carries
/// it. [`TypeMap`](super::TypeMap) dispatches on its [`WireKind`], so a
/// variant's parameters, such as [`ScaledDecimal`](Self::ScaledDecimal)'s
/// scale, [`Array`](Self::Array)'s element type or [`Vector`](Self::Vector)'s
/// dimension, reach the decoder through the payload while one registration
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
    /// PostgreSQL array of one or more dimensions, held as JSON array text
    /// such as `[1,2]` for `{1,2}`, every element stored the way a scalar
    /// column of its type is.
    Array {
        /// Type of every element.
        element: ArrayElement,
    },
    /// Vector of single-precision floats, such as pgvector's `vector(n)`,
    /// held in sqlite-vec's float32 layout, a `BLOB` of the elements as
    /// little-endian IEEE 754 `f32` with no header.
    Vector {
        /// Number of elements every value has, or `None` when the column
        /// declares no dimension and any length is accepted.
        dimensions: Option<u32>,
    },
}

/// Element type of a [`WireType::Array`], one of the [`WireType`]s
/// without parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrayElement {
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

impl ArrayElement {
    /// The [`WireType`] each element decodes as.
    #[must_use]
    pub const fn wire_type(self) -> WireType {
        match self {
            Self::Bool => WireType::Bool,
            Self::Int => WireType::Int,
            Self::Real => WireType::Real,
            Self::Text => WireType::Text,
            Self::Bytes => WireType::Bytes,
            Self::Uuid => WireType::Uuid,
            Self::Decimal => WireType::Decimal,
            Self::Timestamp => WireType::Timestamp,
            Self::TimestampTz => WireType::TimestampTz,
            Self::Date => WireType::Date,
            Self::Time => WireType::Time,
            Self::Interval => WireType::Interval,
            Self::Json => WireType::Json,
            Self::Jsonb => WireType::Jsonb,
        }
    }
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
            Self::Array { .. } => WireKind::Array,
            Self::Vector { .. } => WireKind::Vector,
        }
    }
}

/// A [`WireType`] without its parameters, the key
/// [`TypeMap`](super::TypeMap) registers decoders under.
///
/// Every [`WireType::ScaledDecimal`] maps to [`WireKind::ScaledDecimal`]
/// whatever its scale, every [`WireType::Array`] to [`WireKind::Array`]
/// whatever its element type, and every [`WireType::Vector`] to
/// [`WireKind::Vector`] whatever its dimension. The decoder reads the
/// parameter from the payload.
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
    /// [`WireType::Array`] of any element type.
    Array,
    /// [`WireType::Vector`] of any dimension.
    Vector,
}
