//! Integration tests for SQL digestion into `DiffSetBuilder` via `digest_sql`.
//!
//! These tests verify that `DiffSetBuilder::digest_sql` correctly parses SQL
//! DML statements (INSERT, UPDATE, DELETE) and populates the patchset builder.
//! Schemas are created manually via `SimpleTable::new`.

#![cfg(feature = "testing")]

use sqlite_diff_rs::testing::assert_patchset_sql_parity;
use sqlite_diff_rs::{PatchSet, SimpleTable};

/// Helper: create a `PatchSet` with tables pre-registered.
fn patchset_with(tables: &[SimpleTable]) -> PatchSet<SimpleTable, String, Vec<u8>> {
    let mut ps = PatchSet::new();
    for t in tables {
        ps.add_table(t);
    }
    ps
}

// =============================================================================
// Basic parsing tests
// =============================================================================

#[test]
fn test_digest_simple_insert() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');")
        .unwrap();
    assert_eq!(ps.len(), 1);
    assert!(!ps.is_empty());
    assert_ne!(ps.build(), [] as [u8; 0]);
}

#[test]
fn test_digest_simple_delete() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql("DELETE FROM users WHERE id = 1;").unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_digest_simple_update() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql("UPDATE users SET name = 'Bob' WHERE id = 1;")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_digest_multiple_tables() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let posts = SimpleTable::with_rowid_alias(
        "posts",
        &[
            ("id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("content", "TEXT"),
        ],
        0,
    );
    let mut ps = patchset_with(&[users, posts]);
    ps.digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');")
        .unwrap();
    ps.digest_sql("INSERT INTO posts (id, user_id, content) VALUES (1, 1, 'Hello World');")
        .unwrap();
    assert_eq!(ps.len(), 2);
}

#[test]
fn test_digest_mixed_operations() {
    let users = SimpleTable::with_rowid_alias(
        "users",
        &[("id", "INTEGER"), ("name", "TEXT"), ("age", "INTEGER")],
        0,
    );
    let mut ps = patchset_with(&[users]);
    ps.digest_sql(
        "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30);\
         INSERT INTO users (id, name, age) VALUES (2, 'Bob', 25);\
         UPDATE users SET age = 31 WHERE id = 1;\
         DELETE FROM users WHERE id = 2;",
    )
    .unwrap();
    // INSERT(1) + UPDATE(1) = INSERT(1) with updated values
    // INSERT(2) + DELETE(2) = cancelled out
    assert_eq!(ps.len(), 1);
}

// =============================================================================
// Error handling tests
// =============================================================================

#[test]
fn test_table_not_registered_error() {
    let mut ps: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
    let result = ps.digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');");
    assert!(result.is_err());
}

#[test]
fn test_invalid_sql_error() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    let result = ps.digest_sql("THIS IS NOT VALID SQL");
    assert!(result.is_err());
}
#[test]
fn test_bare_trailing_block_comment_is_rejected_by_sqlite_and_digest_sql() {
    let table = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
    let mut ps = patchset_with(&[table]);
    ps.digest_sql("INSERT INTO t VALUES (10, 20)").unwrap();
    let before = ps.build();
    let sql = "INSERT\r\r\r\r\r\r\rINTO\tt\r\x0c\x0cVALUES(1, 2)               \x0c/*";

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)")
        .unwrap();
    assert!(conn.execute_batch(sql).is_err());
    assert!(matches!(
        ps.digest_sql(sql),
        Err(sqlite_diff_rs::builders::sql::ParseError::Lexer(_))
    ));
    assert_eq!(ps.build(), before);
}

#[test]
fn test_sql_ends_at_first_nul_for_sqlite_and_digest_sql() {
    let table = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
        .unwrap();
    for sql in [
        "INSERT INTO t VALUES (1, '2') /*\0 */",
        "INSERT INTO t VALUES (1, 'a\0b')",
        "INSERT INTO t VALUES (1, X'AB\0CD')",
        "INSERT INTO \"t\0\" VALUES (1, '2')",
    ] {
        let mut ps = patchset_with(core::slice::from_ref(&table));
        ps.digest_sql("INSERT INTO t VALUES (10, '20')").unwrap();
        let before = ps.build();
        assert!(conn.execute_batch(sql).is_err(), "{sql:?}");
        assert!(
            matches!(
                ps.digest_sql(sql),
                Err(sqlite_diff_rs::builders::sql::ParseError::Lexer(_))
            ),
            "{sql:?}"
        );
        assert_eq!(ps.build(), before, "{sql:?}");
    }
}

#[test]
fn test_create_table_rejected() {
    let mut ps: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
    let result = ps.digest_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)");
    assert!(result.is_err());
}

#[test]
fn test_unknown_column_error() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    let result = ps.digest_sql("INSERT INTO users (id, nonexistent) VALUES (1, 'Alice');");
    assert!(result.is_err());
}

// =============================================================================
// Complex value types tests
// =============================================================================

