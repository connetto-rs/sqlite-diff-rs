//! Diesel-feature tests: identifier quoting per backend, bind placeholder
//! rendering, injection resistance, all value variants, and guarded
//! ill-formed cases.

#![cfg(feature = "diesel")]

extern crate alloc;

use diesel::debug_query;
use diesel::mysql::Mysql;
use diesel::pg::Pg;

use sqlite_diff_rs::{DiffOps, DynTable, Insert, PatchDelete, PatchSet, PatchUpdate, SimpleTable};

fn render_pg<
    S: AsRef<str> + Clone + core::hash::Hash + Eq,
    B: AsRef<[u8]> + Clone + core::hash::Hash + Eq,
>(
    patchset: &PatchSet<SimpleTable, S, B>,
) -> alloc::string::String {
    use alloc::string::ToString;
    let mut out = alloc::string::String::new();
    let mut first = true;
    for stmt in patchset.iter() {
        if !first {
            out.push_str("; ");
        }
        first = false;
        out.push_str(&debug_query::<Pg, _>(&stmt).to_string());
    }
    out
}

fn render_mysql<
    S: AsRef<str> + Clone + core::hash::Hash + Eq,
    B: AsRef<[u8]> + Clone + core::hash::Hash + Eq,
>(
    patchset: &PatchSet<SimpleTable, S, B>,
) -> alloc::string::String {
    use alloc::string::ToString;
    let mut out = alloc::string::String::new();
    let mut first = true;
    for stmt in patchset.iter() {
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
fn insert_pg_binds_values_and_quotes_identifiers() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "Alice")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"INSERT INTO "users" ("id", "name") VALUES ($1, $2)"#),
        "{sql}"
    );
    assert!(sql.contains("binds:"));
    assert!(sql.contains("Alice"), "{sql}");
    assert!(
        !sql.contains("'Alice'"),
        "value leaked into SQL text: {sql}"
    );
}

#[test]
fn insert_mysql_uses_backtick_identifiers() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "Bob")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_mysql(&patchset);
    assert!(
        sql.starts_with("INSERT INTO `users` (`id`, `name`) VALUES (?, ?)"),
        "{sql}"
    );
    assert!(!sql.contains("'Bob'"), "value leaked into SQL text: {sql}");
}

#[test]
fn insert_with_null_emits_null_literal() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 42_i64)
        .unwrap()
        .set_null(1)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"INSERT INTO "users" ("id", "name") VALUES ($1, NULL)"#),
        "{sql}"
    );
}

// UPDATE ---------------------------------------------------------------------

#[test]
fn update_pg_puts_pk_in_where_and_skips_pk_from_set() {
    let table = SimpleTable::with_rowid_alias(
        "users",
        &[("id", "INTEGER"), ("name", "TEXT"), ("email", "TEXT")],
        0,
    );
    let update = PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(table.clone())
        .set(0, 7_i64)
        .unwrap()
        .set(2, "new@example.com")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"UPDATE "users" SET "email" = $1 WHERE "id" = $2"#),
        "{sql}"
    );
    assert!(
        !sql.contains(r#""name" ="#),
        "unchanged column leaked: {sql}"
    );
    let set_clause = &sql[..sql.find(" WHERE ").expect("WHERE clause")];
    assert!(
        !set_clause.contains(r#""id" = "#),
        "PK column leaked into SET: {sql}"
    );
}

#[test]
fn update_mysql_backticks_and_placeholder_style() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let update = PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(table.clone())
        .set(0, 3_i64)
        .unwrap()
        .set(1, "Renamed")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);

    let sql = render_mysql(&patchset);
    assert!(
        sql.starts_with("UPDATE `users` SET `name` = ? WHERE `id` = ?"),
        "{sql}"
    );
}

#[test]
fn update_composite_pk_all_pk_columns_in_where() {
    let table = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    let update = PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, 2_i64)
        .unwrap()
        .set(2, "v")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(
            r#"UPDATE "kv" SET "value" = $1 WHERE "tenant_id" = $2 AND "user_id" = $3"#
        ),
        "{sql}"
    );
}

