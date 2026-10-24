//! SQL statement generation from changesets and patchsets.
//!
//! Converts changeset and patchset operations back into SQL statements.
//! Useful for debugging, logging, or applying changes to non-SQLite databases.
//!
//! # Example
//!
//! ```rust
//! use sqlite_diff_rs::{SimpleTable, PatchSet, DiffOps, Insert};
//!
//! let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
//! let insert = Insert::from(table.clone())
//!     .set(0, 1i64).unwrap()
//!     .set(1, "Alice").unwrap();
//!
//! let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
//!
//! for sql in patchset.sql_statements() {
//!     println!("{}", sql);
//!     // Prints: INSERT INTO "users" ("id", "name") VALUES (1, 'Alice')
//! }
//! ```

use alloc::string::String;
use core::fmt::{Debug, Write};
use core::hash::Hash;

use crate::builders::operation::Operation;
use crate::builders::{ChangesetFormat, DiffSetBuilder, PatchsetFormat};
use crate::encoding::{MaybeValue, Value};
use crate::schema::NamedColumns;

/// Type alias for changeset update pairs: (old_value, new_value) for each column.
type ChangesetUpdatePairs<S, B> = [(MaybeValue<S, B>, MaybeValue<S, B>)];

/// Quote a SQL identifier (table or column name) with double quotes.
///
/// Escapes any embedded double quotes by doubling them.
fn quote_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        if c == '"' {
            out.push_str("\"\"");
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}

/// Trait for tables that can provide column names by index.
///
/// This extends [`NamedColumns`] to support SQL generation.
pub trait ColumnNames: NamedColumns {
    /// Get the column name for a given index.
    ///
    /// Returns `None` if the index is out of bounds.
    fn column_name(&self, index: usize) -> Option<&str>;
}

impl ColumnNames for crate::SimpleTable {
    fn column_name(&self, index: usize) -> Option<&str> {
        self.column_name(index)
    }
}

/// Format an INSERT statement with column names.
fn format_insert<T: ColumnNames, S: AsRef<str>, B: AsRef<[u8]>>(
    table: &T,
    values: &[Value<S, B>],
) -> String {
    let mut sql = String::new();
    write!(sql, "INSERT INTO {}", quote_identifier(table.name())).unwrap();

    // Column names
    sql.push_str(" (");
    for i in 0..table.number_of_columns() {
        if i > 0 {
            sql.push_str(", ");
        }
        if let Some(name) = table.column_name(i) {
            sql.push_str(&quote_identifier(name));
        } else {
            write!(sql, "\"col{i}\"").unwrap();
        }
    }
    sql.push_str(") VALUES (");

    // Values
    for (i, val) in values.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        write!(sql, "{val}").unwrap();
    }
    sql.push(')');
    sql
}

/// Appends `column = value` for each `(column index, value)` cell, joined by `separator`.
fn push_cells<'a, T: ColumnNames, S: AsRef<str> + 'a, B: AsRef<[u8]> + 'a>(
    sql: &mut String,
    table: &T,
    separator: &str,
    cells: impl Iterator<Item = (usize, &'a Value<S, B>)>,
) {
    for (i, (col_idx, value)) in cells.enumerate() {
        if i > 0 {
            sql.push_str(separator);
        }
        if let Some(name) = table.column_name(col_idx) {
            sql.push_str(&quote_identifier(name));
        } else {
            write!(sql, "\"col{col_idx}\"").unwrap();
        }
        sql.push_str(" = ");
        write!(sql, "{value}").unwrap();
    }
}

/// The primary-key cells of a row whose key values are `pk`, in key order.
fn pk_cells<'a, T: ColumnNames, S, B>(
    table: &'a T,
    pk: &'a [Value<S, B>],
) -> impl Iterator<Item = (usize, &'a Value<S, B>)> {
    table
        .primary_key_columns()
        .enumerate()
        .map(move |(pk_ordinal, col_idx)| (col_idx, &pk[pk_ordinal]))
}

/// The non-key cells an UPDATE sets, from each column's new value.
fn set_cells<'a, T: ColumnNames, S: 'a, B: 'a>(
    table: &'a T,
    news: impl Iterator<Item = &'a MaybeValue<S, B>>,
) -> impl Iterator<Item = (usize, &'a Value<S, B>)> {
    news.enumerate()
        .filter_map(|(col_idx, new)| new.as_ref().map(|value| (col_idx, value)))
        .filter(move |(col_idx, _)| table.primary_key_index(*col_idx).is_none())
}

