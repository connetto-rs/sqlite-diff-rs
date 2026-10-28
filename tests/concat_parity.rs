//! Consolidation parity with `sqlite3changeset_concat` and `sqlite3changeset_invert`.
//!
//! Each statement is recorded by its own session. Folding those diff sets with `|` must give
//! the bytes SQLite's concatenation gives, for changesets and patchsets alike.

#![cfg(feature = "testing")]

use rusqlite::session::{concat_strm, invert_strm};
use sqlite_diff_rs::testing::session_changeset_and_patchset_with_setup;
use sqlite_diff_rs::{
    ChangeDelete, ChangeSet, ChangesetFormat, DiffOps, DiffSetBuilder, ParsedDiffSet, Reverse,
    SimpleTable, Update,
};

const ITEMS: &str = "CREATE TABLE items (id INTEGER PRIMARY KEY, a INTEGER, b INTEGER)";

/// The changeset and patchset each statement records on its own, in order.
fn per_statement(setup: &[&str], statements: &[&str]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut applied: Vec<&str> = setup.to_vec();
    statements
        .iter()
        .map(|&statement| {
            let recorded = session_changeset_and_patchset_with_setup(&applied, &[statement]);
            applied.push(statement);
            recorded
        })
        .collect()
}

fn sqlite_concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().fold(Vec::new(), |acc, part| {
        let mut out = Vec::new();
        concat_strm(&mut acc.as_slice(), &mut &part[..], &mut out).unwrap();
        out
    })
}

fn builder_concat(parts: &[&[u8]]) -> Vec<u8> {
    let mut parsed = parts.iter().map(|part| ParsedDiffSet::parse(part).unwrap());
    match parsed.next().unwrap() {
        ParsedDiffSet::Changeset(first) => parsed
            .fold(DiffSetBuilder::from(first), |acc, next| {
                let ParsedDiffSet::Changeset(next) = next else {
                    unreachable!()
                };
                acc | DiffSetBuilder::from(next)
            })
            .build(),
        ParsedDiffSet::Patchset(first) => parsed
            .fold(DiffSetBuilder::from(first), |acc, next| {
                let ParsedDiffSet::Patchset(next) = next else {
                    unreachable!()
                };
                acc | DiffSetBuilder::from(next)
            })
            .build(),
    }
}

fn assert_concat_parity(setup: &[&str], statements: &[&str]) {
    let recorded = per_statement(setup, statements);
    let changesets: Vec<&[u8]> = recorded.iter().map(|(c, _)| c.as_slice()).collect();
    let patchsets: Vec<&[u8]> = recorded.iter().map(|(_, p)| p.as_slice()).collect();
    for (format, parts) in [("changeset", changesets), ("patchset", patchsets)] {
        let expected = sqlite_concat(&parts);
        let actual = builder_concat(&parts);
        assert_eq!(
            ParsedDiffSet::parse(&actual).unwrap(),
            ParsedDiffSet::parse(&expected).unwrap(),
            "{format} of {statements:?}"
        );
        assert_eq!(actual, expected, "{format} bytes of {statements:?}");
    }
}

const ONE_ROW: &[&str] = &[ITEMS, "INSERT INTO items VALUES (1, 1, 1)"];

#[test]
fn update_then_update_of_another_column() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "UPDATE items SET a = 2 WHERE id = 1",
            "UPDATE items SET b = 2 WHERE id = 1",
        ],
    );
}

#[test]
fn update_then_update_of_the_same_column() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "UPDATE items SET a = 2 WHERE id = 1",
            "UPDATE items SET a = 3, b = 3 WHERE id = 1",
        ],
    );
}

#[test]
fn update_and_back_cancels() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "UPDATE items SET a = 2, b = 2 WHERE id = 1",
            "UPDATE items SET a = 1 WHERE id = 1",
            "UPDATE items SET b = 1 WHERE id = 1",
        ],
    );
}

#[test]
fn update_then_delete_keeps_untouched_old_values() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "UPDATE items SET a = 2 WHERE id = 1",
            "DELETE FROM items WHERE id = 1",
        ],
    );
}

#[test]
fn delete_then_insert_of_another_row_keeps_changed_columns() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "DELETE FROM items WHERE id = 1",
            "INSERT INTO items VALUES (1, 1, 5)",
        ],
    );
}

#[test]
fn delete_then_insert_of_the_same_row_cancels() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "DELETE FROM items WHERE id = 1",
            "INSERT INTO items VALUES (1, 1, 1)",
        ],
    );
}

#[test]
fn delete_insert_update_delete_chain() {
    assert_concat_parity(
        ONE_ROW,
        &[
            "DELETE FROM items WHERE id = 1",
            "INSERT INTO items VALUES (1, 4, 1)",
            "UPDATE items SET b = 6 WHERE id = 1",
            "DELETE FROM items WHERE id = 1",
        ],
    );
}

#[test]
fn composite_key_after_the_first_column() {
    assert_concat_parity(
        &[
            "CREATE TABLE t (v TEXT, k1 INTEGER, w REAL, k2 TEXT, PRIMARY KEY (k2, k1))",
            "INSERT INTO t VALUES ('x', 1, 0.5, 'a')",
            "INSERT INTO t VALUES ('y', 2, 1.5, 'b')",
        ],
        &[
            "UPDATE t SET v = 'z' WHERE k1 = 1",
            "UPDATE t SET w = 2.5 WHERE k1 = 2",
            "UPDATE t SET w = -0.0 WHERE k1 = 1",
            "DELETE FROM t WHERE k1 = 2",
            "UPDATE t SET v = 'x' WHERE k1 = 1",
        ],
    );
}