// DELETE ---------------------------------------------------------------------

#[test]
fn delete_pg_uses_pk_only_where() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let delete = PatchDelete::new(table.clone(), alloc::vec![9_i64.into()]);
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().delete(delete);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"DELETE FROM "users" WHERE "id" = $1"#),
        "{sql}"
    );
}

#[test]
fn delete_mysql() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let delete = PatchDelete::new(table.clone(), alloc::vec![9_i64.into()]);
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().delete(delete);

    let sql = render_mysql(&patchset);
    assert!(
        sql.starts_with("DELETE FROM `users` WHERE `id` = ?"),
        "{sql}"
    );
}

// Injection resistance -------------------------------------------------------

#[test]
fn adversarial_string_value_never_terminates_sql() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "'); DROP TABLE users; --")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    let sql_section = sql.split(" -- binds:").next().unwrap_or(&sql);
    assert!(
        !sql_section.contains("DROP TABLE"),
        "adversarial payload leaked into SQL text: {sql}"
    );
    assert!(sql_section.contains("$2"), "{sql}");
}

#[test]
fn adversarial_table_and_column_names_are_quoted() {
    let table = SimpleTable::with_rowid_alias("us\"ers", &[("id\"col", "INTEGER")], 0);
    let insert = Insert::from(table.clone()).set(0, 1_i64).unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    assert!(sql.contains(r#""us""ers""#), "{sql}");
    assert!(sql.contains(r#""id""col""#), "{sql}");
}

// Backend polymorphism -------------------------------------------------------

#[test]
fn same_statement_walks_both_backends() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "Alice")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let pg = render_pg(&patchset);
    let mysql = render_mysql(&patchset);
    assert!(pg.contains(r#""users""#));
    assert!(mysql.contains("`users`"));
}

// All Value variants ---------------------------------------------------------

#[test]
fn insert_real_value_binds_as_double() {
    let table =
        SimpleTable::with_rowid_alias("prices", &[("id", "INTEGER"), ("amount", "REAL")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, 9.99_f64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"INSERT INTO "prices" ("id", "amount") VALUES ($1, $2)"#),
        "{sql}"
    );
    let sql_text = sql.split(" -- binds:").next().unwrap_or(&sql);
    assert!(
        !sql_text.contains("9.99"),
        "float leaked into SQL text: {sql}"
    );
}

#[test]
fn insert_blob_value_binds_as_binary() {
    let table = SimpleTable::with_rowid_alias("blobs", &[("id", "INTEGER"), ("data", "BLOB")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, alloc::vec![0xDE_u8, 0xAD, 0xBE, 0xEF])
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_pg(&patchset);
    assert!(
        sql.starts_with(r#"INSERT INTO "blobs" ("id", "data") VALUES ($1, $2)"#),
        "{sql}"
    );
    let sql_text = sql.split(" -- binds:").next().unwrap_or(&sql);
    assert!(
        !sql_text.contains("DEAD") && !sql_text.contains("dead"),
        "blob bytes leaked into SQL text: {sql}"
    );
}

// Multi-op / multi-table iteration -------------------------------------------

#[test]
fn iter_visits_every_op_across_multiple_tables() {
    use sqlite_diff_rs::PatchsetOp;

    let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let orders =
        SimpleTable::with_rowid_alias("orders", &[("id", "INTEGER"), ("total", "INTEGER")], 0);

    let patchset = PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new()
        .insert(
            Insert::from(users.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, "Alice")
                .unwrap(),
        )
        .insert(
            Insert::from(users.clone())
                .set(0, 2_i64)
                .unwrap()
                .set(1, "Bob")
                .unwrap(),
        )
        .delete(PatchDelete::new(orders.clone(), alloc::vec![7_i64.into()]))
        .insert(
            Insert::from(orders.clone())
                .set(0, 8_i64)
                .unwrap()
                .set(1, 42_i64)
                .unwrap(),
        );

    let ops: alloc::vec::Vec<_> = patchset.iter().collect();
    assert_eq!(ops.len(), 4);

    let names: alloc::vec::Vec<&str> = ops.iter().map(|op| op.table().name()).collect();
    assert_eq!(names, alloc::vec!["users", "users", "orders", "orders"]);

    assert!(matches!(ops[0], PatchsetOp::Insert { .. }));
    assert!(matches!(ops[1], PatchsetOp::Insert { .. }));
    assert!(matches!(ops[2], PatchsetOp::Delete { .. }));
    assert!(matches!(ops[3], PatchsetOp::Insert { .. }));

    let sql = render_pg(&patchset);
    let statements: alloc::vec::Vec<&str> = sql.split("; ").collect();
    assert_eq!(statements.len(), 4, "{sql}");
    assert!(statements[0].starts_with(r#"INSERT INTO "users""#), "{sql}");
    assert!(
        statements[2].starts_with(r#"DELETE FROM "orders""#),
        "{sql}"
    );
}

// Ill-formed queries surface a QueryBuilderError -----------------------------
//
// `debug_query::to_string()` turns a walk_ast error into `fmt::Error`, which
// panics `to_string()`; `catch_unwind` confirms the guard fired.

#[test]
fn update_with_only_pk_columns_set_returns_query_builder_error() {
    let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
    let update = PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(table.clone())
        .set(0, 1_i64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);
    let op = patchset.iter().next().expect("one op");

    let rendered = std::panic::catch_unwind(|| debug_query::<Pg, _>(&op).to_string());
    assert!(
        rendered.is_err(),
        "expected render failure, got: {rendered:?}"
    );
}

#[test]
fn delete_against_table_with_no_pk_returns_query_builder_error() {
    let table = SimpleTable::new("no_pk", &[("a", ""), ("b", "")], &[]);
    let delete = PatchDelete::new(table.clone(), alloc::vec![]);
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().delete(delete);
    let op = patchset.iter().next().expect("one op");

    let rendered = std::panic::catch_unwind(|| debug_query::<Pg, _>(&op).to_string());
    assert!(
        rendered.is_err(),
        "expected render failure, got: {rendered:?}"
    );
}

// Downstream `Adapter` + native `Binder` -----------------------------------
//
// The bool case is the driver: an adapter dispatches the `users.active`
// column through a `BoolBinder` that calls `push_bind_param::<sql_types::Bool,
// bool>(...)`. The rendered SQL must contain NO `CAST` anywhere; the value
// travels as a native Diesel bind of the target sql_type.

use diesel::backend::Backend;
use diesel::query_builder::AstPass;
use diesel::result::QueryResult;
use diesel::serialize::ToSql;
use diesel::sql_types::{BigInt, Bool};
use sqlite_diff_rs::{Adapter, Binder, DefaultBinder, Value};

/// User-side binder: owns a `bool`, binds it natively as `sql_types::Bool`.
struct BoolBinder(bool);

impl<DB> Binder<DB> for BoolBinder
where
    DB: Backend + diesel::sql_types::HasSqlType<Bool>,
    bool: ToSql<Bool, DB>,
{
    fn walk<'b>(&'b self, out: &mut AstPass<'_, 'b, DB>) -> QueryResult<()> {
        out.push_bind_param::<Bool, bool>(&self.0)
    }
}

/// User-side adapter. Maps:
/// * `users.active` (column 1) to `BoolBinder`.
/// * `orders_typed.active` (column 2) to `BoolBinder`; PK is `(order_id,
///   region_id)` with `region_id` at column 0 and `order_id` at column 1, so
///   PK ordinals reverse the column order and the walk_ast/resolve
///   lockstep must respect PK ordinal order, not column order.
/// * Everything else falls through to `DefaultBinder`.
struct UsersAdapter;

impl<DB, S, B> Adapter<DB, S, B> for UsersAdapter
where
    DB: Backend
        + diesel::sql_types::HasSqlType<Bool>
        + diesel::sql_types::HasSqlType<BigInt>
        + diesel::sql_types::HasSqlType<diesel::sql_types::Double>
        + diesel::sql_types::HasSqlType<diesel::sql_types::Text>
        + diesel::sql_types::HasSqlType<diesel::sql_types::Binary>,
    bool: ToSql<Bool, DB>,
    i64: ToSql<BigInt, DB>,
    f64: ToSql<diesel::sql_types::Double, DB>,
    str: ToSql<diesel::sql_types::Text, DB>,
    [u8]: ToSql<diesel::sql_types::Binary, DB>,
    S: AsRef<str> + Sync,
    B: AsRef<[u8]> + Sync,
{
    fn column_name(&self, table: &str, column_index: usize) -> &str {
        match table {
            "users" => ["id", "active"][column_index],
            "orders_typed" => ["region_id", "order_id", "active"][column_index],
            other => panic!("adapter has no column layout for table {other:?}"),
        }
    }

    fn bind<'a>(
        &self,
        table: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> diesel::result::QueryResult<Box<dyn Binder<DB> + Send + 'a>> {
        match (table, column_index, value) {
            ("users", 1, Value::Integer(i)) | ("orders_typed", 2, Value::Integer(i)) => {
                Ok(Box::new(BoolBinder(*i != 0)))
            }
            _ => Ok(Box::new(DefaultBinder::from(value))),
        }
    }
}

#[test]
fn adapter_binds_bool_natively_no_cast_in_emitted_sql() {
    // `active` is stored as SQLite INTEGER, but the target column is BOOLEAN.
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, 1_i64) // truthy
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let adapter = UsersAdapter;
    let mut sql = alloc::string::String::new();
    for op in patchset.iter().map(|op| op.with_adapter::<Pg, _>(&adapter)) {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    // Query template shape and identifier quoting are unchanged.
    assert!(
        sql.contains(r#"INSERT INTO "users" ("id", "active") VALUES ($1, $2)"#),
        "{sql}"
    );
    // Absolute rule from the design: NO CAST anywhere in the emitted SQL.
    assert!(!sql.contains("CAST"), "found unexpected CAST: {sql}");
    // The bool went through the bind collector as a real bool, not text or int.
    // `debug_query` renders binds via their `Debug` impl; `bool`'s `Debug`
    // produces `true` / `false`.
    assert!(
        sql.contains("true"),
        "expected native `true` bind in output: {sql}"
    );
    assert!(!sql.contains("TRUE"), "no literal keyword expected: {sql}");
}

#[test]
fn adapter_bool_binds_false_when_integer_is_zero() {
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, 0_i64) // falsy
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let mut sql = alloc::string::String::new();
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&UsersAdapter))
    {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }
    assert!(!sql.contains("CAST"), "{sql}");
    assert!(sql.contains("false"), "{sql}");
}

#[test]
fn adapter_falls_through_to_default_binder_for_other_columns() {
    // `id` gets DefaultBinder -> BigInt. Value in the debug binds should read
    // as an integer, not a bool.
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 42_i64)
        .unwrap()
        .set(1, 1_i64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let mut sql = alloc::string::String::new();
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&UsersAdapter))
    {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }
    assert!(!sql.contains("CAST"), "{sql}");
    assert!(sql.contains("42"), "id bind missing: {sql}");
}

// Lockstep tests: resolve() and walk_ast() must agree on binder order for
// UPDATE (SET first, then WHERE), DELETE (WHERE only), and composite PKs
// where PK ordinal reverses column order.

#[test]
fn adapter_update_binds_set_then_where_in_lockstep() {
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    // SET active = false, WHERE id = 42.
    let update = PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(table.clone())
        .set(0, 42_i64) // PK slot (goes to WHERE)
        .unwrap()
        .set(1, 0_i64) // active = false (goes to SET, native BOOLEAN bind)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);

    let mut sql = alloc::string::String::new();
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&UsersAdapter))
    {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    assert!(
        sql.starts_with(r#"UPDATE "users" SET "active" = $1 WHERE "id" = $2"#),
        "wrong UPDATE shape: {sql}"
    );
    assert!(!sql.contains("CAST"), "{sql}");
    // Lockstep witness: the SET bind is $1 (a bool), the WHERE bind is $2
    // (a bigint). If resolve() pushed WHERE-first, the debug binds would be
    // `[42, false]` instead of `[false, 42]`.
    let binds = sql.split(" -- binds:").nth(1).unwrap_or("");
    let false_pos = binds.find("false").expect("false in binds");
    let forty_two_pos = binds.find("42").expect("42 in binds");
    assert!(
        false_pos < forty_two_pos,
        "SET bind (false) must precede WHERE bind (42): binds={binds}"
    );
}

#[test]
fn adapter_delete_emits_where_only_with_single_pk_bind() {
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let delete = PatchDelete::new(table.clone(), alloc::vec![7_i64.into()]);
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().delete(delete);

    let mut sql = alloc::string::String::new();
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&UsersAdapter))
    {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    assert!(
        sql.starts_with(r#"DELETE FROM "users" WHERE "id" = $1"#),
        "{sql}"
    );
    assert!(!sql.contains("CAST"), "{sql}");
    assert!(sql.contains('7'), "PK bind missing: {sql}");
}

#[test]
fn adapter_composite_pk_binds_in_pk_ordinal_order_not_column_order() {
    // `orders_typed` schema: column layout is (region_id, order_id, active).
    // PK is declared as `&[1, 0]`, meaning:
    //   - 1st PK column = column index 1 (order_id)
    //   - 2nd PK column = column index 0 (region_id)
    // So PRIMARY KEY (order_id, region_id) — PK ordinal reverses column
    // order. If resolve() pushed PK binders in column-index order instead
    // of PK-ordinal order, the values would land on the wrong columns.
    let table = SimpleTable::new(
        "orders_typed",
        &[
            ("region_id", "INTEGER"),
            ("order_id", "INTEGER"),
            ("active", "INTEGER"),
        ],
        &[1, 0], // order_id first (col 1), then region_id (col 0)
    );

    // PK values in PK-ordinal order: order_id = 999, region_id = 1.
    let delete = PatchDelete::new(table.clone(), alloc::vec![999_i64.into(), 1_i64.into()]);
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().delete(delete);

    let mut sql = alloc::string::String::new();
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&UsersAdapter))
    {
        use alloc::string::ToString;
        sql.push_str(&debug_query::<Pg, _>(&op).to_string());
    }

    // Identifier order in WHERE follows PK-ordinal order.
    assert!(
        sql.starts_with(r#"DELETE FROM "orders_typed" WHERE "order_id" = $1 AND "region_id" = $2"#),
        "{sql}"
    );
    // Bind order must also match PK ordinal order: [999, 1] not [1, 999].
    // Distinct values make this observable.
    let binds = sql.split(" -- binds:").nth(1).unwrap_or("");
    let pos_999 = binds.find("999").expect("999 in binds");
    let pos_1 = binds
        .find(", 1")
        .or_else(|| binds.find(" 1]"))
        .expect("1 in binds");
    assert!(
        pos_999 < pos_1,
        "PK-ordinal-0 bind (999) must precede PK-ordinal-1 bind (1): binds={binds}"
    );
}

// Send/Sync: adapter-bound ops move across thread boundaries -------------

/// Compile-time check: `T: Send` for the type parameter it's called with.
fn assert_send<T: Send>(_: &T) {}

#[test]
fn bound_patchset_op_is_send() {
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let patchset = PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new()
        .insert(
            Insert::from(table.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 1_i64)
                .unwrap(),
        );
    let adapter = UsersAdapter;
    let op = patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&adapter))
        .next()
        .unwrap();
    assert_send(&op); // fails to compile if `Send` regresses.
}

#[test]
fn bound_patchset_ops_survive_thread_scope() {
    // Real threading witness: build a batch, distribute across scoped
    // threads, render each. If the type isn't `Send`, this doesn't compile.
    let table =
        SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let patchset = PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new()
        .insert(
            Insert::from(table.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 1_i64)
                .unwrap(),
        )
        .insert(
            Insert::from(table.clone())
                .set(0, 2_i64)
                .unwrap()
                .set(1, 0_i64)
                .unwrap(),
        );
    let adapter = UsersAdapter;
    let ops: alloc::vec::Vec<_> = patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&adapter))
        .collect();

    std::thread::scope(|s| {
        for op in ops {
            s.spawn(move || {
                use alloc::string::ToString;
                let sql = debug_query::<Pg, _>(&op).to_string();
                assert!(sql.contains("INSERT INTO"));
                assert!(!sql.contains("CAST"));
            });
        }
    });
}

