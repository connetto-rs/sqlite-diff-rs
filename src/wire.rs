//! Schema-aware forward conversion from CDC wire formats into
//! [`Value`](crate::encoding::Value).
//!
//! Every supported wire source (`pg_walstream`, `wal2json`, `maxwell`)
//! carries per-column type metadata alongside the raw value bytes. This
//! module funnels the three sources through one shared decoding contract
//! so users can register a single type-to-decoder mapping and consume
//! multiple wire formats interchangeably.
//!
//! # Shape
//!
//! - [`WireSource`] (sealed): per-source marker with an associated
//!   payload struct. Each payload carries a semantic [`WireType`].
//!   Implemented by `PgWalstream`, `Wal2Json`, `Maxwell`.
//! - [`Decoder`]: one implementation per (source, semantic) pair.
//!   Zero-sized unit types for stateless decoders (`BoolDecoder`,
//!   `IntDecoder`, ...), state-carrying structs for user config.
//! - [`WireAdapter`]: single-method dispatcher fed a per-column
//!   payload, returns a [`Value`](crate::encoding::Value).
//! - [`TypeMap`]: generic hashmap-backed [`WireAdapter`] implementation
//!   keyed by [`WireKind`], the [`WireType`] without its parameters. The
//!   primary user-facing type.
//! - [`TypeMapDefaults`]: per-source `defaults()` builder for a
//!   [`TypeMap`] pre-populated with the crate's self-evident mappings.
mod adapter;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
mod bytes_helpers;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
mod conversion_error;
mod decoder;
mod error;
#[cfg(any(feature = "maxwell", feature = "wal2json"))]
mod json_decoders;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
mod json_helpers;
mod minor_units;
mod scalar_helpers;
mod sealed;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
mod shared_builders;
mod source;
mod temporal;
mod type_map;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
mod uuid_helpers;
mod wire_type;

#[cfg(feature = "maxwell")]
mod impls_maxwell;
mod impls_pg_binary;
#[cfg(feature = "pg-walstream")]
mod impls_pg_walstream;
#[cfg(feature = "wal2json")]
mod impls_wal2json;

pub use adapter::WireAdapter;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
pub use conversion_error::ConversionError;
pub use decoder::Decoder;
pub use decoder::{
    BoolDecoder, DateDecoder, DecimalMinorUnitsDecoder, DecimalTextDecoder,
    Int64OverflowToTextDecoder, IntDecoder, IntervalIso8601Decoder, IntervalPostgresDecoder,
    JsonCanonicalDecoder, JsonVerbatimDecoder, MySqlBinaryDecoder, NullDecoder,
    PgByteaBinaryDecoder, PgByteaTextModeDecoder, RealDecoder, TextDecoder, TimeDecoder,
    TimestampDecoder, TimestampTzDecoder, UuidBlob16Decoder, UuidText36Decoder,
};
pub use error::DecodeError;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
pub(crate) use sealed::Sealed;
#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
pub(crate) use shared_builders::{
    WireColumnItem, build_changeset_delete, build_insert, build_patch_delete,
    build_patchset_update, resolve_table,
};
pub use source::{Digestable, PgBinary, PgBinaryColumn, WireColumnTypes, WireSchema, WireSource};
pub use type_map::{TypeMap, TypeMapDefaults};
pub use wire_type::{WireKind, WireType};
