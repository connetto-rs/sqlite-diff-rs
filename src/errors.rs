//! Submodule defining the errors used across the crate.

/// Errors that can occur during diffing and patching operations.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The provided index is out of bounds for the number of columns in the table.
    #[error("Column index {0} out of bounds for table with {1} columns")]
    ColumnIndexOutOfBounds(usize, usize),

    /// A changeset UPDATE was given a new value for a primary key column without its old
    /// value, which the UPDATE needs to name the row it changes.
    #[error("Primary key column {0} needs its old value in a changeset UPDATE")]
    UndefinedOldKey(usize),
}