// Adapter that rejects the mapping ---------------------------------------
//
// The trait's fallibility exists specifically so an adapter can say "this
// source value cannot be represented as the target column type". The
// canonical case is a SQLite `INTEGER` source paired with a target `UUID`
// column — no integer parses into a UUID, and the adapter should signal
// that at bind time.

struct UuidRejectingAdapter;

impl<S, B> Adapter<Pg, S, B> for UuidRejectingAdapter
where
    S: AsRef<str> + Sync,
    B: AsRef<[u8]> + Sync,
{
    fn column_name(&self, _table: &str, column_index: usize) -> &str {
        ["id", "session"][column_index]
    }

    fn bind<'a>(
        &self,
        _table: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> diesel::result::QueryResult<Box<dyn Binder<Pg> + Send + 'a>> {
        // Column 1 is a UUID; only text is representable, integers are not.
        if column_index == 1 && matches!(value, Value::Integer(_)) {
            return Err(diesel::result::Error::QueryBuilderError(Box::new(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cannot bind Integer to UUID column",
                ),
            )));
        }
        Ok(Box::new(DefaultBinder::from(value)))
    }
}

#[test]
fn adapter_bind_error_surfaces_at_walk_time() {
    let table =
        SimpleTable::with_rowid_alias("accounts", &[("id", "INTEGER"), ("session", "INTEGER")], 0);
    // Column 1 is `Integer` but the adapter treats it as a UUID column.
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, 42_i64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let op = patchset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Pg, _>(&UuidRejectingAdapter);

    // `debug_query::to_string()` turns a walk_ast error into `fmt::Error`,
    // which makes `to_string()` panic. `catch_unwind` confirms the guarded
    // error path fired.
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        debug_query::<Pg, _>(&op).to_string()
    }));
    assert!(
        rendered.is_err(),
        "adapter bind error should propagate as a walk_ast failure, got: {rendered:?}"
    );
}

