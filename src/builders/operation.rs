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

/// Two consecutive UPDATEs as one, keeping the first's old values and the second's new ones.
fn merge_updates<F: Format<S, B>, S, B>(
    lhs: UpdatePairs<F, S, B>,
    rhs: UpdatePairs<F, S, B>,
) -> UpdatePairs<F, S, B> {
    lhs.into_iter()
        .zip(rhs)
        .map(|((old, _mid), (_mid2, new))| (old, new))
        .collect()
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

impl<S: Clone + Debug + AsRef<str>, B: Clone + Debug + AsRef<[u8]>> Reverse
    for Operation<ChangesetFormat, S, B>
{
    type Output = Self;

    fn reverse(self) -> Self::Output {
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
            // UPDATE reversed becomes UPDATE with old/new swapped
            Self::Update { values, indirect } => Self::Update {
                values: values.into_iter().map(|(old, new)| (new, old)).collect(),
                indirect,
            },
        }
    }
}

// ============================================================================
// Operation + Operation for Changeset
// ============================================================================

impl<S: Clone + Debug + PartialEq + AsRef<str>, B: Clone + Debug + PartialEq + AsRef<[u8]>>
    core::ops::Add for Operation<ChangesetFormat, S, B>
{
    type Output = Option<Self>;

    fn add(self, rhs: Self) -> Self::Output {
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

            // UPDATE + UPDATE: keep original old, use final new
            (Self::Update { values: lhs, .. }, Self::Update { values: rhs, .. }) => {
                Some(Self::Update {
                    values: merge_updates::<ChangesetFormat, S, B>(lhs, rhs),
                    indirect,
                })
            }

            // UPDATE + DELETE: delete with original old values
            (Self::Update { values: upd, .. }, Self::Delete { .. }) => {
                // Collect old values, converting MaybeValue to Value (None becomes Null)
                let old_values = upd
                    .into_iter()
                    .map(|(old, _new)| old.unwrap_or(Value::Null))
                    .collect();
                Some(Self::Delete {
                    data: old_values,
                    indirect,
                })
            }

            // DELETE + INSERT: update if different, cancel if same
            (Self::Delete { data: del, .. }, Self::Insert { values: ins, .. }) => {
                if del == ins {
                    None // Same values, cancel out
                } else {
                    let update_values = del
                        .into_iter()
                        .zip(ins)
                        .map(|(old, new)| (Some(old), Some(new)))
                        .collect();
                    Some(Self::Update {
                        values: update_values,
                        indirect,
                    })
                }
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

// ============================================================================
// Operation + Operation for Patchset
// ============================================================================

impl<S: Clone + PartialEq + AsRef<str>, B: Clone + PartialEq + AsRef<[u8]>> core::ops::Add
    for Operation<PatchsetFormat, S, B>
{
    type Output = Option<Self>;

    fn add(self, rhs: Self) -> Self::Output {
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

            // UPDATE + UPDATE: keep original old (unit), use final new
            (Self::Update { values: lhs, .. }, Self::Update { values: rhs, .. }) => {
                Some(Self::Update {
                    values: merge_updates::<PatchsetFormat, S, B>(lhs, rhs),
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
