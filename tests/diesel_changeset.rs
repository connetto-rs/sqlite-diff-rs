//! Diesel-feature tests for changeset rendering. Mirrors the patchset suite
//! and adds the cases that only a changeset can express: primary-key changes,
//! including composite keys. A patchset stores no new primary-key value, so
//! those updates are changeset-only.

#![cfg(feature = "diesel")]

use diesel::debug_query;
use diesel::mysql::Mysql;
use diesel::pg::Pg;

use sqlite_diff_rs::{
    ChangeDelete, ChangeSet, ChangeUpdate, DiffOps, Insert, PatchSet, PatchUpdate, SimpleTable,
};

fn render_pg<S, B>(changeset: &ChangeSet<SimpleTable, S, B>) -> String
where
    S: AsRef<str> + Clone + core::hash::Hash + Eq + core::fmt::Debug,
    B: AsRef<[u8]> + Clone + core::hash::Hash + Eq + core::fmt::Debug,
{
    let mut out = String::new();
    let mut first = true;
    for stmt in changeset.iter() {
        if !first {
            out.push_str("; ");
        }
        first = false;
        out.push_str(&debug_query::<Pg, _>(&stmt).to_string());
    }
    out
}

fn render_mysql<S, B>(changeset: &ChangeSet<SimpleTable, S, B>) -> String
where
    S: AsRef<str> + Clone + core::hash::Hash + Eq + core::fmt::Debug,
    B: AsRef<[u8]> + Clone + core::hash::Hash + Eq + core::fmt::Debug,
{
    let mut out = String::new();
    let mut first = true;
    for stmt in changeset.iter() {
        if !first {
            out.push_str("; ");
        }
        first = false;
        out.push_str(&debug_query::<Mysql, _>(&stmt).to_string());
    }
    out
}

// INSERT ---------------------------------------------------------------------

#[test]
fn insert_matches_patchset_shape() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(table)
            .set(0, 1_i64)
            .unwrap()
            .set(1, "Alice")
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(r#"INSERT INTO "users" ("id", "name") VALUES ($1, $2)"#),
        "{sql}"
    );
    assert!(sql.contains("Alice"), "{sql}");
    assert!(
        !sql.contains("'Alice'"),
        "value leaked into SQL text: {sql}"
    );
}

#[test]
fn insert_mysql_backticks() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(table)
            .set(0, 1_i64)
            .unwrap()
            .set(1, "Bob")
            .unwrap(),
    );

    let sql = render_mysql(&changeset);
    assert!(
        sql.starts_with("INSERT INTO `users` (`id`, `name`) VALUES (?, ?)"),
        "{sql}"
    );
}

// UPDATE: only actually-changed columns are written -------------------------

#[test]
fn update_writes_only_changed_columns_and_keeps_pk_out_of_set_when_unchanged() {
    let table = SimpleTable::with_rowid_alias(
        "users",
        &[("id", "INTEGER"), ("name", "TEXT"), ("email", "TEXT")],
        0,
    );
    // PK unchanged (old == new), name changed, email untouched.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 7_i64, 7_i64)
            .unwrap()
            .set(1, "old", "new")
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(r#"UPDATE "users" SET "name" = $1 WHERE "id" = $2"#),
        "{sql}"
    );
    let set_clause = &sql[..sql.find(" WHERE ").expect("WHERE clause")];
    assert!(
        !set_clause.contains(r#""id" ="#),
        "unchanged PK leaked into SET (would spuriously touch it): {sql}"
    );
    assert!(
        !set_clause.contains(r#""email""#),
        "untouched column in SET: {sql}"
    );
    assert!(sql.contains(r#"binds: ["new", 7]"#), "{sql}");
}

// UPDATE: primary-key changes (the changeset-only corner cases) --------------

#[test]
fn update_changes_a_single_primary_key() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 1_i64, 2_i64)
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    // SET writes the new PK, WHERE matches the old PK.
    assert!(
        sql.starts_with(r#"UPDATE "users" SET "id" = $1 WHERE "id" = $2"#),
        "{sql}"
    );
    assert!(sql.contains("binds: [2, 1]"), "{sql}");
}

#[test]
fn update_changes_one_column_of_a_composite_primary_key() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    // tenant_id unchanged, user_id 2 -> 9 (PK move), value untouched.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 5_i64)
            .unwrap()
            .set(1, 2_i64, 9_i64)
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(
            r#"UPDATE "kv" SET "user_id" = $1 WHERE "tenant_id" = $2 AND "user_id" = $3"#
        ),
        "{sql}"
    );
    let set_clause = &sql[..sql.find(" WHERE ").expect("WHERE clause")];
    assert!(
        !set_clause.contains(r#""tenant_id""#),
        "unchanged PK column leaked into SET: {sql}"
    );
    // WHERE binds the OLD composite key (user_id = 2), not the new one.
    assert!(sql.contains("binds: [9, 5, 2]"), "{sql}");
}

#[test]
fn update_changes_every_column_of_a_composite_primary_key() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 50_i64)
            .unwrap()
            .set(1, 2_i64, 9_i64)
            .unwrap()
            .set(2, "a", "b")
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(
            r#"UPDATE "kv" SET "tenant_id" = $1, "user_id" = $2, "value" = $3 WHERE "tenant_id" = $4 AND "user_id" = $5"#
        ),
        "{sql}"
    );
    // SET carries the new key (50, 9), WHERE carries the old key (5, 2).
    assert!(sql.contains(r#"binds: [50, 9, "b", 5, 2]"#), "{sql}");
}

#[test]
fn update_with_no_actual_change_fails_to_render() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    // Every column set to its own value: nothing to write.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 1_i64, 1_i64)
            .unwrap()
            .set(1, "x", "x")
            .unwrap(),
    );
    let op = changeset.iter().next().expect("one op");

    let rendered = std::panic::catch_unwind(|| debug_query::<Pg, _>(&op).to_string());
    assert!(
        rendered.is_err(),
        "no-op update should not render: {rendered:?}"
    );
}