#[test]
fn adapter_bind_ok_still_walks_normally() {
    // Sanity witness: the same adapter succeeds when column 1 has a
    // representable value (a Text). No error, no panic on debug rendering.
    let table =
        SimpleTable::with_rowid_alias("accounts", &[("id", "INTEGER"), ("session", "TEXT")], 0);
    let insert = Insert::from(table.clone())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "550e8400-e29b-41d4-a716-446655440000")
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let op = patchset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Pg, _>(&UuidRejectingAdapter);
    let sql = debug_query::<Pg, _>(&op).to_string();
    assert!(
        sql.starts_with(r#"INSERT INTO "accounts" ("id", "session") VALUES ($1, $2)"#),
        "{sql}"
    );
}

// Adapter that leaves columns out of an INSERT ----------------------------

/// Leaves out a `NULL` in `organizations.founded_at` and every column of `counters`.
struct GrantsAdapter;

impl<S, B> Adapter<Pg, S, B> for GrantsAdapter
where
    S: AsRef<str> + Sync,
    B: AsRef<[u8]> + Sync,
{
    fn column_name(&self, table: &str, column_index: usize) -> &str {
        match table {
            "organizations" => ["id", "founded_at", "created_by_id"][column_index],
            "counters" => ["id"][column_index],
            other => panic!("adapter has no column layout for table {other:?}"),
        }
    }

    fn bind<'a>(
        &self,
        _table: &str,
        _column_index: usize,
        value: &'a Value<S, B>,
    ) -> diesel::result::QueryResult<Box<dyn Binder<Pg> + Send + 'a>> {
        Ok(Box::new(DefaultBinder::from(value)))
    }

    fn omits_from_insert(&self, table: &str, column_index: usize, value: &Value<S, B>) -> bool {
        matches!(
            (table, column_index, value),
            ("organizations", 1, Value::Null)
        ) || table == "counters"
    }
}

