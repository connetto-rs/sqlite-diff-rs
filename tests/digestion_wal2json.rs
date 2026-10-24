//! Tests for `wal2json` wire event digestion via `DiffSetBuilder::digest`.
//!
//! Exercises the `Digestable` impls on `MessageV2` and `ChangeV1` for both
//! `ChangesetFormat` and `PatchsetFormat`, covering every operation kind,
//! error paths, and no-op actions (B, C, T, M) that should be ignored.

#![cfg(feature = "wal2json")]

extern crate alloc;

use alloc::vec::Vec;

use sqlite_diff_rs::wal2json::{
    Action, ChangeV1, Column, ColumnArrays, ConversionError, LogicalMessageV2, MessageV2, OldKeys,
    RowV2, TransactionBoundary, TruncateV2, Wal2Json, parse_v1, parse_v2,
};
use sqlite_diff_rs::{
    ChangeSet, ChangesetOp, DecodeError, DynTable, ParsedDiffSet, PatchSet, PatchsetOp,
    SimpleTable, TypeMap, Value, WireSchema,
};

mod common;
use common::{
    TestUsersTable, assert_parsed_users_update, assert_users_patchset_update, assert_users_row,
    assert_users_update, source_scoped_test_schema, test_schema,
};

fn default_adapter() -> TypeMap<Wal2Json, String, Vec<u8>> {
    TypeMap::defaults()
}

struct DuplicateNameSchema {
    public: TestUsersTable,
    private: TestUsersTable,
}

impl WireSchema for DuplicateNameSchema {
    type Table = TestUsersTable;

    fn get(&self, source_schema: Option<&str>, table_name: &str) -> Option<&Self::Table> {
        match (source_schema, table_name) {
            (Some("public"), "users") => Some(&self.public),
            (Some("private"), "users") => Some(&self.private),
            _ => None,
        }
    }
}

fn duplicate_name_schema() -> DuplicateNameSchema {
    DuplicateNameSchema {
        public: TestUsersTable(SimpleTable::with_rowid_alias(
            "public.users",
            &[("id", "INTEGER"), ("name", "TEXT"), ("active", "INTEGER")],
            0,
        )),
        private: TestUsersTable(SimpleTable::with_rowid_alias(
            "private.users",
            &[("id", "INTEGER"), ("other", "TEXT"), ("active", "INTEGER")],
            0,
        )),
    }
}

fn column(name: &str, type_name: &str, value: serde_json::Value) -> Column {
    let mut column = Column::new(name);
    column.type_name = Some(type_name.to_string());
    column.value = Some(value);
    column
}

/// `RowV2` is `#[non_exhaustive]`, so a row is built through its constructor.
fn v2_row(
    action: Action,
    table: &str,
    columns: Option<Vec<Column>>,
    identity: Option<Vec<Column>>,
    lsn: Option<&str>,
) -> MessageV2 {
    let mut row = RowV2::new(table);
    row.schema = Some("public".to_string());
    row.columns = columns;
    row.identity = identity;
    row.lsn = lsn.map(str::to_string);
    match action {
        Action::Insert => MessageV2::Insert(row),
        Action::Update => MessageV2::Update(row),
        Action::Delete => MessageV2::Delete(row),
        other => panic!("{other:?} is not a row action"),
    }
}

fn v1_arrays(names: &[&str], types: &[&str], values: Vec<serde_json::Value>) -> ColumnArrays {
    let mut arrays = ColumnArrays::new(names.iter().map(|name| (*name).to_string()).zip(values));
    arrays.columntypes = Some(types.iter().map(|t| (*t).to_string()).collect());
    arrays
}

fn v1_oldkeys(names: &[&str], types: &[&str], values: Vec<serde_json::Value>) -> OldKeys {
    let mut keys = OldKeys::new(names.iter().map(|name| (*name).to_string()).zip(values));
    keys.keytypes = Some(types.iter().map(|t| (*t).to_string()).collect());
    keys
}

fn int_col(name: &str, val: i64) -> Column {
    column(
        name,
        "integer",
        serde_json::Value::Number(serde_json::Number::from(val)),
    )
}

fn text_col(name: &str, val: &str) -> Column {
    column(name, "text", serde_json::Value::String(val.to_string()))
}

