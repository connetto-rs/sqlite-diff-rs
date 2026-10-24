//! Builder for constructing changesets and patchsets.

mod change;
mod delete_operation;
#[cfg(feature = "diesel-async")]
mod diesel_async_query;
#[cfg(feature = "diesel")]
mod diesel_query;
mod format;
mod insert_operation;
mod operation;
pub mod sql;
mod sql_output;
mod update_operation;
mod view;

#[cfg(any(feature = "wal2json", feature = "maxwell", feature = "pg-walstream"))]
pub(crate) use change::DeleteArg;
pub use change::{ChangeSet, DiffOps, DiffSet, DiffSetBuilder, PatchSet};
pub use delete_operation::{ChangeDelete, PatchDelete};
#[cfg(feature = "diesel-async")]
pub use diesel_async_query::ApplyOpsAsync;
#[cfg(feature = "diesel")]
pub use diesel_query::{
    Adapter, ApplyOps, Binder, BoundChangesetOp, BoundOp, BoundPatchsetOp, DefaultBinder,
};
pub(crate) use format::Format;
pub use format::{ChangesetFormat, DiffFormat, PatchsetFormat};
pub use insert_operation::Insert;
pub(crate) use operation::Operation;
pub use operation::{Indirect, Reverse};
pub use sql_output::ColumnNames;
pub use update_operation::Update;
pub use view::{
    ChangesetOp, ChangesetUpdatePair, ChangesetUpdatePairExt, PatchsetOp, PatchsetUpdateEntry,
};