fn organizations() -> SimpleTable {
    SimpleTable::with_rowid_alias(
        "organizations",
        &[
            ("id", "INTEGER"),
            ("founded_at", "TEXT"),
            ("created_by_id", "INTEGER"),
        ],
        0,
    )
}

fn render_with_grants(
    patchset: &PatchSet<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>,
) -> alloc::string::String {
    use alloc::string::ToString;
    let op = patchset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Pg, _>(&GrantsAdapter);
    debug_query::<Pg, _>(&op).to_string()
}

#[test]
fn adapter_leaves_an_omitted_column_out_of_columns_and_values() {
    let insert = Insert::from(organizations())
        .set(0, 1_i64)
        .unwrap()
        .set_null(1)
        .unwrap()
        .set(2, 7_i64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_with_grants(&patchset);
    assert_eq!(
        sql,
        r#"INSERT INTO "organizations" ("id", "created_by_id") VALUES ($1, $2) -- binds: [1, 7]"#
    );
}

#[test]
fn adapter_keeps_a_column_it_omits_only_when_null() {
    let insert = Insert::from(organizations())
        .set(0, 1_i64)
        .unwrap()
        .set(1, "2024-01-01")
        .unwrap()
        .set(2, 7_i64)
        .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);

    let sql = render_with_grants(&patchset);
    assert_eq!(
        sql,
        r#"INSERT INTO "organizations" ("id", "founded_at", "created_by_id") VALUES ($1, $2, $3) -- binds: [1, "2024-01-01", 7]"#
    );
}

