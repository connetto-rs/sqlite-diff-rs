//! End-to-end byte-parity check against rusqlite's session extension.
//!
//! Pins the `differential_testing::run_differential_test` helper without
//! requiring a fuzz run. The crash inputs and structured fuzzers exercise
//! this helper through wrappers, but this file exercises it directly.

#![cfg(feature = "testing")]

use sqlite_diff_rs::SimpleTable;
use sqlite_diff_rs::differential_testing::run_differential_test;

#[test]
fn differential_insert_update_delete_byte_parity() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let create = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    let dml = [
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "INSERT INTO users (id, name) VALUES (2, 'Bob')",
        "UPDATE users SET name = 'Alicia' WHERE id = 1",
        "DELETE FROM users WHERE id = 2",
    ];
    assert!(run_differential_test(&[users], &[create], &dml));
}

#[test]
fn differential_multi_table_byte_parity() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let posts = SimpleTable::with_rowid_alias(
        "posts",
        &[("id", "INTEGER"), ("user_id", "INTEGER"), ("body", "TEXT")],
        0,
    );
    let create_users = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    let create_posts = "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, body TEXT)";
    let dml = [
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "INSERT INTO posts (id, user_id, body) VALUES (10, 1, 'hello')",
        "UPDATE posts SET body = 'world' WHERE id = 10",
    ];
    assert!(run_differential_test(
        &[users, posts],
        &[create_users, create_posts],
        &dml
    ));
}

/// The schemas fix the table order, as attaching each table does, whichever table changes first.
#[test]
fn differential_orders_tables_by_schema() {
    let t0 = SimpleTable::with_rowid_alias("t0", &[("a", "INTEGER")], 0);
    let t1 = SimpleTable::with_rowid_alias("t1", &[("a", "INTEGER")], 0);
    assert!(run_differential_test(
        &[t0, t1],
        &[
            "CREATE TABLE t0 (a INTEGER PRIMARY KEY)",
            "CREATE TABLE t1 (a INTEGER PRIMARY KEY)",
        ],
        &[
            "INSERT INTO t1 (a) VALUES (1)",
            "INSERT INTO t0 (a) VALUES (1)"
        ],
    ));
}

#[test]
fn differential_runs_every_statement_of_a_multi_statement_string() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let create = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    let dml = ["INSERT INTO users VALUES (1, 'a'); INSERT INTO users VALUES (2, 'b')"];
    assert!(run_differential_test(&[users], &[create], &dml));
}

#[test]
fn differential_accepts_complete_statement_with_valid_eof_comments() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let create = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    for sql in [
        "INSERT INTO users VALUES (1, 'a') /* text",
        "INSERT INTO users VALUES (1, 'a') /**/",
        "INSERT INTO users VALUES (1, 'a') --",
    ] {
        assert!(
            run_differential_test(std::slice::from_ref(&users), &[create], &[sql]),
            "{sql}"
        );
    }
}

#[test]
fn differential_ignores_everything_after_the_first_nul() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let create = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    for sql in [
        "INSERT INTO users VALUES (1, 'a')\0garbage",
        "INSERT INTO users VALUES (1, 'a');\0INSERT INTO users VALUES (2, 'b')",
        "INSERT INTO users VALUES (1, 'a') /* x \0 */ garbage",
        "INSERT INTO users VALUES (1, 'a') --\0\ngarbage",
        "/* \0 */ INSERT INTO users VALUES (1, 'a')",
        "-- \0\nINSERT INTO users VALUES (1, 'a')",
    ] {
        assert!(
            run_differential_test(std::slice::from_ref(&users), &[create], &[sql]),
            "{sql:?}"
        );
    }
}

#[test]
fn differential_skips_statements_without_a_counterpart_row() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let create = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
    for dml in [
        "UPDATE users SET name = 'x' WHERE id = 5",
        "DELETE FROM users WHERE id = 5",
        "INSERT INTO users VALUES (1, 'a'); INSERT INTO users VALUES (1, 'b')",
    ] {
        assert!(
            !run_differential_test(std::slice::from_ref(&users), &[create], &[dml]),
            "{dml}"
        );
    }
}
