//! Schema-less operation enum for internal use in `DiffSetBuilder`.
//!
//! Operations store only row data (values), not the table schema `T`.
//! The schema lives as the key of the outer `IndexMap` in `DiffSetBuilder`.
//! All consolidation logic (Operation + Operation) is defined here.

use alloc::vec::Vec;
use core::fmt::Debug;

use crate::{
    builders::{ChangesetFormat, PatchsetFormat, format::Format},
    encoding::{MaybeValue, Value},
    schema::SchemaWithPK,
};

/// A schema-less database operation, parameterized by format `F` and value
/// types `S`, `B`.
///
/// Each variant carries the SQLite session-extension `indirect` flag, which
/// distinguishes direct application writes (`false`) from trigger-induced
/// or cascading changes (`true`). The table schema `T` is NOT stored here;
/// it lives as the key in `DiffSetBuilder`'s
/// `IndexMap<T, IndexMap<Vec<Value<S, B>>, Operation<F, S, B>>>`.
#[derive(Debug, Clone)]
pub(crate) enum Operation<F: Format<S, B>, S, B> {
    /// A row was inserted. Stores all column values.
    Insert {
        /// Full row values, one per column.
        values: Vec<Value<S, B>>,
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// A row was deleted. Stores format-specific delete data:
    /// - Changeset: `Vec<Value<S, B>>` (full old-row values)
    /// - Patchset: `()` (PK is stored as the `IndexMap` key)
    Delete {
        /// Format-specific delete payload.
        data: F::DeleteData,
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// A row was updated. Stores `(old, new)` pairs per column.
    /// - Changeset: `(MaybeValue<S, B>, MaybeValue<S, B>)` per column (None = undefined)
    /// - Patchset: `((), MaybeValue<S, B>)` per column
    Update {
        /// `(old, new)` pairs, one per column.
        values: Vec<(F::Old, MaybeValue<S, B>)>,
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
}

impl<F: Format<S, B>, S, B> Operation<F, S, B> {
    /// Returns the indirect flag of this operation.
    #[inline]
    pub(crate) fn indirect(&self) -> bool {
        match self {
            Self::Insert { indirect, .. }
            | Self::Delete { indirect, .. }
            | Self::Update { indirect, .. } => *indirect,
        }
    }
}

/// The `(old, new)` pairs of an UPDATE in format `F`.
type UpdatePairs<F, S, B> = Vec<(<F as Format<S, B>>::Old, MaybeValue<S, B>)>;

/// The row of an INSERT with every defined new value of a following UPDATE applied.
fn apply_update<F: Format<S, B>, S, B>(
    mut values: Vec<Value<S, B>>,
    updates: UpdatePairs<F, S, B>,
) -> Vec<Value<S, B>> {
    for (idx, (_old, new)) in updates.into_iter().enumerate() {
        if let Some(new_val) = new {
            values[idx] = new_val;
        }
    }
    values
}

/// The `(old, new)` pairs of a changeset UPDATE.
type ChangesetPairs<S, B> = Vec<(MaybeValue<S, B>, MaybeValue<S, B>)>;

/// The pairs of a changeset UPDATE as `sessionMergeUpdate` writes them, or `None` when the
/// UPDATE changes nothing.
///
/// A non-key column whose old and new values are equal is undefined on both sides. A key column
/// keeps its old value as the row identity, and its new value only when the key moved, which
/// SQLite never records but the CDC digests do.
fn settle_update<S, B, T: SchemaWithPK>(
    pairs: impl Iterator<Item = (MaybeValue<S, B>, MaybeValue<S, B>)>,
    table: &T,
) -> Option<ChangesetPairs<S, B>>
where
    S: AsRef<str> + PartialEq,
    B: AsRef<[u8]> + PartialEq,
{
    let mut changed = false;
    let settled = pairs
        .enumerate()
        .map(|(col_idx, (old, new))| {
            if table.primary_key_index(col_idx).is_some() {
                let new = new.filter(|new| old.as_ref() != Some(new));
                changed |= new.is_some();
                (old, new)
            } else if old == new {
                (None, None)
            } else {
                changed = true;
                (old, new)
            }
        })
        .collect();
    changed.then_some(settled)
}

impl<F: Format<S, B>, S: PartialEq + AsRef<str>, B: PartialEq + AsRef<[u8]>> PartialEq
    for Operation<F, S, B>
where
    F::DeleteData: PartialEq,
    F::Old: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        if self.indirect() != other.indirect() {
            return false;
        }
        match (self, other) {
            (Self::Insert { values: a, .. }, Self::Insert { values: b, .. }) => a == b,
            (Self::Delete { data: a, .. }, Self::Delete { data: b, .. }) => a == b,
            (Self::Update { values: a, .. }, Self::Update { values: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl<F: Format<S, B>, S: Eq + AsRef<str>, B: Eq + AsRef<[u8]>> Eq for Operation<F, S, B>
where
    F::DeleteData: Eq,
    F::Old: Eq,
{
}

/// Builders that carry the SQLite session-extension indirect-change flag.
///
/// Implemented for [`Insert`](crate::Insert), [`Update`](crate::Update),
/// [`ChangeDelete`](crate::ChangeDelete), and [`PatchDelete`](crate::PatchDelete).
///
/// The flag distinguishes direct application writes (`false`) from changes
/// produced by triggers or foreign-key cascades (`true`). SQLite's session
/// extension writes it as a single byte after each operation's op-code so
/// consumers can filter on it when applying changesets. Defaults to `false`
/// on construction.
///
/// See the [SQLite session-extension docs](https://www.sqlite.org/session/sqlite3session_indirect.html).
pub trait Indirect: Sized {
    /// Mark this operation as indirect (trigger-induced or cascading).
    #[must_use]
    fn indirect(self, indirect: bool) -> Self;
}

/// Trait for reversing operations.
///
/// Reversing a database operation is useful for creating inverse changesets
/// (undo), resolving conflicts in distributed systems, and testing
/// bidirectional synchronization.
pub trait Reverse {
    /// The reverse of this operation.
    type Output;

    /// Returns the reverse of this operation.
    fn reverse(self) -> Self::Output;
}

impl<S: Clone + Debug + PartialEq + AsRef<str>, B: Clone + Debug + PartialEq + AsRef<[u8]>>
    Operation<ChangesetFormat, S, B>
{
    /// The inverse of this operation, as `sqlite3changeset_invert` writes it.
    ///
    /// An UPDATE swaps its old and new values, except on a key column whose new value is
    /// undefined, which keeps the key in the old record.
    pub(crate) fn invert<T: SchemaWithPK>(self, table: &T) -> Self {
        match self {
            // INSERT reversed becomes DELETE (same values)
            Self::Insert { values, indirect } => Self::Delete {
                data: values,
                indirect,
            },
            // DELETE reversed becomes INSERT (same values)
            Self::Delete { data, indirect } => Self::Insert {
                values: data,
                indirect,
            },
            // UPDATE reversed becomes UPDATE with old/new swapped, the key staying old
            Self::Update { values, indirect } => Self::Update {
                values: values
                    .into_iter()
                    .enumerate()
                    .map(|(col_idx, (old, new))| {
                        if new.is_none() && table.primary_key_index(col_idx).is_some() {
                            (old, new)
                        } else {
                            (new, old)
                        }
                    })
                    .collect(),
                indirect,
            },
        }
    }

    /// This operation followed by `rhs` on the same row of `table`, as
    /// `sqlite3changeset_concat` merges them, or `None` when they cancel out.
    pub(crate) fn merge<T: SchemaWithPK>(self, rhs: Self, table: &T) -> Option<Self> {
        // "Last-write-wins": the merged op carries the rhs operand's
        // indirect flag.
        let indirect = rhs.indirect();
        match (self, rhs) {
            // INSERT + INSERT: keep first values
            (Self::Insert { values: lhs, .. }, Self::Insert { .. }) => Some(Self::Insert {
                values: lhs,
                indirect,
            }),

            // INSERT + UPDATE: apply update to insert values
            (
                Self::Insert { values, .. },
                Self::Update {
                    values: updates, ..
                },
            ) => Some(Self::Insert {
                values: apply_update::<ChangesetFormat, S, B>(values, updates),
                indirect,
            }),

            // INSERT + DELETE: cancel out
            (Self::Insert { .. }, Self::Delete { .. }) => None,

            // UPDATE + INSERT: keep update
            (Self::Update { values: lhs, .. }, Self::Insert { .. }) => Some(Self::Update {
                values: lhs,
                indirect,
            }),

            // UPDATE + UPDATE: first defined old, last defined new, dropped if nothing changes
            (Self::Update { values: lhs, .. }, Self::Update { values: rhs, .. }) => {
                let merged = lhs
                    .into_iter()
                    .zip(rhs)
                    .map(|((old1, new1), (old2, new2))| (old1.or(old2), new2.or(new1)));
                settle_update(merged, table).map(|values| Self::Update { values, indirect })
            }

            // UPDATE + DELETE: delete with the update's old values, the delete's where undefined
            (Self::Update { values: upd, .. }, Self::Delete { data: del, .. }) => {
                Some(Self::Delete {
                    data: upd
                        .into_iter()
                        .zip(del)
                        .map(|((old, _new), deleted)| old.unwrap_or(deleted))
                        .collect(),
                    indirect,
                })
            }

            // DELETE + INSERT: update of the differing columns, cancel if same
            (Self::Delete { data: del, .. }, Self::Insert { values: ins, .. }) => {
                let pairs = del
                    .into_iter()
                    .zip(ins)
                    .map(|(old, new)| (Some(old), Some(new)));
                settle_update(pairs, table).map(|values| Self::Update { values, indirect })
            }

            // DELETE + UPDATE or DELETE + DELETE: keep the delete with rhs's indirect
            (Self::Delete { data: lhs, .. }, Self::Update { .. } | Self::Delete { .. }) => {
                Some(Self::Delete {
                    data: lhs,
                    indirect,
                })
            }
        }
    }
}

impl<S, B> Operation<PatchsetFormat, S, B> {
    /// This operation followed by `rhs` on the same row, as `sqlite3changeset_concat`
    /// merges them, or `None` when they cancel out.
    pub(crate) fn merge(self, rhs: Self) -> Option<Self> {
        let indirect = rhs.indirect();
        match (self, rhs) {
            // INSERT + INSERT: keep first
            (Self::Insert { values: lhs, .. }, Self::Insert { .. }) => Some(Self::Insert {
                values: lhs,
                indirect,
            }),

            // INSERT + UPDATE: apply update to insert values
            (
                Self::Insert { values, .. },
                Self::Update {
                    values: updates, ..
                },
            ) => Some(Self::Insert {
                values: apply_update::<PatchsetFormat, S, B>(values, updates),
                indirect,
            }),

            // INSERT + DELETE: cancel out
            (Self::Insert { .. }, Self::Delete { data: (), .. }) => None,

            // UPDATE + INSERT: keep update
            (Self::Update { values: lhs, .. }, Self::Insert { .. }) => Some(Self::Update {
                values: lhs,
                indirect,
            }),

            // UPDATE + UPDATE: last defined new value of each column
            (Self::Update { values: lhs, .. }, Self::Update { values: rhs, .. }) => {
                Some(Self::Update {
                    values: lhs
                        .into_iter()
                        .zip(rhs)
                        .map(|(((), new1), ((), new2))| ((), new2.or(new1)))
                        .collect(),
                    indirect,
                })
            }

            // UPDATE + DELETE: keep delete (patchset doesn't need old values)
            (Self::Update { .. }, Self::Delete { data, .. }) => {
                Some(Self::Delete { data, indirect })
            }

            // DELETE + INSERT: always becomes update (patchset can't compare old values)
            (Self::Delete { data: (), .. }, Self::Insert { values: ins, .. }) => {
                let update_values = ins.into_iter().map(|new| ((), Some(new))).collect();
                Some(Self::Update {
                    values: update_values,
                    indirect,
                })
            }

            // DELETE + UPDATE or DELETE + DELETE: keep the delete with rhs's indirect
            (Self::Delete { data: lhs, .. }, Self::Update { .. } | Self::Delete { .. }) => {
                Some(Self::Delete {
                    data: lhs,
                    indirect,
                })
            }
        }
    }
}