#[test]
fn adapter_omission_leaves_update_untouched() {
    let update =
        PatchUpdate::<_, alloc::string::String, alloc::vec::Vec<u8>>::from(organizations())
            .set(0, 1_i64)
            .unwrap()
            .set_null(1)
            .unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().update(update);

    let sql = render_with_grants(&patchset);
    assert_eq!(
        sql,
        r#"UPDATE "organizations" SET "founded_at" = NULL WHERE "id" = $1 -- binds: [1]"#
    );
}

#[test]
fn adapter_omitting_every_column_fails_to_render() {
    let table = SimpleTable::with_rowid_alias("counters", &[("id", "INTEGER")], 0);
    let insert = Insert::from(table).set(0, 1_i64).unwrap();
    let patchset =
        PatchSet::<SimpleTable, alloc::string::String, alloc::vec::Vec<u8>>::new().insert(insert);
    let op = patchset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Pg, _>(&GrantsAdapter);

    let mut query_builder = diesel::pg::PgQueryBuilder::default();
    let err = diesel::query_builder::QueryFragment::<Pg>::to_sql(&op, &mut query_builder, &Pg)
        .expect_err("an INSERT naming no column must not render");
    assert!(
        matches!(&err, diesel::result::Error::QueryBuilderError(inner)
            if inner.to_string() == "INSERT leaves out every column"),
        "{err:?}"
    );
}
