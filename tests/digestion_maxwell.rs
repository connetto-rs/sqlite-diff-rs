//! Tests for `maxwell` wire event digestion via `DiffSetBuilder::digest`.
//!
//! Exercises the `Digestable` impls on `Message` for both
//! `ChangesetFormat` and `PatchsetFormat`, covering every operation kind,
//! and error paths.

#![cfg(feature = "maxwell")]

extern crate alloc;

use alloc::vec::Vec;

use sqlite_diff_rs::maxwell::{
    ColumnDefinition, ControlMessage, ConversionError, DatabaseChange, DatabaseDefinition,
    DatabaseDropChange, DdlMetadata, Maxwell, Message, OpType, RowChange, TableAlterChange,
    TableCreateChange, TableDefinition, TableDropChange,
};
use sqlite_diff_rs::{
    ChangeSet, ChangesetOp, DecodeError, DynTable, ParsedDiffSet, PatchSet, PatchsetOp, TypeMap,
    Value,
};

mod common;
use common::{
    TestUsersTable, assert_parsed_users_update, assert_users_patchset_update, assert_users_row,
    assert_users_update, source_scoped_test_schema, test_schema,
};

fn default_adapter() -> TypeMap<Maxwell, String, Vec<u8>> {
    TypeMap::defaults()
}

fn data_map(id: i64, name: &str, active: bool) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    map.insert(
        "id".to_string(),
        serde_json::Value::Number(serde_json::Number::from(id)),
    );
    map.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    map.insert("active".to_string(), serde_json::Value::Bool(active));
    map
}

fn row_change(
    table: &str,
    data: serde_json::Map<String, serde_json::Value>,
    old: Option<serde_json::Map<String, serde_json::Value>>,
) -> RowChange {
    RowChange {
        database: "testdb".to_string(),
        table: table.to_string(),
        data,
        old,
        ..Default::default()
    }
}

fn message(
    op_type: OpType,
    data: serde_json::Map<String, serde_json::Value>,
    old: Option<serde_json::Map<String, serde_json::Value>>,
) -> Message {
    let row = row_change("users", data, old);
    match op_type {
        OpType::Insert => Message::Insert(row),
        OpType::Update => Message::Update(row),
        OpType::Delete => Message::Delete(row),
        OpType::BootstrapInsert => Message::BootstrapInsert(row),
        other => panic!("unhandled op type in test helper: {other:?}"),
    }
}

fn minimal_ddl_metadata() -> DdlMetadata {
    DdlMetadata {
        ts: 0,
        sql: String::new(),
        position: None,
        gtid: None,
        schema_id: None,
    }
}

fn minimal_table_definition(database: &str, table: &str) -> TableDefinition {
    TableDefinition {
        database: database.to_string(),
        table: table.to_string(),
        charset: None,
        primary_key: alloc::vec!["id".to_string()],
        columns: alloc::vec![ColumnDefinition {
            name: "id".to_string(),
            column_type: "int".to_string(),
            charset: None,
            signed: None,
            enum_values: None,
            column_length: None,
        }],
    }
}

#[test]
fn maxwell_uses_database_for_schema_lookup() {
    let schema = source_scoped_test_schema("testdb");
    let message = message(OpType::Insert, data_map(1, "Alice", true), None);
    let patchset: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&message, &schema, &default_adapter())
        .unwrap();

    assert_eq!(patchset.iter().count(), 1);
}

// -- ChangesetFormat: Insert, Update, Delete --------------------------------

