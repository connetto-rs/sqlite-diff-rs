//! [`ConversionError`]: shared failure mode for digesting a CDC wire
//! event (`pg_walstream`, `wal2json`, `maxwell`) into a diff builder.
//!
//! The three forward sources previously each defined a near-identical
//! copy of this enum. They now share this one type and re-export it, so
//! the shared build helpers in [`shared_builders`](super::shared_builders)
//! return a single error type across every source.

use alloc::string::String;

use super::error::DecodeError;

/// Errors raised while folding a CDC wire event into a diff builder.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConversionError {
    /// A column name from the event was not found in the table schema.
    #[error("Column '{0}' not found in table schema")]
    ColumnNotFound(String),

    /// The table name in the event does not match the expected schema.
    #[error("Table name mismatch: expected '{expected}', got '{actual}'")]
    TableMismatch {
        /// Expected table name from the schema.
        expected: String,
        /// Actual table name from the wire event.
        actual: String,
    },

    /// Table named in the wire event is not in the schema.
    #[error("Table '{0}' not found in schema")]
    TableNotFound(String),

    /// The event is missing a required set of columns (for example the
    /// full column list, or every primary-key column of a delete).
    #[error("Missing columns in event")]
    MissingColumns,

    /// The event is missing required data for an operation. The tuple is
    /// (what was missing, operation name), for example `("pk", "DELETE")`.
    #[error("Missing {0} data for {1} operation")]
    MissingData(&'static str, &'static str),

    /// Old-row data is required but not available (replica identity issue).
    #[error("Old data not available (check replica identity setting)")]
    MissingOldData,

    /// The event carries no old value for this primary key column, which a changeset UPDATE needs to name its row.
    #[error("No old value for primary key column '{0}' (check replica identity setting)")]
    MissingOldKey(String),

    /// A JSON value type is not supported for conversion.
    #[error("Unsupported value type for column '{0}'")]
    UnsupportedType(String),

    /// The event type is not applicable for the requested conversion.
    #[error("Event type '{0}' cannot be converted to the requested operation")]
    InvalidEventType(String),

    /// The operation type is not applicable for the requested conversion.
    #[error("Operation '{0}' cannot be converted to the requested type")]
    InvalidOperation(String),

    /// A user-registered decoder rejected a column payload.
    #[error("Decoder failed: {0}")]
    Decode(#[from] DecodeError),
}
