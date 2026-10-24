//! SQLite async E2E: drive the same patchsets through `diesel-async` over a
//! `SyncConnectionWrapper<SqliteConnection>` (in-memory, no container), then
//! read the state back and assert. Exercises `ApplyOpsAsync::apply_async` and
//! `apply_transactional_async` end to end against a real database.
//!
//! Only the async `diesel_async::RunQueryDsl` is imported (no
//! `diesel::prelude::*` glob), so `.load`/`.execute` resolve unambiguously to
//! the async connection.

use diesel::prelude::QueryableByName;
use diesel::result::QueryResult;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::{Sqlite, SqliteConnection};
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use diesel_async::{AsyncConnection, RunQueryDsl, SimpleAsyncConnection};
use diesel_e2e::{
    KvRow, UserRow, duplicate_email_patchset, insert_three_users, kv_full_cycle, kv_schema,
    users_schema,
};
use sqlite_diff_rs::{
    Adapter, ApplyOpsAsync, Binder, DefaultBinder, DiffOps, Insert, PatchSet, SimpleTable, Value,
};

type Db = SyncConnectionWrapper<SqliteConnection>;

const USERS_DDL: &str = "\
CREATE TABLE users (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    email   TEXT,
    score   REAL
)";

const KV_DDL: &str = "\
CREATE TABLE kv (
    tenant_id INTEGER NOT NULL,
    user_id   INTEGER NOT NULL,
    value     TEXT,
    PRIMARY KEY (tenant_id, user_id)
)";

/// A UNIQUE column we can collide on without tripping patchset PK
/// consolidation (which would drop a duplicate PK at build time).
const UNIQ_USERS_DDL: &str = "\
CREATE TABLE users (
    id      INTEGER PRIMARY KEY,
    email   TEXT NOT NULL UNIQUE
)";

#[derive(QueryableByName, Debug, PartialEq)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

async fn conn() -> Db {
    Db::establish(":memory:")
        .await
        .expect("open in-memory SQLite through the sync wrapper")
}

#[tokio::test]
async fn users_insert_update_delete_full_cycle_async() {
    let mut conn = conn().await;
    diesel_e2e::users_full_cycle_async(&mut conn, USERS_DDL).await;
}

#[tokio::test]
async fn kv_composite_pk_lifecycle_transactional_async() {
    let mut conn = conn().await;
    conn.batch_execute(KV_DDL).await.unwrap();

    let schema = kv_schema();
    let patchset = kv_full_cycle(&schema);
    // Consolidation collapses the batch before apply: insert+delete of
    // (1, 10) cancels, and insert+update of (1, 20) merges into a single
    // INSERT, so exactly one row is affected.
    let count = patchset
        .iter()
        .apply_transactional_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let rows: Vec<KvRow> =
        sql_query("SELECT tenant_id, user_id, value FROM kv ORDER BY tenant_id, user_id")
            .load(&mut conn)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tenant_id, 1);
    assert_eq!(rows[0].user_id, 20);
    assert_eq!(rows[0].value.as_deref(), Some("two-updated"));
}

#[tokio::test]
async fn apply_transactional_async_rolls_back_on_mid_batch_failure() {
    let mut conn = conn().await;
    conn.batch_execute(UNIQ_USERS_DDL).await.unwrap();

    let patchset = duplicate_email_patchset();
    let result = patchset.iter().apply_transactional_async(&mut conn).await;
    assert!(result.is_err(), "UNIQUE violation must surface as an error");

    // The transaction rolled the whole batch back, so no rows landed.
    let rows: Vec<CountRow> = sql_query("SELECT COUNT(*) AS n FROM users")
        .load(&mut conn)
        .await
        .unwrap();
    assert_eq!(rows[0].n, 0);
}

// Adapter path: identifiers and binders come from an `Adapter`, which drives
// the async `BoundOp` execution path (distinct from the naive `PatchsetOp`
// path exercised above).

struct UsersAdapter;

impl<S: AsRef<str> + Sync, B: AsRef<[u8]> + Sync> Adapter<Sqlite, S, B> for UsersAdapter {
    fn column_name(&self, _table: &str, index: usize) -> &str {
        ["id", "name", "email", "score"][index]
    }