#[test]
fn maxwell_changeset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(1, "Alice", true);
    let msg = message(OpType::Insert, data, None);

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
    // Parse the encoded bytes back to catch op-code, value-type, and column-count bugs.
    let parsed = ParsedDiffSet::parse(&bytes).expect("bytes must re-parse");
    let ParsedDiffSet::Changeset(parsed_cs) = parsed else {
        panic!("expected changeset marker in encoded bytes");
    };
    let parsed_ops: Vec<_> = parsed_cs.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        ChangesetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    // Oracle: our bytes must be bit-for-bit identical to what SQLite emits.
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
fn maxwell_changeset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let new_data = data_map(1, "Alicia", true);
    let old_data = data_map(1, "Alice", true);
    let msg = message(OpType::Update, new_data, Some(old_data));

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1, "one operation expected");
    match &ops[0] {
        ChangesetOp::Update { table, values, .. } => {
            assert_eq!(table.name(), "users");
            assert_eq!(values[0].1, Some(Value::Integer(1)), "new id (unchanged)");
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
fn maxwell_changeset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(1, "Alice", true);
    let msg = message(OpType::Delete, data, None);

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

// -- PatchsetFormat: Insert, Update, Delete ---------------------------------

#[test]
fn maxwell_patchset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(1, "Alice", true);
    let msg = message(OpType::Insert, data, None);

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
        panic!("expected patchset marker in encoded bytes");
    };
    let parsed_ops: Vec<_> = parsed_ps.iter().collect();
    assert_eq!(parsed_ops.len(), 1);
    match &parsed_ops[0] {
        PatchsetOp::Insert { values, .. } => {
            assert_users_row(values, "Alice");
        }
        other => panic!("expected Insert in parsed bytes, got {other:?}"),
    }
    // Oracle: our bytes must be bit-for-bit identical to what SQLite emits.
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
fn maxwell_patchset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let new_data = data_map(1, "Alicia", true);
    let msg = message(OpType::Update, new_data, None);

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
fn maxwell_patchset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(1, "Alice", true);
    let msg = message(OpType::Delete, data, None);

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

// -- Error paths -----------------------------------------------------------

#[test]
fn maxwell_table_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(1, "Alice", true);

    let msg = Message::Insert(row_change("nonexistent", data, None));

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::TableNotFound(n)) => assert_eq!(n, "nonexistent"),
        Err(other) => panic!("expected TableNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn maxwell_column_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();

    let mut data = serde_json::Map::new();
    data.insert(
        "missing_col".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1_i64)),
    );
    let msg = message(OpType::Insert, data, None);

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&msg, &schema, &adapter);
    match result {
        Err(ConversionError::ColumnNotFound(n)) => assert!(n.contains("missing_col")),
        Err(other) => panic!("expected ColumnNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn maxwell_decode_error_is_propagated() {
    let adapter: TypeMap<Maxwell, String, Vec<u8>> = TypeMap::new();
    let schema = test_schema();
    let data = data_map(1, "Alice", true);
    let msg = message(OpType::Insert, data, None);

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

#[test]
fn maxwell_changeset_update_without_old_is_ok() {
    // Maxwell updates carry `old` as optional. When it is absent, every column
    // is treated as unchanged (old equals new), since Maxwell lists changed
    // columns in `old`.
    let schema = test_schema();
    let adapter = default_adapter();
    let new_data = data_map(1, "Alice", true);
    let msg = message(OpType::Update, new_data, None);

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let bytes: Vec<u8> = cs.build();
    assert!(
        !bytes.is_empty(),
        "changeset must produce output without old data"
    );
}

// -- Changeset UPDATE captures the old primary key -------------------------
//
// Maxwell's `old` carries only the columns that changed, so the unchanged
// primary key of a non-key update is absent from it. The digest must still
// capture the old key (equal to the new key, since it did not change) so a
// changeset apply can build a WHERE clause.

#[test]
fn maxwell_changeset_update_captures_old_pk_when_old_omits_it() {
    let schema = test_schema();
    let adapter = default_adapter();
    let new_data = data_map(1, "Alicia", true);
    // Only the changed column is present in `old`, as Maxwell emits it.
    let mut old = serde_json::Map::new();
    old.insert(
        "name".to_string(),
        serde_json::Value::String("Alice".to_string()),
    );
    let msg = message(OpType::Update, new_data, Some(old));

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(
                values[0].0,
                Some(Value::Integer(1)),
                "old primary key must be captured even when absent from `old`"
            );
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
            // An unchanged column absent from `old` is captured as old == new.
            assert_eq!(
                values[2].0, values[2].1,
                "unchanged column captured as old == new"
            );
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn maxwell_changeset_update_captures_changed_pk() {
    // A primary-key change: Maxwell includes the changed key in `old`.
    let schema = test_schema();
    let adapter = default_adapter();
    let new_data = data_map(2, "Alice", true);
    let mut old = serde_json::Map::new();
    old.insert(
        "id".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1_i64)),
    );
    let msg = message(OpType::Update, new_data, Some(old));

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old key");
            assert_eq!(values[0].1, Some(Value::Integer(2)), "new key");
        }
        other => panic!("expected update, got {other:?}"),
    }
}