type ItemUpdate = Update<SimpleTable, ChangesetFormat, String, Vec<u8>>;

fn items() -> SimpleTable {
    SimpleTable::with_rowid_alias(
        "items",
        &[("id", "INTEGER"), ("a", "INTEGER"), ("b", "INTEGER")],
        0,
    )
}

#[test]
fn set_old_records_an_update_as_sqlite_does() {
    let update = ItemUpdate::from(items())
        .set_old(0, 1i64)
        .unwrap()
        .set(1, 1i64, 2i64)
        .unwrap();
    let built = ChangeSet::<_, String, Vec<u8>>::new()
        .update(update)
        .build();
    let (expected, _) = session_changeset_and_patchset_with_setup(
        ONE_ROW,
        &["UPDATE items SET a = 2 WHERE id = 1"],
    );
    assert_eq!(built, expected);
}

#[test]
fn merged_updates_keep_a_key_change() {
    let rename = ItemUpdate::from(items())
        .set(0, 1i64, 1i64)
        .unwrap()
        .set(1, 1i64, 2i64)
        .unwrap();
    let rekey = ItemUpdate::from(items()).set(0, 1i64, 7i64).unwrap();
    let merged = ChangeSet::<_, String, Vec<u8>>::new()
        .update(rename)
        .update(rekey);
    let expected = ChangeSet::<_, String, Vec<u8>>::new().update(
        ItemUpdate::from(items())
            .set(0, 1i64, 7i64)
            .unwrap()
            .set(1, 1i64, 2i64)
            .unwrap(),
    );
    assert_eq!(merged, expected);
}

#[test]
fn merged_key_change_alone_is_kept() {
    let touch = ItemUpdate::from(items())
        .set(0, 1i64, 1i64)
        .unwrap()
        .set(1, 1i64, 2i64)
        .unwrap();
    let rekey_and_back = ItemUpdate::from(items())
        .set(0, 1i64, 7i64)
        .unwrap()
        .set(1, 2i64, 1i64)
        .unwrap();
    let merged = ChangeSet::<_, String, Vec<u8>>::new()
        .update(touch)
        .update(rekey_and_back);
    let expected = ChangeSet::<_, String, Vec<u8>>::new()
        .update(ItemUpdate::from(items()).set(0, 1i64, 7i64).unwrap());
    assert_eq!(merged, expected);
}

#[test]
fn update_then_delete_built_by_hand() {
    let update = ItemUpdate::from(items())
        .set_old(0, 1i64)
        .unwrap()
        .set(1, 1i64, 2i64)
        .unwrap();
    let delete = ChangeDelete::from(items())
        .set(0, 1i64)
        .unwrap()
        .set(1, 2i64)
        .unwrap()
        .set(2, 7i64)
        .unwrap();
    let merged = ChangeSet::<_, String, Vec<u8>>::new()
        .update(update)
        .delete(delete);
    let expected = ChangeSet::<_, String, Vec<u8>>::new().delete(
        ChangeDelete::from(items())
            .set(0, 1i64)
            .unwrap()
            .set(1, 1i64)
            .unwrap()
            .set(2, 7i64)
            .unwrap(),
    );
    assert_eq!(merged, expected);
}

fn sqlite_invert(changeset: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    invert_strm(&mut &changeset[..], &mut out).unwrap();
    out
}

#[test]
fn reverse_keeps_the_key_in_the_old_record() {
    let recorded = per_statement(
        ONE_ROW,
        &[
            "UPDATE items SET a = 2 WHERE id = 1",
            "UPDATE items SET b = 3 WHERE id = 1",
        ],
    );
    let parts: Vec<&[u8]> = recorded.iter().map(|(c, _)| c.as_slice()).collect();
    for changeset in [parts[0].to_vec(), sqlite_concat(&parts)] {
        let ParsedDiffSet::Changeset(parsed) = ParsedDiffSet::parse(&changeset).unwrap() else {
            unreachable!()
        };
        let expected = sqlite_invert(&changeset);
        let in_order: Vec<u8> = parsed.clone().reverse().into();
        assert_eq!(in_order, expected);
        let rebuilt: Vec<u8> = DiffSetBuilder::from(parsed).reverse().build();
        assert_eq!(rebuilt, expected);
    }
}

#[test]
fn reverse_swaps_a_key_change() {
    let rekey = ItemUpdate::from(items())
        .set(0, 1i64, 7i64)
        .unwrap()
        .set(1, 1i64, 2i64)
        .unwrap();
    let reversed = ChangeSet::<_, String, Vec<u8>>::new()
        .update(rekey)
        .reverse();
    let expected = ChangeSet::<_, String, Vec<u8>>::new().update(
        ItemUpdate::from(items())
            .set(0, 7i64, 1i64)
            .unwrap()
            .set(1, 2i64, 1i64)
            .unwrap(),
    );
    assert_eq!(reversed.build(), expected.build());
}