fn bool_col(name: &str, val: bool) -> Column {
    column(name, "boolean", serde_json::Value::Bool(val))
}

fn all_columns(id: i64, name: &str, active: bool) -> Vec<Column> {
    alloc::vec![
        int_col("id", id),
        text_col("name", name),
        bool_col("active", active),
    ]
}

fn all_values(id: i64, name: &str, active: bool) -> Vec<serde_json::Value> {
    alloc::vec![
        serde_json::Value::Number(serde_json::Number::from(id)),
        serde_json::Value::String(name.to_string()),
        serde_json::Value::Bool(active),
    ]
}

#[test]
fn w2j_v2_uses_source_schema_for_equal_table_names() {
    let schema = duplicate_name_schema();
    let msg = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );
    let patchset: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&msg, &schema, &default_adapter())
        .unwrap();
    let operations: Vec<_> = patchset.iter().collect();

    match operations.as_slice() {
        [PatchsetOp::Insert { table, .. }] => assert_eq!(*table, &schema.public),
        other => panic!("expected one public.users insert, got {other:?}"),
    }
}

#[test]
fn w2j_v1_uses_source_schema_for_lookup() {
    let schema = source_scoped_test_schema("public");
    let change = ChangeV1::Insert {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alice", true),
        ),
        pk: None,
    };
    let patchset: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&change, &schema, &default_adapter())
        .unwrap();

    assert_eq!(patchset.iter().count(), 1);
}

// -- MessageV2: ChangesetFormat --------------------------------------------

#[test]
fn w2j_v2_changeset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Insert { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    #[cfg(feature = "testing")]
    {
        let (oracle, _) = sqlite_diff_rs::testing::session_changeset_and_patchset(&[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
            "INSERT INTO users (id, name, active) VALUES (1, 'Alice', 1)",
        ]);
        assert_eq!(bytes, oracle, "changeset bytes must match SQLite");
    }
}

#[test]
fn w2j_v2_changeset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Update,
        "users",
        Some(all_columns(1, "Alicia", true)),
        Some(all_columns(1, "Alice", true)),
        None,
    );
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Update { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_users_update(values);
        }
        other => panic!("expected Update, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_parsed_users_update(values);
        }
        other => panic!("expected Update in parsed bytes, got {other:?}"),
    }
}

#[test]
fn w2j_v2_changeset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Delete,
        "users",
        None,
        Some(all_columns(1, "Alice", true)),
        None,
    );
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Delete {
            table, old_values, ..
        } => {
            assert_eq!(table.name(), "users");
            assert_users_row(old_values, "Alice");
        }
        other => panic!("expected Delete, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Delete { old_values, .. } => {
            assert_users_row(old_values, "Alice");
        }
        other => panic!("expected Delete in parsed bytes, got {other:?}"),
    }
}

// -- MessageV2: PatchsetFormat ---------------------------------------------

#[test]
fn w2j_v2_patchset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Insert { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    #[cfg(feature = "testing")]
    {
        let (_, oracle) = sqlite_diff_rs::testing::session_changeset_and_patchset(&[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
            "INSERT INTO users (id, name, active) VALUES (1, 'Alice', 1)",
        ]);
        assert_eq!(bytes, oracle, "patchset bytes must match SQLite");
    }
}

#[test]
fn w2j_v2_patchset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Update,
        "users",
        Some(all_columns(1, "Alicia", true)),
        Some(all_columns(1, "Alice", true)),
        None,
    );
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Update {
            table, pk, entries, ..
        } => {
            assert_eq!(table.name(), "users");
            assert_users_patchset_update(pk, entries);
        }
        other => panic!("expected Update, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Update { pk, entries, .. } => {
            assert_users_patchset_update(pk, entries);
        }
        other => panic!("expected Update in parsed bytes, got {other:?}"),
    }
}

#[test]
fn w2j_v2_patchset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = v2_row(
        Action::Delete,
        "users",
        None,
        Some(all_columns(1, "Alice", true)),
        None,
    );
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Delete { table, pk, .. } => {
            assert_eq!(table.name(), "users");
            assert_eq!(pk, &[Value::Integer(1)], "primary key of deleted row");
        }
        other => panic!("expected Delete, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Delete { pk, .. } => {
            assert_eq!(pk, &[Value::Integer(1)], "primary key in encoded bytes");
        }
        other => panic!("expected Delete in parsed bytes, got {other:?}"),
    }
}

