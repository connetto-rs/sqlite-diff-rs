//! Postgres E2E: spin up the official Postgres image via testcontainers,
//! apply the shared patchsets, verify state.
//!
//! Requires a running Docker daemon. Without it, `Postgres::default().start()`
//! errors and the tests fail loudly.

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Binary, Double, Text};
use testcontainers_modules::postgres;
use testcontainers_modules::testcontainers::runners::SyncRunner;

const USERS_DDL: &str = "\
CREATE TABLE users (
    id      BIGINT PRIMARY KEY,
    name    TEXT NOT NULL,
    email   TEXT,
    score   DOUBLE PRECISION
)";

const BLOBS_DDL: &str = "\
CREATE TABLE blobs (
    id      BIGINT PRIMARY KEY,
    payload BYTEA
)";

const KV_DDL: &str = "\
CREATE TABLE kv (
    tenant_id BIGINT NOT NULL,
    user_id   BIGINT NOT NULL,
    value     TEXT,
    PRIMARY KEY (tenant_id, user_id)
)";

fn boot() -> (
    testcontainers_modules::testcontainers::Container<postgres::Postgres>,
    PgConnection,
) {
    let container = postgres::Postgres::default()
        .start()
        .expect("start Postgres container (Docker must be running)");
    let host = container.get_host().expect("container host");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("container Postgres port");
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let conn = PgConnection::establish(&url).expect("connect to Postgres");
    (container, conn)
}

#[test]
fn users_insert_update_delete_full_cycle() {
    let (_container, mut conn) = boot();
    diesel_e2e::users_full_cycle(&mut conn, USERS_DDL);
}

#[test]
fn blobs_null_and_raw_bytes_roundtrip() {
    let (_container, mut conn) = boot();
    diesel_e2e::blobs_roundtrip(&mut conn, BLOBS_DDL);
}

#[test]
fn kv_composite_pk_lifecycle() {
    let (_container, mut conn) = boot();
    diesel_e2e::kv_lifecycle(&mut conn, KV_DDL);
}

// Adapter path: native BOOLEAN bind against a real Pg column ---------------
//
// The `users_typed.active` column is a real `BOOLEAN`. Sending an `i64` bind
// against it errors in Pg ("column active is of type boolean but expression
// is of type bigint"). The adapter path binds `bool` natively, so the INSERT
// succeeds and the round-trip returns the expected value.
use sqlite_diff_rs::DiffOps;

use diesel::backend::Backend;
use diesel::query_builder::AstPass;
use diesel::result::QueryResult;
use diesel::serialize::ToSql;
use diesel::sql_types::Bool;
use sqlite_diff_rs::{Adapter, Binder, DefaultBinder, Value};

const USERS_TYPED_DDL: &str = "\
CREATE TABLE users_typed (
    id     BIGINT PRIMARY KEY,
    active BOOLEAN NOT NULL
)";

#[derive(QueryableByName, Debug, PartialEq)]
struct UsersTypedRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Bool)]
    active: bool,
}

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

struct UsersTypedAdapter;

impl<DB, S, B> Adapter<DB, S, B> for UsersTypedAdapter
where
    DB: Backend
        + diesel::sql_types::HasSqlType<Bool>
        + diesel::sql_types::HasSqlType<BigInt>
        + diesel::sql_types::HasSqlType<Double>
        + diesel::sql_types::HasSqlType<Text>
        + diesel::sql_types::HasSqlType<Binary>,
    bool: ToSql<Bool, DB>,
    i64: ToSql<BigInt, DB>,
    f64: ToSql<Double, DB>,
    str: ToSql<Text, DB>,
    [u8]: ToSql<Binary, DB>,
    S: AsRef<str> + Sync,
    B: AsRef<[u8]> + Sync,
{
    fn column_name(&self, _table: &str, column_index: usize) -> &str {
        ["id", "active"][column_index]
    }

    fn bind<'a>(
        &self,
        table: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> diesel::result::QueryResult<Box<dyn Binder<DB> + Send + 'a>> {
        match (table, column_index, value) {
            ("users_typed", 1, Value::Integer(i)) => Ok(Box::new(BoolBinder(*i != 0))),
            _ => Ok(Box::new(DefaultBinder::from(value))),
        }
    }
}