/// Format a DELETE statement (changeset format - has all old values).
fn format_delete_changeset<T: ColumnNames, S: AsRef<str>, B: AsRef<[u8]>>(
    table: &T,
    values: &[Value<S, B>],
) -> String {
    let mut sql = String::new();
    write!(sql, "DELETE FROM {}", quote_identifier(table.name())).unwrap();
    sql.push_str(" WHERE ");
    // Use PK columns for the WHERE clause
    let keys = values
        .iter()
        .enumerate()
        .filter(|(col_idx, _)| table.primary_key_index(*col_idx).is_some());
    push_cells(&mut sql, table, " AND ", keys);
    sql
}

/// Format a DELETE statement (patchset format - PK only).
fn format_delete_patchset<T: ColumnNames, S: AsRef<str>, B: AsRef<[u8]>>(
    table: &T,
    pk: &[Value<S, B>],
) -> String {
    let mut sql = String::new();
    write!(sql, "DELETE FROM {}", quote_identifier(table.name())).unwrap();
    sql.push_str(" WHERE ");
    push_cells(&mut sql, table, " AND ", pk_cells(table, pk));
    sql
}

/// Format an UPDATE statement (changeset format).
fn format_update_changeset<T: ColumnNames, S: AsRef<str>, B: AsRef<[u8]>>(
    table: &T,
    pairs: &ChangesetUpdatePairs<S, B>,
) -> String {
    let mut sql = String::new();
    write!(sql, "UPDATE {}", quote_identifier(table.name())).unwrap();
    sql.push_str(" SET ");
    // SET clause: columns that changed (have new values), PK columns go in WHERE
    push_cells(
        &mut sql,
        table,
        ", ",
        set_cells(table, pairs.iter().map(|(_old, new)| new)),
    );
    // WHERE clause: use old PK values
    sql.push_str(" WHERE ");
    let old_keys = pairs
        .iter()
        .enumerate()
        .filter(|(col_idx, _)| table.primary_key_index(*col_idx).is_some())
        .filter_map(|(col_idx, (old, _new))| old.as_ref().map(|value| (col_idx, value)));
    push_cells(&mut sql, table, " AND ", old_keys);
    sql
}

/// Format an UPDATE statement (patchset format).
fn format_update_patchset<T: ColumnNames, S: AsRef<str>, B: AsRef<[u8]>>(
    table: &T,
    pk: &[Value<S, B>],
    pairs: &[((), MaybeValue<S, B>)],
) -> String {
    let mut sql = String::new();
    write!(sql, "UPDATE {}", quote_identifier(table.name())).unwrap();
    sql.push_str(" SET ");
    // SET clause: columns that changed (have new values), excluding PK
    push_cells(
        &mut sql,
        table,
        ", ",
        set_cells(table, pairs.iter().map(|((), new)| new)),
    );
    sql.push_str(" WHERE ");
    push_cells(&mut sql, table, " AND ", pk_cells(table, pk));
    sql
}

// ============================================================================
// DiffSetBuilder::sql_statements() implementations
// ============================================================================

impl<
    T: ColumnNames,
    S: AsRef<str> + Clone + Debug + Hash + Eq,
    B: AsRef<[u8]> + Clone + Debug + Hash + Eq,
> DiffSetBuilder<ChangesetFormat, T, S, B>
{
    /// Iterate over operations as SQL statements.
    ///
    /// Each operation is converted to an INSERT, UPDATE, or DELETE statement.
    /// Identifiers (table and column names) are quoted with double quotes.
    /// Statements do not include trailing semicolons.
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlite_diff_rs::{SimpleTable, ChangeSet, DiffOps, Insert};
    ///
    /// let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    /// let insert = Insert::from(table.clone())
    ///     .set(0, 1i64).unwrap()
    ///     .set(1, "Alice").unwrap();
    ///
    /// let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
    ///
    /// for sql in changeset.sql_statements() {
    ///     assert_eq!(sql, r#"INSERT INTO "users" ("id", "name") VALUES (1, 'Alice')"#);
    /// }
    /// ```
    pub fn sql_statements(&self) -> impl Iterator<Item = String> + '_ {
        self.tables.iter().flat_map(|(table, rows)| {
            rows.values().map(move |op| match op {
                Operation::Insert { values, .. } => format_insert(table, values),
                Operation::Delete { data: values, .. } => format_delete_changeset(table, values),
                Operation::Update { values, .. } => format_update_changeset(table, values),
            })
        })
    }
}

