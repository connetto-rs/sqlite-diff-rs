#![doc = include_str!("../README.md")]
#![no_std]
#![deny(clippy::mod_module_files)]
#![allow(private_bounds, private_interfaces)]

extern crate alloc;

mod affinity;
pub mod builders;
#[cfg(any(test, feature = "testing"))]
pub mod differential_testing;
pub(crate) mod encoding;
pub mod errors;
#[cfg(feature = "maxwell")]
pub mod maxwell;
pub mod parser;
#[cfg(feature = "pg-walstream")]
pub mod pg_walstream;
#[cfg(feature = "pg-walstream")]
pub mod pg_walstream_reverse;
pub mod schema;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(feature = "wal2json")]
pub mod wal2json;
pub mod wire;

// Re-export main types
pub use affinity::Affinity;
#[cfg(feature = "diesel-async")]
pub use builders::ApplyOpsAsync;
#[cfg(feature = "diesel")]
pub use builders::{
    Adapter, ApplyOps, Binder, BoundChangesetOp, BoundOp, BoundPatchsetOp, DefaultBinder,
};
pub use builders::{
    ChangeDelete, ChangeSet, ChangesetFormat, ChangesetOp, ChangesetUpdatePair,
    ChangesetUpdatePairExt, ColumnNames, DiffFormat, DiffOps, DiffSet, DiffSetBuilder, Indirect,
    Insert, PatchDelete, PatchSet, PatchsetFormat, PatchsetOp, PatchsetUpdateEntry, Reverse,
    Update,
};
pub use encoding::Value;
pub use parser::{FormatMarker, ParseError, ParsedDiffSet, TableSchema};
pub use schema::{DynTable, IndexableValues, NamedColumns, SchemaWithPK, SimpleTable};
pub use wire::{
    ArrayElement, ArrayJsonDecoder, BoolDecoder, DateDecoder, DecimalMinorUnitsDecoder,
    DecimalTextDecoder, DecodeError, Decoder, Digestable, Int64OverflowToTextDecoder, IntDecoder,
    IntervalIso8601Decoder, IntervalPostgresDecoder, JsonCanonicalDecoder, JsonVerbatimDecoder,
    MySqlBinaryDecoder, NullDecoder, PgBinary, PgBinaryColumn, PgByteaBinaryDecoder,
    PgByteaTextModeDecoder, RealDecoder, TextDecoder, TimeDecoder, TimestampDecoder,
    TimestampTzDecoder, TypeMap, TypeMapDefaults, UuidBlob16Decoder, UuidText36Decoder,
    VectorFloat32Decoder, WireAdapter, WireColumnTypes, WireKind, WireSchema, WireSource, WireType,
};

// Type aliases for common use cases
/// Type alias for `Update<T, ChangesetFormat, S, B>`.
///
/// Changeset updates store both old and new values for each column.
pub type ChangeUpdate<T, S, B> = Update<T, ChangesetFormat, S, B>;

/// Type alias for `Update<T, PatchsetFormat, S, B>`.
///
/// Patchset updates only store new values (PK values in new, non-PK as Undefined or new value).
pub type PatchUpdate<T, S, B> = Update<T, PatchsetFormat, S, B>;

// Re-export errors
pub use errors::Error;