// -- ChangeV1: ChangesetFormat ---------------------------------------------

#[test]
fn w2j_v1_changeset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Insert {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alice", true),
        ),
        pk: None,
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Insert { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    #[cfg(feature = "testing")]
    {
        let (oracle, _) = sqlite_diff_rs::testing::session_changeset_and_patchset(&[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
            "INSERT INTO users (id, name, active) VALUES (1, 'Alice', 1)",
        ]);
        assert_eq!(bytes, oracle, "changeset bytes must match SQLite");
    }
}

#[test]
fn w2j_v1_changeset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Update {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alicia", true),
        ),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Update { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_eq!(values.len(), 3, "three columns");
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old id from oldkeys");
            assert_eq!(values[0].1, Some(Value::Integer(1)), "new id");
            assert_eq!(values[1].0, None, "name absent from oldkeys");
            assert_eq!(
                values[1].1,
                Some(Value::Text("Alicia".to_string())),
                "new name"
            );
        }
        other => panic!("expected Update, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values.len(), 3, "column count in encoded bytes");
            assert_eq!(values[1].1, Some(Value::Text("Alicia".to_string())));
        }
        other => panic!("expected Update in parsed bytes, got {other:?}"),
    }
}

#[test]
fn w2j_v1_changeset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Delete {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Delete {
            table, old_values, ..
        } => {
            assert_eq!(table.name(), "users");
            assert_eq!(old_values.len(), 3, "three columns");
            // Only id is in oldkeys; name and active default to Null.
            assert_eq!(old_values[0], Value::Integer(1), "id from oldkeys");
            assert_eq!(old_values[1], Value::Null, "name absent from oldkeys");
            assert_eq!(old_values[2], Value::Null, "active absent from oldkeys");
        }
        other => panic!("expected Delete, got {other:?}"),
    }
    let bytes: Vec<u8> = cs.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Delete { old_values, .. } => {
            assert_eq!(old_values.len(), 3, "column count in encoded bytes");
            assert_eq!(old_values[0], Value::Integer(1));
        }
        other => panic!("expected Delete in parsed bytes, got {other:?}"),
    }
}

// -- ChangeV1: PatchsetFormat ----------------------------------------------

#[test]
fn w2j_v1_patchset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Insert {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alice", true),
        ),
        pk: None,
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Insert { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    #[cfg(feature = "testing")]
    {
        let (_, oracle) = sqlite_diff_rs::testing::session_changeset_and_patchset(&[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
            "INSERT INTO users (id, name, active) VALUES (1, 'Alice', 1)",
        ]);
        assert_eq!(bytes, oracle, "patchset bytes must match SQLite");
    }
}

#[test]
fn w2j_v1_patchset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Update {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alicia", true),
        ),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Update {
            table, pk, entries, ..
        } => {
            assert_eq!(table.name(), "users");
            assert_users_patchset_update(pk, entries);
        }
        other => panic!("expected Update, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Update { pk, entries, .. } => {
            assert_users_patchset_update(pk, entries);
        }
        other => panic!("expected Update in parsed bytes, got {other:?}"),
    }
}

#[test]
fn w2j_v1_patchset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let change = ChangeV1::Delete {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = ps.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        PatchsetOp::Delete { table, pk, .. } => {
            assert_eq!(table.name(), "users");
            assert_eq!(pk, &[Value::Integer(1)], "primary key of deleted row");
        }
        other => panic!("expected Delete, got {other:?}"),
    }
    let bytes: Vec<u8> = ps.build();
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Patchset(parsed_ps) = parsed else {
        panic!("expected patchset marker");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Delete { pk, .. } => {
            assert_eq!(pk, &[Value::Integer(1)], "primary key in encoded bytes");
        }
        other => panic!("expected Delete in parsed bytes, got {other:?}"),
    }
}

// -- Error paths -----------------------------------------------------------

