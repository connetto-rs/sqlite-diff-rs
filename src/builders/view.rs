//! Read-only views of operations inside a [`DiffSet`].
//!
//! [`DiffSet::iter`] (changeset format) and [`DiffSet::iter`] (patchset
//! format) yield one of these per stored operation. The fields borrow
//! from the underlying [`DiffSet`], so consumers can render or inspect
//! the contents without copying any row data.
//!
//! Patchsets do not carry full old-row values, so [`PatchsetOp::Delete`]
//! and [`PatchsetOp::Update`] expose only the primary-key columns. The
//! [`PatchsetOp::Update`] `entries` slice is the raw `(F::Old, new)`
//! storage (with `F::Old = ()` for patchsets); call `.iter().map(|(_, v)| v)`
//! to drop the unit and walk just the new values.
//!
//! [`DiffSet`]: super::DiffSet
//! [`DiffSet::iter`]: super::DiffSet::iter

use alloc::vec::Vec;
use core::fmt::Debug;

use super::format::{ChangesetFormat, PatchsetFormat};
use super::operation::Operation;
use crate::encoding::Value;
use crate::schema::SchemaWithPK;

/// `(old, new)` values for one changeset UPDATE column, where `None` is the
/// undefined wire marker.
///
/// A changeset UPDATE carries the old image of the key columns and of the
/// changed columns, and the new image of the changed columns only, so the four
/// pair shapes are distinct. [`ChangesetUpdatePairExt::is_changed`] reads them.
pub type ChangesetUpdatePair<S, B> = (Option<Value<S, B>>, Option<Value<S, B>>);

/// Reads changeset UPDATE pair wire semantics.
pub trait ChangesetUpdatePairExt {
    /// Whether the wire says this column's value changed.
    ///
    /// Undefined on both sides is a column outside the diff. Old-only is the
    /// row identity, which SQLite writes for every key column of every UPDATE,
    /// so it did not change. Both sides present changed exactly when the values
    /// differ, which is how an unchanged key column encoded by this crate
    /// reads. New without old carries no old image to compare, so it counts as
    /// changed.
    #[must_use]
    fn is_changed(&self) -> bool;
}

impl<S, B> ChangesetUpdatePairExt for ChangesetUpdatePair<S, B>
where
    Value<S, B>: PartialEq,
{
    #[inline]
    fn is_changed(&self) -> bool {
        match self {
            (_, None) => false,
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => true,
        }
    }
}

/// Entry stored per column in a patchset UPDATE: a unit (the format
/// does not carry the old value) plus the new value (or `None` for
/// unchanged columns).
pub type PatchsetUpdateEntry<S, B> = ((), Option<Value<S, B>>);