// -- Contract: BootstrapInsert digests as Insert ---------------------------

#[test]
fn bootstrap_insert_changeset_matches_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(42, "Eve", false);

    let insert_msg = message(OpType::Insert, data.clone(), None);
    let bootstrap_msg = message(OpType::BootstrapInsert, data, None);

    let insert_bytes: Vec<u8> = ChangeSet::new()
        .digest(&insert_msg, &schema, &adapter)
        .unwrap()
        .build();
    let bootstrap_bytes: Vec<u8> = ChangeSet::new()
        .digest(&bootstrap_msg, &schema, &adapter)
        .unwrap()
        .build();

    assert_eq!(
        insert_bytes, bootstrap_bytes,
        "BootstrapInsert changeset must be identical to Insert on the same row"
    );
}

#[test]
fn bootstrap_insert_patchset_matches_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = data_map(42, "Eve", false);

    let insert_msg = message(OpType::Insert, data.clone(), None);
    let bootstrap_msg = message(OpType::BootstrapInsert, data, None);

    let insert_bytes: Vec<u8> = PatchSet::new()
        .digest(&insert_msg, &schema, &adapter)
        .unwrap()
        .build();
    let bootstrap_bytes: Vec<u8> = PatchSet::new()
        .digest(&bootstrap_msg, &schema, &adapter)
        .unwrap()
        .build();

    assert_eq!(
        insert_bytes, bootstrap_bytes,
        "BootstrapInsert patchset must be identical to Insert on the same row"
    );
}

// -- Contract: non-row variants return the builder unchanged ---------------

#[test]
fn bootstrap_start_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::BootstrapStart(ControlMessage {
        database: "testdb".to_string(),
        table: "users".to_string(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "BootstrapStart must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "BootstrapStart must not add any ops");
}

#[test]
fn bootstrap_complete_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::BootstrapComplete(ControlMessage {
        database: "testdb".to_string(),
        table: "users".to_string(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(
        cs.iter().count(),
        0,
        "BootstrapComplete must not add any ops"
    );
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(
        ps.iter().count(),
        0,
        "BootstrapComplete must not add any ops"
    );
}

#[test]
fn table_create_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::TableCreate(TableCreateChange {
        database: "testdb".to_string(),
        table: "users".to_string(),
        definition: minimal_table_definition("testdb", "users"),
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "TableCreate must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "TableCreate must not add any ops");
}

#[test]
fn table_alter_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::TableAlter(TableAlterChange {
        database: "testdb".to_string(),
        table: "users".to_string(),
        old_definition: minimal_table_definition("testdb", "users"),
        definition: minimal_table_definition("testdb", "users"),
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "TableAlter must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "TableAlter must not add any ops");
}

#[test]
fn table_drop_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::TableDrop(TableDropChange {
        database: "testdb".to_string(),
        table: "users".to_string(),
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "TableDrop must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "TableDrop must not add any ops");
}

#[test]
fn database_create_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::DatabaseCreate(DatabaseChange {
        definition: DatabaseDefinition {
            database: "testdb".to_string(),
            charset: None,
        },
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "DatabaseCreate must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "DatabaseCreate must not add any ops");
}

#[test]
fn database_alter_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::DatabaseAlter(DatabaseChange {
        definition: DatabaseDefinition {
            database: "testdb".to_string(),
            charset: None,
        },
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "DatabaseAlter must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "DatabaseAlter must not add any ops");
}

#[test]
fn database_drop_leaves_builder_unchanged() {
    let schema = test_schema();
    let adapter = default_adapter();
    let msg = Message::DatabaseDrop(DatabaseDropChange {
        database: "testdb".to_string(),
        metadata: minimal_ddl_metadata(),
        ..Default::default()
    });
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(cs.iter().count(), 0, "DatabaseDrop must not add any ops");
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&msg, &schema, &adapter).unwrap();
    assert_eq!(ps.iter().count(), 0, "DatabaseDrop must not add any ops");
}