#[test]
fn w2j_table_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();

    let msg = v2_row(
        Action::Insert,
        "nonexistent",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::TableNotFound(n)) => assert_eq!(n, "nonexistent"),
        Err(other) => panic!("expected TableNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn w2j_missing_columns_is_error_for_insert() {
    let schema = test_schema();
    let adapter = default_adapter();

    let msg = v2_row(Action::Insert, "users", None, None, None);

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::MissingColumns) => {}
        Err(other) => panic!("expected MissingColumns, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn w2j_missing_identity_is_error_for_delete() {
    let schema = test_schema();
    let adapter = default_adapter();

    let msg = v2_row(Action::Delete, "users", None, None, None);

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::MissingColumns) => {}
        Err(other) => panic!("expected MissingColumns, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn w2j_column_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();

    let msg = v2_row(
        Action::Insert,
        "users",
        Some(alloc::vec![column(
            "missing_col",
            "integer",
            serde_json::Value::Number(serde_json::Number::from(1_i64))
        )]),
        None,
        None,
    );

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::ColumnNotFound(n)) => assert!(n.contains("missing_col")),
        Err(other) => panic!("expected ColumnNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn w2j_decode_error_is_propagated() {
    let adapter: TypeMap<Wal2Json, String, Vec<u8>> = TypeMap::new();
    let schema = test_schema();

    let msg = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::Decode(DecodeError::NoDecoderForType { column })) => {
            assert_ne!(column, "");
        }
        Err(other) => panic!("expected Decode(NoDecoderForType), got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

/// A v1 delete carries its old keys or it does not parse, so the builder can no longer be handed
/// one without them. This replaces a test that asserted a `MissingColumns` conversion error.
#[test]
fn w2j_v1_delete_without_oldkeys_is_rejected_when_parsed() {
    let json = r#"{"change":[{"kind":"delete","schema":"public","table":"users"}]}"#;

    assert!(
        parse_v1(json).is_err(),
        "a delete without oldkeys must not parse"
    );
}

/// A row action without a table is refused at the parse boundary now, so digestion never sees one.
/// This replaces a test that built such a message directly, which the typed model cannot express.
#[test]
fn w2j_v2_row_without_table_is_rejected_when_parsed() {
    let json =
        r#"{"action":"I","schema":"public","columns":[{"name":"id","type":"integer","value":1}]}"#;

    assert!(
        parse_v2(json).is_err(),
        "a row action without a table must not parse"
    );
}

#[test]
fn w2j_v2_non_row_actions_are_ignored() {
    let schema = test_schema();
    let adapter = default_adapter();

    let messages = [
        MessageV2::Begin(TransactionBoundary::new()),
        MessageV2::Commit(TransactionBoundary::new()),
        MessageV2::Truncate(TruncateV2::new("users")),
        MessageV2::Message(LogicalMessageV2::new(true, "prefix", "content")),
    ];

    for msg in messages {
        let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
            ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
        assert!(
            cs.build().is_empty(),
            "non-row action {:?} must be ignored",
            msg.action()
        );
    }
}

/// The kinds the model does not know are refused at the parse boundary, so digestion never sees
/// one. This replaces a test that fed an "unknown" kind straight to the builder, which the typed
/// model can no longer express.
#[test]
fn w2j_v1_unknown_kind_is_rejected_when_parsed() {
    let json = r#"{"change":[{"kind":"unknown","schema":"public","table":"users"}]}"#;

    assert!(
        parse_v1(json).is_err(),
        "an unknown kind must not parse into a change"
    );
}

// -- MessageV2 lsn field ---------------------------------------------------

#[test]
fn w2j_v2_lsn_present_parses() {
    let json = r#"{"action":"I","schema":"public","table":"users","lsn":"0/16B2270","columns":[{"name":"id","type":"integer","value":1}]}"#;
    let MessageV2::Insert(row) = parse_v2(json).unwrap() else {
        panic!("expected an insert");
    };
    assert_eq!(row.lsn.as_deref(), Some("0/16B2270"));
}

#[test]
fn w2j_v2_lsn_absent_defaults_none() {
    let json = r#"{"action":"I","schema":"public","table":"users","columns":[{"name":"id","type":"integer","value":1}]}"#;
    let MessageV2::Insert(row) = parse_v2(json).unwrap() else {
        panic!("expected an insert");
    };
    assert_eq!(row.lsn, None);
}

#[test]
fn w2j_v2_lsn_does_not_affect_digest() {
    let schema = test_schema();
    let adapter = default_adapter();

    let without = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        None,
    );
    let with = v2_row(
        Action::Insert,
        "users",
        Some(all_columns(1, "Alice", true)),
        None,
        Some("0/16B2270"),
    );

    let cs_without: ChangeSet<TestUsersTable, String, Vec<u8>> = ChangeSet::new()
        .digest(&without, &schema, &adapter)
        .unwrap();
    let cs_with: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&with, &schema, &adapter).unwrap();
    assert_eq!(
        cs_without.build(),
        cs_with.build(),
        "changeset output must be identical regardless of lsn"
    );

    let ps_without: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&without, &schema, &adapter).unwrap();
    let ps_with: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&with, &schema, &adapter).unwrap();
    assert_eq!(
        ps_without.build(),
        ps_with.build(),
        "patchset output must be identical regardless of lsn"
    );
}