/// View over one operation in a changeset.
#[derive(Debug)]
pub enum ChangesetOp<'a, T, S, B> {
    /// `INSERT`. Carries every column's value in column order.
    Insert {
        /// Table this row belongs to.
        table: &'a T,
        /// Full row values, one per column.
        values: &'a [Value<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// `UPDATE`. Carries `(old, new)` pairs per column. `None` in either
    /// slot means "undefined" (the column was not part of the diff).
    Update {
        /// Table this row belongs to.
        table: &'a T,
        /// `(old, new)` pairs, one per column.
        values: &'a [ChangesetUpdatePair<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// `DELETE`. Carries the full old-row values in column order.
    Delete {
        /// Table this row belongs to.
        table: &'a T,
        /// Full old-row values, one per column.
        old_values: &'a [Value<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
}

impl<'a, T, S, B> ChangesetOp<'a, T, S, B> {
    /// Returns the schema of the table this operation applies to.
    #[must_use]
    pub fn table(&self) -> &'a T {
        match self {
            Self::Insert { table, .. }
            | Self::Update { table, .. }
            | Self::Delete { table, .. } => table,
        }
    }

    /// Returns the SQLite session-extension indirect flag.
    #[must_use]
    pub fn indirect(&self) -> bool {
        match self {
            Self::Insert { indirect, .. }
            | Self::Update { indirect, .. }
            | Self::Delete { indirect, .. } => *indirect,
        }
    }
}

impl<'a, T, S: Clone + Debug + AsRef<str>, B: Clone + Debug + AsRef<[u8]>>
    ChangesetOp<'a, T, S, B>
{
    /// The view of `op`, an operation stored for `table`.
    pub(crate) fn of_stored(table: &'a T, op: &'a Operation<ChangesetFormat, S, B>) -> Self {
        match op {
            Operation::Insert { values, indirect } => Self::Insert {
                table,
                values: values.as_slice(),
                indirect: *indirect,
            },
            Operation::Update { values, indirect } => Self::Update {
                table,
                values: values.as_slice(),
                indirect: *indirect,
            },
            Operation::Delete { data, indirect } => Self::Delete {
                table,
                old_values: data.as_slice(),
                indirect: *indirect,
            },
        }
    }
}

impl<T, S, B> ChangesetOp<'_, T, S, B>
where
    Value<S, B>: PartialEq,
{
    /// Returns the indices of the columns whose pairs are
    /// [`ChangesetUpdatePairExt::is_changed`], and nothing for a non-UPDATE.
    pub fn changed_column_indices(&self) -> impl Iterator<Item = usize> {
        let values: &[ChangesetUpdatePair<S, B>] = match self {
            Self::Update { values, .. } => values,
            Self::Insert { .. } | Self::Delete { .. } => &[],
        };
        values
            .iter()
            .enumerate()
            .filter_map(|(index, pair)| pair.is_changed().then_some(index))
    }
}

impl<T: SchemaWithPK, S: Clone, B: Clone> ChangesetOp<'_, T, S, B> {
    /// Returns the primary-key cells of this operation, in key order.
    ///
    /// `Insert` and `Delete` read the key from the full row via
    /// [`SchemaWithPK::extract_pk`]. For `Update` each key cell is taken
    /// old-first: a changeset UPDATE carries the key in the old slot as the
    /// row identity, while the new slot is absent for a key column that did
    /// not change, so reading the new slot (as `extract_pk` would over the
    /// pair storage) can yield `None`.
    #[must_use]
    pub fn primary_key(&self) -> Vec<Value<S, B>> {
        match *self {
            Self::Insert { table, values, .. } => table.extract_pk(&values),
            Self::Delete {
                table, old_values, ..
            } => table.extract_pk(&old_values),
            Self::Update { table, values, .. } => table
                .primary_key_columns()
                .map(|col_idx| {
                    let (old, new) = &values[col_idx];
                    old.clone().or_else(|| new.clone()).unwrap_or(Value::Null)
                })
                .collect(),
        }
    }
}

/// View over one operation in a patchset.
#[derive(Debug)]
pub enum PatchsetOp<'a, T, S, B> {
    /// `INSERT`. Carries every column's value in column order.
    Insert {
        /// Table this row belongs to.
        table: &'a T,
        /// Full row values, one per column.
        values: &'a [Value<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// `UPDATE`. Carries primary-key values plus a `(unit, new)` entry
    /// per non-PK column. The unit reflects the patchset format's
    /// missing old-value storage; consumers can map `|(_, v)| v` to walk
    /// just the new values.
    Update {
        /// Table this row belongs to.
        table: &'a T,
        /// Primary-key column values for the row being updated.
        pk: &'a [Value<S, B>],
        /// `(unit, new)` entries, one per column.
        entries: &'a [PatchsetUpdateEntry<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
    /// `DELETE`. Carries only the primary-key columns; the rest of the
    /// old row is not stored in patchset format.
    Delete {
        /// Table this row belongs to.
        table: &'a T,
        /// Primary-key column values for the row being deleted.
        pk: &'a [Value<S, B>],
        /// SQLite session-extension indirect flag.
        indirect: bool,
    },
}

impl<'a, T, S, B> PatchsetOp<'a, T, S, B> {
    /// Returns the schema of the table this operation applies to.
    #[must_use]
    pub fn table(&self) -> &'a T {
        match self {
            Self::Insert { table, .. }
            | Self::Update { table, .. }
            | Self::Delete { table, .. } => table,
        }
    }

    /// Returns the SQLite session-extension indirect flag.
    #[must_use]
    pub fn indirect(&self) -> bool {
        match self {
            Self::Insert { indirect, .. }
            | Self::Update { indirect, .. }
            | Self::Delete { indirect, .. } => *indirect,
        }
    }

    /// For an `Update` op, returns the new values per column (with the
    /// unit dropped). Returns `None` for `Insert` and `Delete`.
    #[must_use]
    pub fn update_new_values(&self) -> Option<Vec<&'a Option<Value<S, B>>>> {
        match self {
            Self::Update { entries, .. } => Some(entries.iter().map(|((), v)| v).collect()),
            _ => None,
        }
    }
}