    fn bind<'a>(
        &self,
        _table: &str,
        _column_index: usize,
        value: &'a Value<S, B>,
    ) -> QueryResult<Box<dyn Binder<Sqlite> + Send + 'a>> {
        Ok(Box::new(DefaultBinder::from(value)))
    }
}

/// Rejects binding an `Integer` to column 1 (modeling a UUID column), so the
/// async execute must surface the adapter error.
struct SessionAdapter;

impl<S: AsRef<str> + Sync, B: AsRef<[u8]> + Sync> Adapter<Sqlite, S, B> for SessionAdapter {
    fn column_name(&self, _table: &str, index: usize) -> &str {
        ["id", "session"][index]
    }

    fn bind<'a>(
        &self,
        _table: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> QueryResult<Box<dyn Binder<Sqlite> + Send + 'a>> {
        if column_index == 1 && matches!(value, Value::Integer(_)) {
            return Err(diesel::result::Error::QueryBuilderError(
                "cannot bind Integer to UUID column".into(),
            ));
        }
        Ok(Box::new(DefaultBinder::from(value)))
    }
}

#[tokio::test]
async fn adapter_path_applies_via_apply_async() {
    let mut conn = conn().await;
    conn.batch_execute(USERS_DDL).await.unwrap();

    let schema = users_schema();
    let patchset = insert_three_users(&schema);
    let count = patchset
        .iter()
        .map(|op| op.with_adapter::<Sqlite, _>(&UsersAdapter))
        .apply_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(count, 3);

    let rows: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
        .load(&mut conn)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].email.as_deref(), Some("alice@example.com"));
    assert_eq!(rows[2].name, "Carol'); DROP TABLE users; --");
}

#[tokio::test]
async fn adapter_bind_error_surfaces_through_apply_async() {
    let mut conn = conn().await;
    conn.batch_execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, session TEXT NOT NULL)")
        .await
        .unwrap();

    // Column 1 carries an Integer, but the adapter models it as a UUID column
    // and rejects the mapping; the async execute must fail and land nothing.
    let schema =
        SimpleTable::with_rowid_alias("accounts", &[("id", "INTEGER"), ("session", "TEXT")], 0);
    let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(schema)
            .set(0, 1_i64)
            .unwrap()
            .set(1, 42_i64)
            .unwrap(),
    );

    let result = patchset
        .iter()
        .map(|op| op.with_adapter::<Sqlite, _>(&SessionAdapter))
        .apply_async(&mut conn)
        .await;
    assert!(
        result.is_err(),
        "adapter rejection must surface as an error"
    );

    let rows: Vec<CountRow> = sql_query("SELECT COUNT(*) AS n FROM accounts")
        .load(&mut conn)
        .await
        .unwrap();
    assert_eq!(rows[0].n, 0);
}

#[tokio::test]
async fn changeset_composite_pk_move_via_apply_async() {
    use sqlite_diff_rs::{ChangeSet, ChangeUpdate};

    let mut conn = conn().await;
    conn.batch_execute(KV_DDL).await.unwrap();
    conn.batch_execute("INSERT INTO kv (tenant_id, user_id, value) VALUES (1, 20, 'orig')")
        .await
        .unwrap();

    // Move (1, 20) -> (2, 99) and change the value. A patchset has no slot for
    // the new key, so this is changeset-only and exercises `ChangesetOp` async.
    let schema = SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    );
    let changeset = ChangeSet::<SimpleTable, String, Vec<u8>>::new().update(
        ChangeUpdate::<_, String, Vec<u8>>::from(schema)
            .set(0, 1_i64, 2_i64)
            .unwrap()
            .set(1, 20_i64, 99_i64)
            .unwrap()
            .set(2, "orig", "moved")
            .unwrap(),
    );
    let count = changeset.iter().apply_async(&mut conn).await.unwrap();
    assert_eq!(count, 1);

    let rows: Vec<KvRow> =
        sql_query("SELECT tenant_id, user_id, value FROM kv ORDER BY tenant_id, user_id")
            .load(&mut conn)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tenant_id, 2);
    assert_eq!(rows[0].user_id, 99);
    assert_eq!(rows[0].value.as_deref(), Some("moved"));
}
