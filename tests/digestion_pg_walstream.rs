//! Tests for `pg_walstream` wire event digestion via `DiffSetBuilder::digest`.
//!
//! Exercises the `Digestable` impls on `EventType` for both
//! `ChangesetFormat` and `PatchsetFormat`, covering every operation kind,
//! error paths, and no-op events that should be silently ignored.

#![cfg(feature = "pg-walstream")]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

use sqlite_diff_rs::pg_walstream::{ColumnValue, ConversionError, EventType, PgWalstream, RowData};
use sqlite_diff_rs::{
    ChangeSet, ChangesetOp, DecodeError, DynTable, ParsedDiffSet, PatchSet, PatchsetOp, TypeMap,
    Value,
};

mod common;
use common::{
    TestUsersTable, assert_parsed_users_update, assert_users_patchset_update, assert_users_row,
    assert_users_update, source_scoped_test_schema, test_schema,
};

fn default_adapter() -> TypeMap<PgWalstream, String, Vec<u8>> {
    TypeMap::defaults()
}

fn row_data(id: i64, name: &str, active: bool) -> RowData {
    let mut data = RowData::new();
    data.push(Arc::from("id"), ColumnValue::text(&id.to_string()));
    data.push(Arc::from("name"), ColumnValue::text(name));
    data.push(
        Arc::from("active"),
        ColumnValue::text(if active { "t" } else { "f" }),
    );
    data
}

#[test]
fn pgoutput_uses_schema_for_lookup() {
    let schema = source_scoped_test_schema("public");
    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data: row_data(1, "Alice", true),
    };
    let patchset: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&event, &schema, &default_adapter())
        .unwrap();

    assert_eq!(patchset.iter().count(), 1);
}

// -- ChangesetFormat: Insert, Update, Delete --------------------------------

#[test]
fn pg_changeset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = row_data(1, "Alice", true);
    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data,
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_changeset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let old = row_data(1, "Alice", true);
    let new = row_data(1, "Alicia", true);
    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: Some(old),
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_changeset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = row_data(1, "Alice", true);
    let event = EventType::Delete {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: data,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };
    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_patchset_insert() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = row_data(1, "Alice", true);
    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data,
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_patchset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let old = row_data(1, "Alice", true);
    let new = row_data(1, "Alicia", true);
    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: Some(old),
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_patchset_delete() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = row_data(1, "Alice", true);
    let event = EventType::Delete {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: data,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };
    let ps: PatchSet<TestUsersTable, String, Vec<u8>> =
        PatchSet::new().digest(&event, &schema, &adapter).unwrap();
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
fn pg_table_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();
    let data = row_data(1, "Alice", true);

    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("nonexistent"),
        relation_oid: 1,
        data,
    };

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&event, &schema, &adapter);
    match result {
        Err(ConversionError::TableNotFound(n)) => assert_eq!(n, "nonexistent"),
        Err(other) => panic!("expected TableNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn pg_column_not_found_is_error() {
    let schema = test_schema();
    let adapter = default_adapter();
    let mut data = RowData::new();
    data.push(Arc::from("id"), ColumnValue::text("1"));
    // Use a column name that doesn't exist in the schema
    data.push(Arc::from("missing_col"), ColumnValue::text("val"));

    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data,
    };

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&event, &schema, &adapter);
    match result {
        Err(ConversionError::ColumnNotFound(n)) => assert!(n.contains("missing_col")),
        Err(other) => panic!("expected ColumnNotFound, got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn pg_decode_error_is_propagated() {
    // Use an empty TypeMap so every decode fails with NoDecoderForType.
    let adapter: TypeMap<PgWalstream, String, Vec<u8>> = TypeMap::new();
    let schema = test_schema();
    let data = row_data(1, "Alice", true);

    let event = EventType::Insert {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        data,
    };

    let result: Result<ChangeSet<TestUsersTable, String, Vec<u8>>, ConversionError> =
        ChangeSet::new().digest(&event, &schema, &adapter);
    match result {
        Err(ConversionError::Decode(DecodeError::NoDecoderForType { column })) => {
            assert_ne!(column, "");
        }
        Err(other) => panic!("expected Decode(NoDecoderForType), got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn pg_missing_old_data_still_works_for_changeset_update() {
    let schema = test_schema();
    let adapter = default_adapter();
    let new = row_data(1, "Alice", true);

    // pg_walstream Update events carry old_data as Option.
    // When it's None for a changeset, the builder falls through
    // to `set_new` for columns without old data — this is fine.
    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: None,
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
    let bytes: Vec<u8> = cs.build();
    assert!(
        !bytes.is_empty(),
        "changeset should produce output with None old_data"
    );
}

// -- No-op handling --------------------------------------------------------

#[test]
fn pg_update_with_no_old_data_patchset_still_works() {
    let schema = test_schema();
    let new = row_data(1, "Alice", true);

    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: None,
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };

    let ps: PatchSet<TestUsersTable, String, Vec<u8>> = PatchSet::new()
        .digest(&event, &schema, &default_adapter())
        .unwrap();
    let bytes: Vec<u8> = ps.build();
    assert!(
        !bytes.is_empty(),
        "patchset update with no old data must produce output"
    );
}

// -- Changeset UPDATE captures the old primary key -------------------------
//
// Under REPLICA IDENTITY DEFAULT a non-key update sends no old tuple, so
// `old_data` is None. The unchanged primary key must still be captured from
// the new tuple (the key did not change) so a changeset apply can build a
// WHERE clause.

#[test]
fn pg_changeset_update_captures_old_pk_when_old_data_absent() {
    let schema = test_schema();
    let adapter = default_adapter();
    let new = row_data(1, "Alicia", true);

    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: None,
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(
                values[0].0,
                Some(Value::Integer(1)),
                "old primary key must be captured from the new tuple when old_data is absent"
            );
            // A non-key column has no old value on the wire, so it stays set_new.
            assert_eq!(
                values[1].0, None,
                "non-key old absent under default identity"
            );
            assert_eq!(
                values[1].1,
                Some(Value::Text("Alicia".to_string())),
                "new name"
            );
        }
        other => panic!("expected update, got {other:?}"),
    }
}

#[test]
fn pg_changeset_update_captures_changed_pk() {
    // A primary-key change: the old tuple carries the old key.
    let schema = test_schema();
    let adapter = default_adapter();
    let old = row_data(1, "Alice", true);
    let new = row_data(2, "Alice", true);

    let event = EventType::Update {
        schema: Arc::from("public"),
        table: Arc::from("users"),
        relation_oid: 1,
        old_data: Some(old),
        new_data: new,
        replica_identity: pg_walstream::ReplicaIdentity::Default,
        key_columns: alloc::vec![Arc::from("id")],
    };

    let cs: ChangeSet<TestUsersTable, String, Vec<u8>> =
        ChangeSet::new().digest(&event, &schema, &adapter).unwrap();
    let ops: Vec<_> = cs.iter().collect();
    match &ops[0] {
        ChangesetOp::Update { values, .. } => {
            assert_eq!(values[0].0, Some(Value::Integer(1)), "old key");
            assert_eq!(values[0].1, Some(Value::Integer(2)), "new key");
        }
        other => panic!("expected update, got {other:?}"),
    }
}