impl<'a, T, S, B> PatchsetOp<'a, T, S, B> {
    /// The view of `op`, an operation stored for `table` under the primary key `pk`.
    pub(crate) fn of_stored(
        table: &'a T,
        pk: &'a [Value<S, B>],
        op: &'a Operation<PatchsetFormat, S, B>,
    ) -> Self {
        match op {
            Operation::Insert { values, indirect } => Self::Insert {
                table,
                values: values.as_slice(),
                indirect: *indirect,
            },
            Operation::Update { values, indirect } => Self::Update {
                table,
                pk,
                entries: values.as_slice(),
                indirect: *indirect,
            },
            Operation::Delete { indirect, .. } => Self::Delete {
                table,
                pk,
                indirect: *indirect,
            },
        }
    }
}

impl<T: SchemaWithPK, S: Clone, B: Clone> PatchsetOp<'_, T, S, B> {
    /// Returns the primary-key cells of this operation, in key order.
    ///
    /// `Insert` reads the key from the full row via
    /// [`SchemaWithPK::extract_pk`]. `Update` and `Delete` already store only
    /// the key columns in key order, so their cells are returned directly.
    #[must_use]
    pub fn primary_key(&self) -> Vec<Value<S, B>> {
        match *self {
            Self::Insert { table, values, .. } => table.extract_pk(&values),
            Self::Update { pk, .. } | Self::Delete { pk, .. } => pk.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChangeDelete, ChangeSet, DiffOps, Insert, ParsedDiffSet, PatchSet, SimpleTable};
    use alloc::string::String;
    use alloc::vec;

    type Pair = ChangesetUpdatePair<String, Vec<u8>>;
    type Entry = PatchsetUpdateEntry<String, Vec<u8>>;
    type Val = Value<String, Vec<u8>>;

    #[test]
    fn changeset_primary_key_single_key() {
        let schema = SimpleTable::with_rowid_alias("kv", &[("id", "INTEGER"), ("val", "TEXT")], 0);

        let insert_values: Vec<Val> = vec![Value::Integer(1), Value::Text("a".into())];
        let insert = ChangesetOp::Insert {
            table: &schema,
            values: &insert_values,
            indirect: false,
        };
        assert_eq!(insert.primary_key(), vec![Value::Integer(1)]);

        // UPDATE touching only the non-key column: the key sits in the old slot
        // with an undefined (None) new slot, so old-first must recover it.
        let update_values: Vec<Pair> = vec![
            (Some(Value::Integer(2)), None),
            (
                Some(Value::Text("before".into())),
                Some(Value::Text("after".into())),
            ),
        ];
        let update = ChangesetOp::Update {
            table: &schema,
            values: &update_values,
            indirect: false,
        };
        assert_eq!(update.primary_key(), vec![Value::Integer(2)]);

        let delete_values: Vec<Val> = vec![Value::Integer(3), Value::Text("gone".into())];
        let delete = ChangesetOp::Delete {
            table: &schema,
            old_values: &delete_values,
            indirect: false,
        };
        assert_eq!(delete.primary_key(), vec![Value::Integer(3)]);
    }

    #[test]
    fn changeset_primary_key_composite_reordered_key() {
        // Columns (a, b, c), key (b, a): flags [2, 1, 0], key order is b then a.
        let schema = SimpleTable::new(
            "abc",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("c", "TEXT")],
            &[1, 0],
        );
        let expected: Vec<Val> = vec![Value::Integer(20), Value::Integer(10)];

        let insert_values: Vec<Val> = vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Text("z".into()),
        ];
        let insert = ChangesetOp::Insert {
            table: &schema,
            values: &insert_values,
            indirect: false,
        };
        assert_eq!(insert.primary_key(), expected);

        // Only non-key column c changes, so both key columns are undefined in
        // the new slot and must be recovered from the old slot.
        let update_values: Vec<Pair> = vec![
            (Some(Value::Integer(10)), None),
            (Some(Value::Integer(20)), None),
            (
                Some(Value::Text("z".into())),
                Some(Value::Text("z2".into())),
            ),
        ];
        let update = ChangesetOp::Update {
            table: &schema,
            values: &update_values,
            indirect: false,
        };
        assert_eq!(update.primary_key(), expected);

        let delete_values: Vec<Val> = vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Text("z".into()),
        ];
        let delete = ChangesetOp::Delete {
            table: &schema,
            old_values: &delete_values,
            indirect: false,
        };
        assert_eq!(delete.primary_key(), expected);
    }

    #[test]
    fn patchset_primary_key_variants() {
        let kv = SimpleTable::with_rowid_alias("kv", &[("id", "INTEGER"), ("val", "TEXT")], 0);

        let insert_values: Vec<Val> = vec![Value::Integer(1), Value::Text("a".into())];
        let insert = PatchsetOp::Insert {
            table: &kv,
            values: &insert_values,
            indirect: false,
        };
        assert_eq!(insert.primary_key(), vec![Value::Integer(1)]);

        let update_pk: Vec<Val> = vec![Value::Integer(9)];
        let entries: Vec<Entry> = vec![((), None), ((), Some(Value::Text("z".into())))];
        let update = PatchsetOp::Update {
            table: &kv,
            pk: &update_pk,
            entries: &entries,
            indirect: false,
        };
        assert_eq!(update.primary_key(), vec![Value::Integer(9)]);

        let delete_pk: Vec<Val> = vec![Value::Integer(7)];
        let delete = PatchsetOp::Delete {
            table: &kv,
            pk: &delete_pk,
            indirect: false,
        };
        assert_eq!(delete.primary_key(), vec![Value::Integer(7)]);

        // Composite key: INSERT recovers key order (b, a) from the full row.
        let abc = SimpleTable::new(
            "abc",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("c", "TEXT")],
            &[1, 0],
        );
        let abc_values: Vec<Val> = vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Text("z".into()),
        ];
        let abc_insert = PatchsetOp::Insert {
            table: &abc,
            values: &abc_values,
            indirect: false,
        };
        assert_eq!(
            abc_insert.primary_key(),
            vec![Value::Integer(20), Value::Integer(10)]
        );
    }

    #[test]
    fn changeset_primary_key_through_iter_composite() {
        // Build a real changeset and read the key back through iter(), so the
        // extract_pk key ordering is exercised end to end.
        let schema = SimpleTable::new(
            "abc",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("c", "TEXT")],
            &[1, 0],
        );
        let cs: ChangeSet<SimpleTable, String, Vec<u8>> = ChangeSet::new().insert(
            Insert::from(schema)
                .set(0, 10i64)
                .unwrap()
                .set(1, 20i64)
                .unwrap()
                .set(2, "z")
                .unwrap(),
        );
        let ops: Vec<_> = cs.iter().collect();
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].primary_key(),
            vec![Value::Integer(20), Value::Integer(10)]
        );
    }

    #[test]
    fn patchset_primary_key_through_iter_composite_delete() {
        // A digested DELETE stores its key in key order; primary_key() must
        // return it in that same order through iter().
        let schema = SimpleTable::new(
            "abc",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("c", "TEXT")],
            &[1, 0],
        );
        let mut ps: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
        ps.add_table(&schema);
        ps.digest_sql("DELETE FROM abc WHERE a = 10 AND b = 20")
            .unwrap();
        let ops: Vec<_> = ps.iter().collect();
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].primary_key(),
            vec![Value::Integer(20), Value::Integer(10)]
        );
    }

    #[test]
    fn primary_key_on_parsed_changeset_ops() {
        // The whole point: ops from a parsed diff are over TableSchema<String>,
        // not SimpleTable. primary_key() must be callable there.
        let schema = SimpleTable::with_rowid_alias("kv", &[("id", "INTEGER"), ("val", "TEXT")], 0);
        let bytes = ChangeSet::<SimpleTable, String, Vec<u8>>::new()
            .insert(
                Insert::from(schema.clone())
                    .set(0, 1i64)
                    .unwrap()
                    .set(1, "a")
                    .unwrap(),
            )
            .delete(
                ChangeDelete::from(schema)
                    .set(0, 2i64)
                    .unwrap()
                    .set(1, "b")
                    .unwrap(),
            )
            .build();
        let ParsedDiffSet::Changeset(set) = ParsedDiffSet::parse(&bytes).unwrap() else {
            panic!("expected changeset");
        };
        let keys: Vec<Vec<Val>> = set.iter().map(|op| op.primary_key()).collect();
        assert!(keys.contains(&vec![Value::Integer(1)]));
        assert!(keys.contains(&vec![Value::Integer(2)]));
    }

    #[test]
    fn primary_key_on_parsed_patchset_ops() {
        let schema = SimpleTable::with_rowid_alias("kv", &[("id", "INTEGER"), ("val", "TEXT")], 0);
        let mut ps: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
        ps.add_table(&schema);
        ps.digest_sql("INSERT INTO kv (id, val) VALUES (1, 'a')")
            .unwrap();
        ps.digest_sql("DELETE FROM kv WHERE id = 2").unwrap();
        let bytes = ps.build();
        let ParsedDiffSet::Patchset(set) = ParsedDiffSet::parse(&bytes).unwrap() else {
            panic!("expected patchset");
        };
        let keys: Vec<Vec<Val>> = set.iter().map(|op| op.primary_key()).collect();
        assert!(keys.contains(&vec![Value::Integer(1)]));
        assert!(keys.contains(&vec![Value::Integer(2)]));
    }
}