// -- Changeset UPDATE captures the old-row image ---------------------------
//
// The changeset format stores old and new per column, so the digest must read
// the wal2json old image (v2 `identity`, v1 `oldkeys`), not only the new
// values. A primary-key change depends on the old key reaching the WHERE
// clause.

#[test]
fn w2j_v2_changeset_update_captures_old_pk_on_key_change() {
    let schema = test_schema();
    let adapter = default_adapter();

    // id changes 1 -> 2, identity carries the full old row.
    let msg = v2_row(
        Action::Update,
        "users",
        Some(all_columns(2, "Alice", true)),
        Some(all_columns(1, "Alice", true)),
        None,
    );

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(
                values[0].0,
                Some(Value::Integer(1)),
                "old key must be captured"
            );
            assert_eq!(
                values[0].1,
                Some(Value::Integer(2)),
                "new key must be present"
            );
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn w2j_v2_changeset_update_captures_full_old_image() {
    let schema = test_schema();
    let adapter = default_adapter();

    // name changes, identity is the full old row (REPLICA IDENTITY FULL).
    let msg = v2_row(
        Action::Update,
        "users",
        Some(all_columns(1, "Alicia", true)),
        Some(all_columns(1, "Alice", true)),
        None,
    );

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old id captured");
            assert_eq!(
                values[1].0,
                Some(Value::Text("Alice".to_string())),
                "old name captured"
            );
            assert_eq!(
                values[1].1,
                Some(Value::Text("Alicia".to_string())),
                "new name present"
            );
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn w2j_v2_changeset_update_default_identity_captures_pk_only() {
    let schema = test_schema();
    let adapter = default_adapter();

    // name changes, identity carries only the primary key (default identity).
    let msg = v2_row(
        Action::Update,
        "users",
        Some(all_columns(1, "Alicia", true)),
        Some(alloc::vec![int_col("id", 1)]),
        None,
    );

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            // Old primary key captured so the WHERE predicate can render.
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old pk captured");
            // Non-key column absent from identity: old stays None, new set.
            assert_eq!(
                values[1].0, None,
                "non-key old absent under default identity"
            );
            assert_eq!(values[1].1, Some(Value::Text("Alicia".to_string())));
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn w2j_v1_changeset_update_captures_old_pk_from_oldkeys() {
    let schema = test_schema();
    let adapter = default_adapter();

    // id changes 1 -> 2, oldkeys carries the old primary key.
    let change = ChangeV1::Update {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(2, "Alice", true),
        ),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old key from oldkeys");
            assert_eq!(values[0].1, Some(Value::Integer(2)), "new key present");
            assert_eq!(values[1].0, None, "non-key old absent from oldkeys");
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn w2j_v1_changeset_update_non_key_captures_old_pk() {
    // A non-key update: oldkeys carries only the PK, name changes.
    let schema = test_schema();
    let adapter = default_adapter();

    let change = ChangeV1::Update {
        schema: Some("public".to_string()),
        table: "users".to_string(),
        columns: v1_arrays(
            &["id", "name", "active"],
            &["integer", "text", "boolean"],
            all_values(1, "Alicia", true),
        ),
        pk: None,
        oldkeys: v1_oldkeys(
            &["id"],
            &["integer"],
            alloc::vec![serde_json::Value::Number(serde_json::Number::from(1_i64))],
        ),
    };

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&change, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old pk from oldkeys");
            assert_eq!(values[1].0, None, "non-key old absent from oldkeys");
            assert_eq!(values[1].1, Some(Value::Text("Alicia".to_string())));
        }
        other => panic!("expected update, got {other:?}"),
    }
}
