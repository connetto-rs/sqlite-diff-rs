//! Submodule defining a builder for a delete operation.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Debug;

use crate::{DynTable, builders::operation::Indirect, encoding::Value};

#[derive(Debug, Clone)]
/// Represents a delete operation in changeset format.
///
/// Stores the full old-row values for all columns.
pub struct ChangeDelete<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> {
    pub(crate) table: T,
    /// Old values for the deleted row.
    pub(crate) values: Vec<Value<S, B>>,
    /// SQLite session-extension indirect flag. See [`Indirect`].
    pub(crate) indirect: bool,
}

impl<T: DynTable + PartialEq, S: PartialEq + AsRef<str>, B: PartialEq + AsRef<[u8]>> PartialEq
    for ChangeDelete<T, S, B>
{
    fn eq(&self, other: &Self) -> bool {
        self.table == other.table && self.values == other.values && self.indirect == other.indirect
    }
}

impl<T: DynTable + Eq, S: Eq + AsRef<str>, B: Eq + AsRef<[u8]>> Eq for ChangeDelete<T, S, B> {}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> AsRef<T> for ChangeDelete<T, S, B> {
    #[inline]
    fn as_ref(&self) -> &T {
        &self.table
    }
}

impl<T: DynTable, S: Default + Clone + AsRef<str>, B: Default + Clone + AsRef<[u8]>> From<T>
    for ChangeDelete<T, S, B>
{
    #[inline]
    fn from(table: T) -> Self {
        let num_cols = table.number_of_columns();
        Self {
            table,
            values: vec![Value::Null; num_cols],
            indirect: false,
        }
    }
}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> ChangeDelete<T, S, B> {
    /// Sets the value for a specific column by index.
    ///
    /// The value is recorded as given. Use [`Affinity::apply`](crate::Affinity::apply)
    /// for the conversion SQLite applies when storing it in the column.
    ///
    /// # Arguments
    ///
    /// * `col_idx` - The index of the column to set.
    /// * `value` - The value to set for the column.
    ///
    /// # Errors
    ///
    /// * `ColumnIndexOutOfBounds` - If the provided column index is out of bounds for the table schema.
    ///
    pub fn set(
        mut self,
        col_idx: usize,
        value: impl Into<Value<S, B>>,
    ) -> Result<Self, crate::errors::Error> {
        if col_idx >= self.values.len() {
            return Err(crate::errors::Error::ColumnIndexOutOfBounds(
                col_idx,
                self.values.len(),
            ));
        }
        self.values[col_idx] = value.into();
        Ok(self)
    }

    /// Sets a column to NULL.
    ///
    /// This is a convenience method equivalent to `.set(col_idx, Value::Null)`.
    ///
    /// # Errors
    ///
    /// * `ColumnIndexOutOfBounds` - If the provided column index is out of bounds for the table schema.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlite_diff_rs::{ChangeDelete, TableSchema};
    ///
    /// // CREATE TABLE items (id INTEGER PRIMARY KEY, description TEXT)
    /// let schema: TableSchema<String> = TableSchema::new("items".into(), 2, vec![1, 0]);
    ///
    /// // DELETE FROM items WHERE id = 1 AND description IS NULL
    /// let delete = ChangeDelete::<_, String, Vec<u8>>::from(schema)
    ///     .set(0, 1i64).unwrap()
    ///     .set_null(1).unwrap();
    /// ```
    #[inline]
    pub fn set_null(self, col_idx: usize) -> Result<Self, crate::errors::Error>
    where
        S: Default,
        B: Default,
    {
        self.set(col_idx, Value::Null)
    }

    /// Consumes self and returns the values.
    #[inline]
    pub(crate) fn into_values(self) -> Vec<Value<S, B>> {
        self.values
    }
}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> Indirect for ChangeDelete<T, S, B> {
    #[inline]
    fn indirect(mut self, indirect: bool) -> Self {
        self.indirect = indirect;
        self
    }
}

/// Represents a delete operation in patchset format.
///
/// Only stores the table schema and primary key values, as patchsets
/// don't include full row data for deletions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchDelete<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> {
    pub(crate) table: T,
    /// Primary key values for the deleted row.
    pub(crate) pk: Vec<Value<S, B>>,
    /// SQLite session-extension indirect flag. See [`Indirect`].
    pub(crate) indirect: bool,
}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> AsRef<T> for PatchDelete<T, S, B> {
    #[inline]
    fn as_ref(&self) -> &T {
        &self.table
    }
}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> PatchDelete<T, S, B> {
    /// Create a new patchset delete for the given table and primary key values.
    ///
    /// The `pk` values should be the primary key column values, in the order
    /// they appear in the table schema. This is the same format returned by
    /// [`crate::SchemaWithPK::extract_pk`].
    #[inline]
    pub fn new(table: T, pk: Vec<Value<S, B>>) -> Self {
        Self {
            table,
            pk,
            indirect: false,
        }
    }
}

impl<T: DynTable, S: AsRef<str>, B: AsRef<[u8]>> Indirect for PatchDelete<T, S, B> {
    #[inline]
    fn indirect(mut self, indirect: bool) -> Self {
        self.indirect = indirect;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{ChangeDelete, PatchDelete};
    use crate::DynTable;
    use crate::encoding::Value;
    use crate::errors::Error;
    use crate::schema::SimpleTable;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    fn users() -> SimpleTable {
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0)
    }

    #[test]
    fn test_change_delete_out_of_bounds_columns() {
        let delete = ChangeDelete::<_, String, Vec<u8>>::from(users());
        let err = delete.clone().set(5, 1i64).unwrap_err();
        assert!(
            matches!(err, Error::ColumnIndexOutOfBounds(5, 2)),
            "got {err:?}"
        );
        let err = delete.set_null(2).unwrap_err();
        assert!(
            matches!(err, Error::ColumnIndexOutOfBounds(2, 2)),
            "got {err:?}"
        );
    }

    #[test]
    fn test_patch_delete_as_ref_returns_table() {
        let table = users();
        let delete: PatchDelete<_, String, Vec<u8>> =
            PatchDelete::new(table.clone(), vec![Value::Integer(1)]);
        let t: &SimpleTable = delete.as_ref();
        assert_eq!(t.name(), "users");
    }

    #[test]
    fn test_change_delete_eq() {
        let a = ChangeDelete::<_, String, Vec<u8>>::from(users())
            .set(0, 1i64)
            .unwrap();
        let b = ChangeDelete::<_, String, Vec<u8>>::from(users())
            .set(0, 1i64)
            .unwrap();
        assert_eq!(a, b);

        let c = ChangeDelete::<_, String, Vec<u8>>::from(users())
            .set(0, 2i64)
            .unwrap();
        assert_ne!(a, c);
    }
}