#[test]
fn adapter_binds_bool_natively_against_pg_boolean_column() {
    let (_container, mut conn) = boot();
    conn.batch_execute(USERS_TYPED_DDL).unwrap();

    let schema = sqlite_diff_rs::SimpleTable::new(
        "users_typed",
        &[("id", "INTEGER"), ("active", "INTEGER")],
        &[0],
    );
    let patchset = sqlite_diff_rs::PatchSet::<sqlite_diff_rs::SimpleTable, String, Vec<u8>>::new()
        .insert(
            sqlite_diff_rs::Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 1_i64) // truthy
                .unwrap(),
        )
        .insert(
            sqlite_diff_rs::Insert::from(schema.clone())
                .set(0, 2_i64)
                .unwrap()
                .set(1, 0_i64) // falsy
                .unwrap(),
        );

    // Native bind through the adapter. If we were still emitting `CAST(? AS
    // boolean)` or naively binding as BigInt, this would error at execute time.
    let adapter = UsersTypedAdapter;
    for op in patchset
        .iter()
        .map(|op| op.with_adapter::<diesel::pg::Pg, _>(&adapter))
    {
        op.execute(&mut conn)
            .expect("execute adapter-bound op against Pg BOOLEAN column");
    }

    let rows: Vec<UsersTypedRow> = sql_query("SELECT id, active FROM users_typed ORDER BY id")
        .load(&mut conn)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, 1);
    assert!(rows[0].active);
    assert_eq!(rows[1].id, 2);
    assert!(!rows[1].active);
}

#[test]
fn adapter_full_crud_lifecycle_against_pg_boolean_column() {
    // Exercises every walk_ast/resolve code path against a real Pg BOOLEAN
    // column so any lockstep mismatch surfaces as a wrong row or a wrong
    // value after the round trip, not just a debug_query text mismatch.
    let (_container, mut conn) = boot();
    conn.batch_execute(USERS_TYPED_DDL).unwrap();

    let schema = sqlite_diff_rs::SimpleTable::new(
        "users_typed",
        &[("id", "INTEGER"), ("active", "INTEGER")],
        &[0],
    );
    let adapter = UsersTypedAdapter;

    // Step 1: INSERT two rows via adapter.
    let inserts = sqlite_diff_rs::PatchSet::<sqlite_diff_rs::SimpleTable, String, Vec<u8>>::new()
        .insert(
            sqlite_diff_rs::Insert::from(schema.clone())
                .set(0, 10_i64)
                .unwrap()
                .set(1, 1_i64)
                .unwrap(),
        )
        .insert(
            sqlite_diff_rs::Insert::from(schema.clone())
                .set(0, 20_i64)
                .unwrap()
                .set(1, 0_i64)
                .unwrap(),
        );
    for op in inserts
        .iter()
        .map(|op| op.with_adapter::<diesel::pg::Pg, _>(&adapter))
    {
        op.execute(&mut conn).unwrap();
    }

    // Step 2: UPDATE row id=10, flipping active to false.
    let updates = sqlite_diff_rs::PatchSet::<sqlite_diff_rs::SimpleTable, String, Vec<u8>>::new()
        .update(
            sqlite_diff_rs::PatchUpdate::<_, String, Vec<u8>>::from(schema.clone())
                .set(0, 10_i64) // PK slot -> WHERE
                .unwrap()
                .set(1, 0_i64) // active -> SET, native bool
                .unwrap(),
        );
    for op in updates
        .iter()
        .map(|op| op.with_adapter::<diesel::pg::Pg, _>(&adapter))
    {
        op.execute(&mut conn).unwrap();
    }

    // Step 3: DELETE row id=20.
    let deletes =
        sqlite_diff_rs::PatchSet::<sqlite_diff_rs::SimpleTable, String, Vec<u8>>::new().delete(
            sqlite_diff_rs::PatchDelete::new(schema.clone(), vec![20_i64.into()]),
        );
    for op in deletes
        .iter()
        .map(|op| op.with_adapter::<diesel::pg::Pg, _>(&adapter))
    {
        op.execute(&mut conn).unwrap();
    }

    // Verify: one row left, id=10, active=false.
    let rows: Vec<UsersTypedRow> = sql_query("SELECT id, active FROM users_typed ORDER BY id")
        .load(&mut conn)
        .unwrap();
    assert_eq!(rows.len(), 1, "one row should remain after INSERTs+DELETE");
    assert_eq!(rows[0].id, 10);
    assert!(
        !rows[0].active,
        "UPDATE should have flipped active to false"
    );
}
