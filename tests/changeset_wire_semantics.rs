//! Changeset wire semantics integration tests.

#![cfg(feature = "testing")]

use sqlite_diff_rs::testing::{
    session_changeset_and_patchset, session_changeset_and_patchset_with_setup,
};
use sqlite_diff_rs::{
    ChangeSet, ChangesetFormat, ChangesetOp, ChangesetUpdatePairExt, DiffOps, ParseError,
    ParsedDiffSet, SchemaWithPK, TableSchema, Update, Value,
};

fn parsed_changeset(
    bytes: &[u8],
) -> sqlite_diff_rs::DiffSet<ChangesetFormat, TableSchema<String>, String, Vec<u8>> {
    let ParsedDiffSet::Changeset(changeset) = ParsedDiffSet::try_from(bytes).unwrap() else {
        panic!("expected changeset")
    };
    changeset
}

fn encoded_update(
    flags: Vec<u8>,
    configure: impl FnOnce(
        Update<TableSchema<String>, ChangesetFormat, String, Vec<u8>>,
    ) -> Update<TableSchema<String>, ChangesetFormat, String, Vec<u8>>,
) -> Vec<u8> {
    let schema = TableSchema::new("items".to_owned(), flags.len(), flags);
    let update = configure(Update::from(schema));
    ChangeSet::<TableSchema<String>, String, Vec<u8>>::new()
        .update(update)
        .build()
}

#[test]
fn primary_key_columns_follow_composite_key_order() {
    let (bytes, _) = session_changeset_and_patchset_with_setup(
        &[
            "CREATE TABLE items (col0 INTEGER NOT NULL, value TEXT, col2 INTEGER NOT NULL, PRIMARY KEY(col2, col0))",
            "INSERT INTO items VALUES (10, 'before', 20)",
        ],
        &["UPDATE items SET value = 'after' WHERE col0 = 10 AND col2 = 20"],
    );
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();

    // A boolean reading of these flags would answer column order [0, 2].
    assert_eq!(operation.table().pk_flags(), [2, 0, 1]);
    assert_eq!(
        operation
            .table()
            .primary_key_columns()
            .collect::<Vec<usize>>(),
        [2, 0]
    );
}

#[test]
fn primary_key_columns_support_single_key() {
    let (bytes, _) = session_changeset_and_patchset(&[
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT)",
        "INSERT INTO items VALUES (7, 'value')",
    ]);
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();

    assert_eq!(
        operation
            .table()
            .primary_key_columns()
            .collect::<Vec<usize>>(),
        [0]
    );
}

#[test]
fn sqlite_omits_tables_without_primary_keys() {
    let (bytes, _) = session_changeset_and_patchset(&[
        "CREATE TABLE items (id INTEGER, value TEXT)",
        "INSERT INTO items VALUES (7, 'value')",
    ]);

    assert_eq!(bytes, [] as [u8; 0]);
}

#[test]
fn live_update_reports_only_the_touched_column() {
    let (bytes, _) = session_changeset_and_patchset_with_setup(
        &[
            "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT, untouched TEXT)",
            "INSERT INTO items VALUES (1, 'before', 'same')",
        ],
        &["UPDATE items SET value = 'after' WHERE id = 1"],
    );
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();
    let ChangesetOp::Update { values, .. } = &operation else {
        panic!("expected update")
    };

    // The key column is old-only because the old image carries the row identity.
    assert_eq!(
        values
            .iter()
            .map(|(old, new)| (old.is_some(), new.is_some()))
            .collect::<Vec<(bool, bool)>>(),
        [(true, false), (true, true), (false, false)]
    );
    assert!(!values[0].is_changed());
    assert!(!values[2].is_changed());
    assert!(values[1].is_changed());
    assert_eq!(
        operation.changed_column_indices().collect::<Vec<usize>>(),
        [1]
    );
}

#[test]
fn equal_pair_is_unchanged_and_differing_pair_is_changed() {
    let bytes = encoded_update(vec![1, 0], |update| {
        update
            .set(0, 1_i64, 1_i64)
            .unwrap()
            .set(1, "before", "after")
            .unwrap()
    });
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();
    let ChangesetOp::Update { values, .. } = &operation else {
        panic!("expected update")
    };

    assert!(!values[0].is_changed());
    assert!(values[1].is_changed());
    assert_eq!(
        operation.changed_column_indices().collect::<Vec<usize>>(),
        [1]
    );
}

#[test]
fn new_only_pair_is_changed() {
    let bytes = encoded_update(vec![1, 0], |update| {
        update
            .set(0, 1_i64, 1_i64)
            .unwrap()
            .set_new(1, "after")
            .unwrap()
    });
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();
    let ChangesetOp::Update { values, .. } = &operation else {
        panic!("expected update")
    };

    assert_eq!(
        (values[1].0.is_some(), values[1].1.is_some()),
        (false, true)
    );
    assert!(values[1].is_changed());
    assert_eq!(
        operation.changed_column_indices().collect::<Vec<usize>>(),
        [1]
    );
}

/// SQLite's session writes every key column's old value in a changeset UPDATE,
/// so a record leaving one undefined names no row and is refused.
#[test]
fn update_without_old_key_is_refused() {
    let mut bytes = vec![b'T', 2, 1, 0, b't', 0, 0x17, 0];
    // Old record with the key and the value undefined.
    bytes.extend([0x00, 0x00]);
    // New record with key 7 and the value undefined.
    bytes.push(0x01);
    bytes.extend(7_i64.to_be_bytes());
    bytes.push(0x00);

    assert_eq!(
        ParsedDiffSet::try_from(bytes.as_slice()),
        Err(ParseError::UndefinedOldKey {
            table_name: "t".to_owned(),
            column: 0,
            position: 6,
        })
    );
}

#[test]
fn encoded_schema_roundtrip_preserves_primary_key_order() {
    let bytes = encoded_update(vec![2, 0, 1], |update| {
        update
            .set(0, 10_i64, 10_i64)
            .unwrap()
            .set(1, "before", "after")
            .unwrap()
            .set(2, 20_i64, 20_i64)
            .unwrap()
    });
    let changeset = parsed_changeset(&bytes);
    let operation = changeset.iter().next().unwrap();

    assert_eq!(
        operation
            .table()
            .primary_key_columns()
            .collect::<Vec<usize>>(),
        [2, 0]
    );
    assert_eq!(
        operation.primary_key(),
        [Value::Integer(20), Value::Integer(10)]
    );
}