// DELETE: WHERE matches the primary key only, though the full old row is held.

#[test]
fn delete_matches_primary_key_only() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().delete(
        ChangeDelete::<_, String, Vec<u8>>::from(table)
            .set(0, 9_i64)
            .unwrap()
            .set(1, "Bob")
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(r#"DELETE FROM "users" WHERE "id" = $1"#),
        "{sql}"
    );
    assert!(
        !sql.contains(r#""name""#),
        "non-PK old value leaked into WHERE: {sql}"
    );
    assert!(sql.contains("binds: [9]"), "{sql}");
}

#[test]
fn delete_composite_pk_matches_all_key_columns_only() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().delete(
        ChangeDelete::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64)
            .unwrap()
            .set(1, 2_i64)
            .unwrap()
            .set(2, "v")
            .unwrap(),
    );

    let sql = render_mysql(&changeset);
    assert!(
        sql.starts_with("DELETE FROM `kv` WHERE `tenant_id` = ? AND `user_id` = ?"),
        "{sql}"
    );
    assert!(!sql.contains("`value`"), "non-PK leaked into WHERE: {sql}");
}

// Contrast: the same primary-key change is representable only as a changeset.

#[test]
fn patchset_cannot_change_a_primary_key_but_changeset_can() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);

    // Patchset: the format records no new PK value, so the PK is dropped from
    // SET, leaving an empty SET clause that cannot render.
    let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().update(
        PatchUpdate::<_, String, Vec<u8>>::from(table.clone())
            .set(0, 2_i64)
            .unwrap(),
    );
    let patch_op = patchset.iter().next().expect("one op");
    let patch_render = std::panic::catch_unwind(|| debug_query::<Pg, _>(&patch_op).to_string());
    assert!(
        patch_render.is_err(),
        "patchset must not render a PK change: {patch_render:?}"
    );

    // Changeset: carries old and new, so the PK move renders.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 1_i64, 2_i64)
            .unwrap(),
    );
    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(r#"UPDATE "users" SET "id" = $1 WHERE "id" = $2"#),
        "{sql}"
    );
}

// Adapter path: a composite-PK change binds SET (column order) then WHERE
// (PK-ordinal order), with no CAST and the old key in the predicate.

use diesel::backend::Backend;
use diesel::result::QueryResult;
use diesel::serialize::ToSql;
use diesel::sql_types::{BigInt, Binary, Double, HasSqlType, Text};
use sqlite_diff_rs::{Adapter, Binder, DefaultBinder, Value};

struct TestAdapter;

impl<DB, S, B> Adapter<DB, S, B> for TestAdapter
where
    DB: Backend + HasSqlType<BigInt> + HasSqlType<Double> + HasSqlType<Text> + HasSqlType<Binary>,
    i64: ToSql<BigInt, DB>,
    f64: ToSql<Double, DB>,
    str: ToSql<Text, DB>,
    [u8]: ToSql<Binary, DB>,
    S: AsRef<str> + Sync,
    B: AsRef<[u8]> + Sync,
{
    fn column_name(&self, table: &str, column_index: usize) -> &str {
        match table {
            "kv" => ["tenant_id", "user_id", "value"][column_index],
            "orders" => ["region_id", "order_id", "note"][column_index],
            other => panic!("test adapter has no column layout for table {other:?}"),
        }
    }

    fn bind<'a>(
        &self,
        _table: &str,
        _column_index: usize,
        value: &'a Value<S, B>,
    ) -> QueryResult<Box<dyn Binder<DB> + Send + 'a>> {
        Ok(Box::new(DefaultBinder::from(value)))
    }

    fn omits_from_insert(&self, table: &str, column_index: usize, value: &Value<S, B>) -> bool {
        matches!((table, column_index, value), ("kv", 2, Value::Null))
    }
}