impl<T: ColumnNames, S: AsRef<str> + Clone + Hash + Eq, B: AsRef<[u8]> + Clone + Hash + Eq>
    DiffSetBuilder<PatchsetFormat, T, S, B>
{
    /// Iterate over operations as SQL statements.
    ///
    /// Each operation is converted to an INSERT, UPDATE, or DELETE statement.
    /// Identifiers (table and column names) are quoted with double quotes.
    /// Statements do not include trailing semicolons.
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlite_diff_rs::{SimpleTable, PatchSet, DiffOps, Insert};
    ///
    /// let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    /// let insert = Insert::from(table.clone())
    ///     .set(0, 1i64).unwrap()
    ///     .set(1, "Alice").unwrap();
    ///
    /// let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
    ///
    /// for sql in patchset.sql_statements() {
    ///     assert_eq!(sql, r#"INSERT INTO "users" ("id", "name") VALUES (1, 'Alice')"#);
    /// }
    /// ```
    pub fn sql_statements(&self) -> impl Iterator<Item = String> + '_ {
        self.tables.iter().flat_map(|(table, rows)| {
            rows.iter().map(move |(pk, op)| match op {
                Operation::Insert { values, .. } => format_insert(table, values),
                Operation::Delete { data: (), .. } => format_delete_patchset(table, pk),
                Operation::Update { values, .. } => format_update_patchset(table, pk, values),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ChangeDelete, ChangeSet, DiffOps, Insert, PatchDelete, PatchSet, SimpleTable, Update,
    };
    use alloc::vec::Vec;

    #[test]
    fn test_quote_identifier_simple() {
        assert_eq!(quote_identifier("users"), r#""users""#);
    }

    #[test]
    fn test_quote_identifier_with_quotes() {
        assert_eq!(quote_identifier(r#"user"name"#), r#""user""name""#);
    }

    #[test]
    fn test_changeset_insert_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let insert = Insert::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "Alice")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            r#"INSERT INTO "users" ("id", "name") VALUES (1, 'Alice')"#
        );
    }

    #[test]
    fn test_patchset_insert_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let insert = Insert::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "Alice")
            .unwrap();

        let ps = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
        let stmts: Vec<_> = ps.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            r#"INSERT INTO "users" ("id", "name") VALUES (1, 'Alice')"#
        );
    }

    #[test]
    fn test_changeset_delete_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let delete = ChangeDelete::from(table.clone())
            .set(0, 42i64)
            .unwrap()
            .set(1, "Bob")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().delete(delete);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], r#"DELETE FROM "users" WHERE "id" = 42"#);
    }

    #[test]
    fn test_patchset_delete_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let delete = PatchDelete::new(table.clone(), alloc::vec![Value::Integer(42)]);

        let ps = PatchSet::<SimpleTable, String, Vec<u8>>::new().delete(delete);
        let stmts: Vec<_> = ps.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], r#"DELETE FROM "users" WHERE "id" = 42"#);
    }

    #[test]
    fn test_changeset_update_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let update = Update::<SimpleTable, ChangesetFormat, String, Vec<u8>>::from(table.clone())
            .set(0, 1i64, 1i64)
            .unwrap()
            .set(1, "Alice", "Alicia")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(update);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            r#"UPDATE "users" SET "name" = 'Alicia' WHERE "id" = 1"#
        );
    }

    #[test]
    fn test_patchset_update_sql() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let update = Update::<SimpleTable, PatchsetFormat, String, Vec<u8>>::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "Alicia")
            .unwrap();

        let ps = PatchSet::<SimpleTable, String, Vec<u8>>::new().update(update);
        let stmts: Vec<_> = ps.sql_statements().collect();

        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            r#"UPDATE "users" SET "name" = 'Alicia' WHERE "id" = 1"#
        );
    }

    #[test]
    fn test_sql_escapes_quotes_in_strings() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let insert = Insert::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "O'Brien")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(
            stmts[0],
            r#"INSERT INTO "users" ("id", "name") VALUES (1, 'O''Brien')"#
        );
    }

    #[test]
    fn test_sql_escapes_quotes_in_identifiers() {
        let table = SimpleTable::with_rowid_alias(
            r#"user"table"#,
            &[("id", "INTEGER"), (r#"user"name"#, "TEXT")],
            0,
        );
        let insert = Insert::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "Alice")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(insert);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(
            stmts[0],
            r#"INSERT INTO "user""table" ("id", "user""name") VALUES (1, 'Alice')"#
        );
    }

    #[test]
    fn test_multiple_operations() {
        let table =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);

        let insert1 = Insert::from(table.clone())
            .set(0, 1i64)
            .unwrap()
            .set(1, "Alice")
            .unwrap();

        let insert2 = Insert::from(table.clone())
            .set(0, 2i64)
            .unwrap()
            .set(1, "Bob")
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new()
            .insert(insert1)
            .insert(insert2);

        let stmts: Vec<_> = cs.sql_statements().collect();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_composite_pk_delete() {
        let table = SimpleTable::new(
            "order_items",
            &[
                ("order_id", "INTEGER"),
                ("item_id", "INTEGER"),
                ("qty", "INTEGER"),
            ],
            &[0, 1],
        );
        let delete = ChangeDelete::from(table.clone())
            .set(0, 100i64)
            .unwrap()
            .set(1, 5i64)
            .unwrap()
            .set(2, 10i64)
            .unwrap();

        let cs = ChangeSet::<SimpleTable, String, Vec<u8>>::new().delete(delete);
        let stmts: Vec<_> = cs.sql_statements().collect();

        assert_eq!(
            stmts[0],
            r#"DELETE FROM "order_items" WHERE "order_id" = 100 AND "item_id" = 5"#
        );
    }

    /// Test schema whose ColumnNames impl returns `None` for non-PK columns.
    /// Drives the `"\"col{col_idx}\""` fallback path in INSERT/UPDATE SET
    /// and DELETE WHERE rendering.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct AnonColsTable {
        name: alloc::string::String,
        num_columns: usize,
        pk_column: usize,
    }

    impl crate::DynTable for AnonColsTable {
        fn name(&self) -> &str {
            &self.name
        }
        fn number_of_columns(&self) -> usize {
            self.num_columns
        }
        fn write_pk_flags(&self, buf: &mut [u8]) {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = u8::from(i == self.pk_column);
            }
        }
    }

    impl crate::SchemaWithPK for AnonColsTable {
        fn number_of_primary_keys(&self) -> usize {
            1
        }
        fn primary_key_index(&self, col_idx: usize) -> Option<usize> {
            (col_idx == self.pk_column).then_some(0)
        }
        fn extract_pk<S: Clone, B: Clone>(
            &self,
            values: &impl crate::IndexableValues<Text = S, Binary = B>,
        ) -> Vec<Value<S, B>> {
            alloc::vec![values.get(self.pk_column).unwrap()]
        }
    }

    impl crate::schema::NamedColumns for AnonColsTable {
        fn column_index(&self, _column_name: &str) -> Option<usize> {
            None
        }
        fn column_affinity(&self, column_index: usize) -> Option<crate::Affinity> {
            (column_index < self.num_columns).then_some(crate::Affinity::Blob)
        }
        fn rowid_alias(&self) -> Option<usize> {
            None
        }
    }

    impl crate::ColumnNames for AnonColsTable {
        // Always return None: every column rendered should use the
        // \"col{col_idx}\" fallback.
        fn column_name(&self, _index: usize) -> Option<&str> {
            None
        }
    }

    #[test]
    fn test_sql_output_uses_col_index_fallback() {
        let table = AnonColsTable {
            name: "t".into(),
            num_columns: 2,
            pk_column: 0,
        };
        let insert = Insert::from(table.clone())
            .set(0, 42i64)
            .unwrap()
            .set(1, "hello")
            .unwrap();
        let cs = ChangeSet::<AnonColsTable, String, Vec<u8>>::new().insert(insert);
        let stmts: Vec<_> = cs.sql_statements().collect();
        assert_eq!(
            stmts[0],
            r#"INSERT INTO "t" ("col0", "col1") VALUES (42, 'hello')"#
        );
    }

    #[test]
    fn test_sql_output_col_index_fallback_in_patchset_delete() {
        let table = AnonColsTable {
            name: "t".into(),
            num_columns: 2,
            pk_column: 0,
        };
        let delete: PatchDelete<_, String, Vec<u8>> =
            PatchDelete::new(table.clone(), alloc::vec![Value::Integer(7)]);
        let ps = PatchSet::<AnonColsTable, String, Vec<u8>>::new().delete(delete);
        let stmts: Vec<_> = ps.sql_statements().collect();
        assert_eq!(stmts[0], r#"DELETE FROM "t" WHERE "col0" = 7"#);
    }
}