#[test]
fn test_digest_integer_values() {
    let numbers =
        SimpleTable::with_rowid_alias("numbers", &[("id", "INTEGER"), ("value", "INTEGER")], 0);
    let mut ps = patchset_with(&[numbers]);
    ps.digest_sql(
        "INSERT INTO numbers (id, value) VALUES (1, 42);\
         INSERT INTO numbers (id, value) VALUES (2, -100);\
         INSERT INTO numbers (id, value) VALUES (3, 0);",
    )
    .unwrap();
    assert_eq!(ps.len(), 3);
}

#[test]
fn test_digest_real_values() {
    let floats =
        SimpleTable::with_rowid_alias("floats", &[("id", "INTEGER"), ("value", "REAL")], 0);
    let mut ps = patchset_with(&[floats]);
    ps.digest_sql(
        "INSERT INTO floats (id, value) VALUES (1, 3.14);\
         INSERT INTO floats (id, value) VALUES (2, -2.5);",
    )
    .unwrap();
    assert_eq!(ps.len(), 2);
}

#[test]
fn test_digest_text_values() {
    let texts = SimpleTable::with_rowid_alias("texts", &[("id", "INTEGER"), ("value", "TEXT")], 0);
    let mut ps = patchset_with(&[texts]);
    ps.digest_sql(
        "INSERT INTO texts (id, value) VALUES (1, 'hello');\
         INSERT INTO texts (id, value) VALUES (2, 'world');\
         INSERT INTO texts (id, value) VALUES (3, '');",
    )
    .unwrap();
    assert_eq!(ps.len(), 3);
}