#[test]
fn adapter_leaves_an_omitted_column_out_of_a_changeset_insert() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(table)
            .set(0, 1_i64)
            .unwrap()
            .set(1, 2_i64)
            .unwrap()
            .set_null(2)
            .unwrap(),
    );
    let op = changeset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Pg, _>(&TestAdapter);

    assert_eq!(
        debug_query::<Pg, _>(&op).to_string(),
        r#"INSERT INTO "kv" ("tenant_id", "user_id") VALUES ($1, $2) -- binds: [1, 2]"#
    );
}

#[test]
fn adapter_composite_pk_change_binds_set_then_where() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    // user_id 2 -> 9 (PK move) and value "a" -> "b", tenant_id unchanged.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 5_i64)
            .unwrap()
            .set(1, 2_i64, 9_i64)
            .unwrap()
            .set(2, "a", "b")
            .unwrap(),
    );

    let mut sql = String::new();
    for op in changeset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&TestAdapter))
    {
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    assert!(!sql.contains("CAST"), "found unexpected CAST: {sql}");
    assert!(
        sql.contains(
            r#"UPDATE "kv" SET "user_id" = $1, "value" = $2 WHERE "tenant_id" = $3 AND "user_id" = $4"#
        ),
        "{sql}"
    );
    // SET binds (new values, column order) precede WHERE binds (old key,
    // PK-ordinal order): [9, "b", 5, 2].
    assert!(sql.contains(r#"binds: [9, "b", 5, 2]"#), "{sql}");
}

// Reversed PK-ordinal order: the declared primary-key order differs from the
// column order, so a PK change must map ordinal <-> column index correctly in
// both SET (column order) and WHERE (PK-ordinal order). These are
// changeset-only: a patchset cannot change a primary key at all.

#[test]
fn changeset_reversed_ordinal_pk_change_one_column() {
    // orders(region_id [col 0], order_id [col 1], note [col 2]) with
    // PRIMARY KEY(order_id, region_id): PK-ordinal 0 is column 1.
    let table = SimpleTable::new(
        "orders",
        &[
            ("region_id", "INTEGER"),
            ("order_id", "INTEGER"),
            ("note", "TEXT"),
        ],
        &[1, 0],
    );
    // Change order_id (PK-ordinal 0, column 1); region_id unchanged.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 5_i64)
            .unwrap()
            .set(1, 999_i64, 1000_i64)
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(
            r#"UPDATE "orders" SET "order_id" = $1 WHERE "order_id" = $2 AND "region_id" = $3"#
        ),
        "{sql}"
    );
    let set_clause = &sql[..sql.find(" WHERE ").expect("WHERE clause")];
    assert!(
        !set_clause.contains(r#""region_id""#),
        "unchanged PK column leaked into SET: {sql}"
    );
    // SET binds the new order_id (1000). WHERE binds the OLD key in ordinal
    // order (order_id = 999, then region_id = 5).
    assert!(sql.contains("binds: [1000, 999, 5]"), "{sql}");
}

#[test]
fn changeset_reversed_ordinal_pk_change_both_columns() {
    let table = SimpleTable::new(
        "orders",
        &[
            ("region_id", "INTEGER"),
            ("order_id", "INTEGER"),
            ("note", "TEXT"),
        ],
        &[1, 0],
    );
    // Change both key columns: SET follows column order, WHERE follows
    // PK-ordinal order, and the two orders genuinely differ here.
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 50_i64)
            .unwrap()
            .set(1, 999_i64, 1000_i64)
            .unwrap(),
    );

    let sql = render_pg(&changeset);
    assert!(
        sql.starts_with(
            r#"UPDATE "orders" SET "region_id" = $1, "order_id" = $2 WHERE "order_id" = $3 AND "region_id" = $4"#
        ),
        "{sql}"
    );
    // SET column order (region_id=50, order_id=1000). WHERE ordinal order with
    // the OLD key (order_id=999, region_id=5).
    assert!(sql.contains("binds: [50, 1000, 999, 5]"), "{sql}");
}

#[test]
fn adapter_reversed_ordinal_pk_change_binds_in_lockstep() {
    let table = SimpleTable::new(
        "orders",
        &[
            ("region_id", "INTEGER"),
            ("order_id", "INTEGER"),
            ("note", "TEXT"),
        ],
        &[1, 0],
    );
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(table)
            .set(0, 5_i64, 50_i64)
            .unwrap()
            .set(1, 999_i64, 1000_i64)
            .unwrap(),
    );

    let mut sql = String::new();
    for op in changeset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&TestAdapter))
    {
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    assert!(!sql.contains("CAST"), "found unexpected CAST: {sql}");
    // The adapter path resolves binders up front. They must still land in
    // SET-then-WHERE order with WHERE in PK-ordinal order.
    assert!(
        sql.contains(
            r#"UPDATE "orders" SET "region_id" = $1, "order_id" = $2 WHERE "order_id" = $3 AND "region_id" = $4"#
        ),
        "{sql}"
    );
    assert!(sql.contains("binds: [50, 1000, 999, 5]"), "{sql}");
}
