//! Shared schema fixtures and operation assertions for the CDC digestion
//! integration tests.
//!
//! Provides a three-column `users` table (`id` INT PK, `name` TEXT, `active`
//! BOOL) with all required trait impls, plus the assertions every digestion
//! test shares for the canonical row images, reused by `digestion_pg_walstream`,
//! `digestion_wal2json`, and `digestion_maxwell`.

#![allow(dead_code, unused_imports)]

extern crate alloc;

use alloc::vec::Vec;

use sqlite_diff_rs::{
    ChangesetUpdatePair, DynTable, IndexableValues, NamedColumns, PatchsetUpdateEntry,
    SchemaWithPK, SimpleTable, Value, WireColumnTypes, WireSchema, WireType,
};

/// A single-table schema holding the `users` test table.
#[derive(Debug, Clone)]
pub struct TestSchema {
    pub users: TestUsersTable,
}

#[derive(Debug, Clone)]
pub struct SourceScopedTestSchema {
    pub users: TestUsersTable,
    source_schema: &'static str,
}

/// The `users` table: `id INT PK`, `name TEXT`, `active BOOL`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TestUsersTable(pub SimpleTable);

impl DynTable for TestUsersTable {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn number_of_columns(&self) -> usize {
        self.0.number_of_columns()
    }
    fn write_pk_flags(&self, buf: &mut [u8]) {
        self.0.write_pk_flags(buf);
    }
}

impl SchemaWithPK for TestUsersTable {
    fn extract_pk<S: Clone, B: Clone>(
        &self,
        values: &impl IndexableValues<Text = S, Binary = B>,
    ) -> Vec<Value<S, B>> {
        self.0.extract_pk(values)
    }
    fn number_of_primary_keys(&self) -> usize {
        self.0.number_of_primary_keys()
    }
    fn primary_key_index(&self, col: usize) -> Option<usize> {
        self.0.primary_key_index(col)
    }
}

impl NamedColumns for TestUsersTable {
    fn column_index(&self, name: &str) -> Option<usize> {
        self.0.column_index(name)
    }
    fn column_affinity(&self, column_index: usize) -> Option<sqlite_diff_rs::Affinity> {
        self.0.column_affinity(column_index)
    }
    fn rowid_alias(&self) -> Option<usize> {
        self.0.rowid_alias()
    }
}

impl WireColumnTypes for TestUsersTable {
    fn column_type(&self, column_index: usize) -> WireType {
        // id -> Int, name -> Text, active -> Bool
        match column_index {
            0 => WireType::Int,
            1 => WireType::Text,
            2 => WireType::Bool,
            _ => panic!("column {column_index} out of range"),
        }
    }
}

impl WireSchema for TestSchema {
    type Table = TestUsersTable;
    fn get(&self, _source_schema: Option<&str>, table_name: &str) -> Option<&Self::Table> {
        if table_name == "users" {
            Some(&self.users)
        } else {
            None
        }
    }
}

impl WireSchema for SourceScopedTestSchema {
    type Table = TestUsersTable;

    fn get(&self, source_schema: Option<&str>, table_name: &str) -> Option<&Self::Table> {
        (source_schema == Some(self.source_schema) && table_name == "users").then_some(&self.users)
    }
}

/// Constructs the canonical three-column `users` test schema.
pub fn test_schema() -> TestSchema {
    TestSchema {
        users: TestUsersTable(SimpleTable::with_rowid_alias(
            "users",
            &[("id", "INTEGER"), ("name", "TEXT"), ("active", "INTEGER")],
            0,
        )),
    }
}

pub fn source_scoped_test_schema(source_schema: &'static str) -> SourceScopedTestSchema {
    SourceScopedTestSchema {
        users: TestUsersTable(SimpleTable::with_rowid_alias(
            "users",
            &[("id", "INTEGER"), ("name", "TEXT"), ("active", "INTEGER")],
            0,
        )),
        source_schema,
    }
}

/// The encoded image of the canonical `users` row (`id` 1, `name`, `active` 1).
pub fn assert_users_row(values: &[Value<String, Vec<u8>>], name: &str) {
    assert_eq!(values.len(), 3, "three columns");
    assert_eq!(values[0], Value::Integer(1), "id");
    assert_eq!(values[1], Value::Text(name.to_string()), "name");
    assert_eq!(values[2], Value::Integer(1), "active=true encodes as 1");
}

/// The `(old, new)` image pairs of a canonical `users` `UPDATE`.
pub fn assert_users_update(values: &[ChangesetUpdatePair<String, Vec<u8>>]) {
    assert_eq!(values.len(), 3, "three columns");
    assert_eq!(values[0].0, Some(Value::Integer(1)), "old id");
    assert_eq!(
        values[1].0,
        Some(Value::Text("Alice".to_string())),
        "old name"
    );
    assert_eq!(
        values[1].1,
        Some(Value::Text("Alicia".to_string())),
        "new name"
    );
}

/// The primary key and new images of a canonical `users` patchset `UPDATE`.
pub fn assert_users_patchset_update(
    pk: &[Value<String, Vec<u8>>],
    entries: &[PatchsetUpdateEntry<String, Vec<u8>>],
) {
    assert_eq!(pk, &[Value::Integer(1)], "primary key");
    assert_eq!(entries.len(), 3, "three column entries");
    assert_eq!(
        entries[1].1,
        Some(Value::Text("Alicia".to_string())),
        "new name"
    );
    assert_eq!(entries[2].1, Some(Value::Integer(1)), "new active");
}

/// The re-parsed image of a canonical `users` changeset `UPDATE`.
pub fn assert_parsed_users_update(values: &[ChangesetUpdatePair<String, Vec<u8>>]) {
    assert_eq!(values.len(), 3, "column count in encoded bytes");
    assert_eq!(values[1].0, Some(Value::Text("Alice".to_string())));
    assert_eq!(values[1].1, Some(Value::Text("Alicia".to_string())));
}