#[test]
fn test_digest_null_values() {
    let nullable =
        SimpleTable::with_rowid_alias("nullable", &[("id", "INTEGER"), ("value", "")], 0);
    let mut ps = patchset_with(&[nullable]);
    ps.digest_sql("INSERT INTO nullable (id, value) VALUES (1, NULL);")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_digest_blob_values() {
    let blobs = SimpleTable::with_rowid_alias("blobs", &[("id", "INTEGER"), ("data", "BLOB")], 0);
    let mut ps = patchset_with(&[blobs]);
    ps.digest_sql("INSERT INTO blobs (id, data) VALUES (1, X'DEADBEEF');")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

// =============================================================================
// Composite primary key tests
// =============================================================================

#[test]
fn test_digest_composite_pk_insert() {
    let composite = SimpleTable::new(
        "composite",
        &[("a", "INTEGER"), ("b", "INTEGER"), ("value", "TEXT")],
        &[0, 1],
    );
    let mut ps = patchset_with(&[composite]);
    ps.digest_sql("INSERT INTO composite (a, b, value) VALUES (1, 2, 'test');")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_digest_composite_pk_delete() {
    let composite = SimpleTable::new(
        "composite",
        &[("a", "INTEGER"), ("b", "INTEGER"), ("value", "TEXT")],
        &[0, 1],
    );
    let mut ps = patchset_with(&[composite]);
    ps.digest_sql("DELETE FROM composite WHERE a = 1 AND b = 2;")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_digest_composite_pk_update() {
    let composite = SimpleTable::new(
        "composite",
        &[("a", "INTEGER"), ("b", "INTEGER"), ("value", "TEXT")],
        &[0, 1],
    );
    let mut ps = patchset_with(&[composite]);
    ps.digest_sql("UPDATE composite SET value = 'updated' WHERE a = 1 AND b = 2;")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

// =============================================================================
// Operation consolidation tests
// =============================================================================

#[test]
fn test_insert_then_delete_cancels() {
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let mut ps = patchset_with(&[t]);
    ps.digest_sql(
        "INSERT INTO t (id, v) VALUES (1, 'a');\
         DELETE FROM t WHERE id = 1;",
    )
    .unwrap();
    // INSERT + DELETE on same PK should cancel out
    assert_eq!(ps.len(), 0);
}

#[test]
fn test_insert_then_update_becomes_insert() {
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let mut ps = patchset_with(&[t]);
    ps.digest_sql(
        "INSERT INTO t (id, v) VALUES (1, 'a');\
         UPDATE t SET v = 'b' WHERE id = 1;",
    )
    .unwrap();
    // INSERT + UPDATE = INSERT with updated values
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_update_then_update_consolidates() {
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let mut ps = patchset_with(&[t]);
    ps.digest_sql(
        "UPDATE t SET v = 'a' WHERE id = 1;\
         UPDATE t SET v = 'b' WHERE id = 1;",
    )
    .unwrap();
    // UPDATE + UPDATE = single UPDATE
    assert_eq!(ps.len(), 1);
}

// =============================================================================
// Edge cases
// =============================================================================

#[test]
fn test_empty_sql() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql("").unwrap();
    assert!(ps.is_empty());
}

#[test]
fn test_semicolons_only() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql(";;;").unwrap();
    assert!(ps.is_empty());
}

#[test]
fn test_multiple_digest_calls() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let mut ps = patchset_with(&[users]);
    ps.digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');")
        .unwrap();
    ps.digest_sql("INSERT INTO users (id, name) VALUES (2, 'Bob');")
        .unwrap();
    assert_eq!(ps.len(), 2);
}

#[test]
fn test_delete_rejects_non_pk_in_where() {
    let users = SimpleTable::with_rowid_alias(
        "users",
        &[("id", "INTEGER"), ("name", "TEXT"), ("status", "TEXT")],
        0,
    );
    let mut ps = patchset_with(&[users]);
    let result = ps.digest_sql("DELETE FROM users WHERE id = 1 AND status = 'active'");
    assert!(result.is_err());
}

#[test]
fn test_negative_numbers() {
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", "REAL")], &[0]);
    let mut ps = patchset_with(&[t]);
    ps.digest_sql("INSERT INTO t (a, b) VALUES (-42, -3.14);")
        .unwrap();
    assert_eq!(ps.len(), 1);
}

// =============================================================================
// Bit-parity tests against rusqlite (patchset only)
// =============================================================================

#[test]
fn parity_single_insert() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    assert_patchset_sql_parity(
        &[users],
        &[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        ],
    );
}

#[test]
fn parity_insert_and_update() {
    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    assert_patchset_sql_parity(
        &[users],
        &[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO users (id, name) VALUES (1, 'Alice')",
            "UPDATE users SET name = 'Alicia' WHERE id = 1",
        ],
    );
}

#[test]
fn parity_composite_pk() {
    let order_items = SimpleTable::new(
        "order_items",
        &[
            ("order_id", "INTEGER"),
            ("item_id", "INTEGER"),
            ("quantity", "INTEGER"),
        ],
        &[0, 1],
    );
    assert_patchset_sql_parity(
        &[order_items],
        &[
            "CREATE TABLE order_items (order_id INTEGER, item_id INTEGER, quantity INTEGER, PRIMARY KEY (order_id, item_id))",
            "INSERT INTO order_items (order_id, item_id, quantity) VALUES (1, 100, 5)",
            "INSERT INTO order_items (order_id, item_id, quantity) VALUES (1, 101, 3)",
        ],
    );
}

// =============================================================================
// New validation tests (Phase 1C and 1D)
// =============================================================================

#[test]
fn test_explicit_values_shorter_than_column_list() {
    // INSERT INTO t (a, b) VALUES (1) silently filled b with NULL before this fix.
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", "")], &[0]);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("INSERT INTO t (a, b) VALUES (1)");
    assert!(
        result.is_err(),
        "expected error for fewer VALUES than column list entries"
    );
}

#[test]
fn test_explicit_values_matching_column_list_accepted() {
    // Matching counts must still work.
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", "INTEGER")], &[0]);
    let mut ps = patchset_with(&[t]);
    ps.digest_sql("INSERT INTO t (a, b) VALUES (1, 2)").unwrap();
    assert_eq!(ps.len(), 1);
}

#[test]
fn test_positional_fewer_values_already_errors() {
    // The positional form already called expect(Comma) before each value
    // after the first, so a short list already produced an error before
    // this change. Confirm it still does.
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", "")], &[0]);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("INSERT INTO t VALUES (1)");
    assert!(result.is_err());
}

#[test]
fn test_atomicity_failure_leaves_builder_untouched() {
    // The first statement is valid, the second is not. The builder must be
    // empty after the call, not contain the first operation.
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql(
        "INSERT INTO t (id, v) VALUES (1, 'ok'); \
         INSERT INTO unknown_table (id) VALUES (2);",
    );
    assert!(result.is_err(), "second statement must fail");
    assert_eq!(
        ps.len(),
        0,
        "builder must be untouched after a partial failure"
    );
}

#[test]
fn test_or_in_where_delete_rejected() {
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "")], 0);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("DELETE FROM t WHERE id = 1 OR id = 2");
    assert!(result.is_err(), "OR in WHERE must be rejected");
}

#[test]
fn test_or_in_where_update_rejected() {
    let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "TEXT")], 0);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("UPDATE t SET v = 'x' WHERE id = 1 OR id = 2");
    assert!(result.is_err(), "OR in WHERE must be rejected");
}

#[test]
fn test_partial_pk_delete_rejected() {
    // Two-column primary key, only one column given in WHERE.
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", ""), ("v", "")], &[0, 1]);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("DELETE FROM t WHERE a = 1");
    assert!(
        result.is_err(),
        "partial primary key in WHERE must be rejected"
    );
}

#[test]
fn test_partial_pk_update_rejected() {
    let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", ""), ("v", "TEXT")], &[0, 1]);
    let mut ps = patchset_with(&[t]);
    let result = ps.digest_sql("UPDATE t SET v = 'x' WHERE a = 1");
    assert!(
        result.is_err(),
        "partial primary key in WHERE must be rejected"
    );
}

#[test]
fn test_complete_composite_pk_where_accepted() {
    // Both PK columns given: must succeed.
    let t = SimpleTable::new(
        "t",
        &[("a", "INTEGER"), ("b", "INTEGER"), ("v", "")],
        &[0, 1],
    );
    let mut ps = patchset_with(&[t]);
    ps.digest_sql("DELETE FROM t WHERE a = 1 AND b = 2")
        .unwrap();
    assert_eq!(ps.len(), 1);
}
