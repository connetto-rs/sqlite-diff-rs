//! SQLite E2E: build a patchset, iterate over `PatchsetOp`, execute each via
//! Diesel, then read the state back and assert.

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Text};
use sqlite_diff_rs::DiffOps;

use diesel_e2e::{KvRow, UserRow, duplicate_email_patchset, insert_three_users, users_schema};

const USERS_DDL: &str = "\
CREATE TABLE users (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    email   TEXT,
    score   REAL
)";

const BLOBS_DDL: &str = "\
CREATE TABLE blobs (
    id      INTEGER PRIMARY KEY,
    payload BLOB
)";

const KV_DDL: &str = "\
CREATE TABLE kv (
    tenant_id INTEGER NOT NULL,
    user_id   INTEGER NOT NULL,
    value     TEXT,
    PRIMARY KEY (tenant_id, user_id)
)";

fn conn() -> SqliteConnection {
    SqliteConnection::establish(":memory:").expect("open in-memory SQLite")
}

fn apply(
    conn: &mut SqliteConnection,
    patchset: &sqlite_diff_rs::PatchSet<sqlite_diff_rs::SimpleTable, String, Vec<u8>>,
) {
    for op in patchset.iter() {
        op.execute(conn).expect("execute patchset op");
    }
}

// ---------------------------------------------------------------------------

#[test]
fn users_insert_update_delete_full_cycle() {
    let mut conn = conn();
    diesel_e2e::users_full_cycle(&mut conn, USERS_DDL);
}

#[test]
fn blobs_null_and_raw_bytes_roundtrip() {
    let mut conn = conn();
    diesel_e2e::blobs_roundtrip(&mut conn, BLOBS_DDL);
}

#[test]
fn kv_composite_pk_lifecycle() {
    let mut conn = conn();
    diesel_e2e::kv_lifecycle(&mut conn, KV_DDL);
}

#[test]
fn kv_composite_primary_key_move_via_changeset() {
    use sqlite_diff_rs::{ChangeSet, ChangeUpdate, SimpleTable};

    let mut conn = conn();
    conn.batch_execute(KV_DDL).unwrap();
    conn.batch_execute("INSERT INTO kv (tenant_id, user_id, value) VALUES (1, 20, 'orig')")
        .unwrap();

    // Move the row from composite key (1, 20) to (2, 99) and change its value.
    // A patchset has no slot for the new key, so this is changeset-only.
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

    for op in changeset.iter() {
        op.execute(&mut conn).expect("execute changeset op");
    }

    let rows: Vec<KvRow> =
        sql_query("SELECT tenant_id, user_id, value FROM kv ORDER BY tenant_id, user_id")
            .load(&mut conn)
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tenant_id, 2);
    assert_eq!(rows[0].user_id, 99);
    assert_eq!(rows[0].value.as_deref(), Some("moved"));
}

#[test]
fn identifier_with_embedded_quotes_survives_execution() {
    // SQLite tolerates any identifier when double-quoted. Diesel's identifier
    // quoter doubles embedded quotes; the resulting DDL and DML must match.
    #[derive(QueryableByName)]
    struct One {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }

    let mut conn = conn();
    conn.batch_execute(r#"CREATE TABLE "we""ird" ("i""d" INTEGER PRIMARY KEY, "va""l" TEXT)"#)
        .unwrap();

    let schema = sqlite_diff_rs::SimpleTable::with_rowid_alias(
        "we\"ird",
        &[("i\"d", "INTEGER"), ("va\"l", "TEXT")],
        0,
    );
    let patchset = sqlite_diff_rs::PatchSet::<_, String, Vec<u8>>::new().insert(
        sqlite_diff_rs::Insert::from(schema.clone())
            .set(0, 1_i64)
            .unwrap()
            .set(1, "hello")
            .unwrap(),
    );
    apply(&mut conn, &patchset);

    let count: One = sql_query(r#"SELECT COUNT(*) AS n FROM "we""ird""#)
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(count.n, 1);
}

// ApplyOps trait: apply()/apply_transactional() atomicity ------------------

use sqlite_diff_rs::ApplyOps;

/// DDL with a UNIQUE constraint we can trigger without also tripping the
/// patchset PK consolidation (which would drop the duplicate at build time).
const UNIQ_USERS_DDL: &str = "\
CREATE TABLE users (
    id      INTEGER PRIMARY KEY,
    email   TEXT NOT NULL UNIQUE
)";

#[derive(QueryableByName, Debug, PartialEq)]
struct UniqUserRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    email: String,
}

/// `apply` (no transaction) commits everything up to a failing op, so state
/// after a mid-op failure is partial.
#[test]
fn apply_without_transaction_leaves_partial_state_on_failure() {
    let mut conn = conn();
    conn.batch_execute(UNIQ_USERS_DDL).unwrap();

    let patchset = duplicate_email_patchset();
    let result = patchset.iter().apply(&mut conn);
    assert!(
        result.is_err(),
        "expected UNIQUE violation on second insert"
    );

    let rows: Vec<UniqUserRow> = sql_query("SELECT id, email FROM users ORDER BY id")
        .load(&mut conn)
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "first insert should have committed before the failing second one"
    );
    assert_eq!(rows[0].id, 1);
}

/// `apply_transactional` rolls the whole batch back atomically on any mid-op
/// failure, so after a failure the DB is unchanged.
#[test]
fn apply_transactional_rolls_back_all_ops_on_mid_batch_failure() {
    let mut conn = conn();
    conn.batch_execute(UNIQ_USERS_DDL).unwrap();

    let patchset = duplicate_email_patchset();
    let result = patchset.iter().apply_transactional(&mut conn);
    assert!(result.is_err(), "expected UNIQUE violation, batch aborted");

    let rows: Vec<UniqUserRow> = sql_query("SELECT id, email FROM users")
        .load(&mut conn)
        .unwrap();
    assert!(
        rows.is_empty(),
        "transactional apply must roll back the first insert too, found: {rows:?}"
    );
}

/// Happy path: `apply_transactional` returns the summed affected-row count.
#[test]
fn apply_transactional_returns_summed_row_count() {
    let mut conn = conn();
    conn.batch_execute(USERS_DDL).unwrap();

    let schema = users_schema();
    let patchset = insert_three_users(&schema);

    let n = patchset.iter().apply_transactional(&mut conn).unwrap();
    assert_eq!(n, 3, "three inserts should report three affected rows");

    let rows: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
        .load(&mut conn)
        .unwrap();
    assert_eq!(rows.len(), 3);
}

// Adapter that rejects the mapping ---------------------------------------
//
// A real `.execute()` against a live connection must surface the adapter's
// bind error rather than silently applying, and the transaction must roll
// back so nothing lands in the DB.

use diesel::sqlite::Sqlite;
use sqlite_diff_rs::{Adapter, Binder, DefaultBinder, Value};

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
    ) -> diesel::result::QueryResult<Box<dyn Binder<Sqlite> + Send + 'a>> {
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
fn adapter_bind_error_propagates_through_execute() {
    let mut conn = conn();
    conn.batch_execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, session TEXT NOT NULL)")
        .unwrap();

    // Column 1 has Integer value, but the adapter models it as a UUID column
    // and rejects the mapping.
    let schema = sqlite_diff_rs::SimpleTable::with_rowid_alias(
        "accounts",
        &[("id", "INTEGER"), ("session", "TEXT")],
        0,
    );
    let patchset = sqlite_diff_rs::PatchSet::<sqlite_diff_rs::SimpleTable, String, Vec<u8>>::new()
        .insert(
            sqlite_diff_rs::Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 42_i64)
                .unwrap(),
        );

    let op = patchset
        .iter()
        .next()
        .unwrap()
        .with_adapter::<Sqlite, _>(&SessionAdapter);

    let result = op.execute(&mut conn);
    let err = result.expect_err("execute should fail because the adapter rejected the bind");

    // The error surfaces as a QueryBuilderError wrapping the adapter's
    // message via Display.
    match err {
        diesel::result::Error::QueryBuilderError(inner) => {
            let msg = inner.to_string();
            assert!(
                msg.contains("cannot bind Integer to UUID column")
                    || msg.contains("adapter rejected"),
                "unexpected error message: {msg}"
            );
        }
        other => panic!("expected QueryBuilderError, got {other:?}"),
    }

    // Nothing landed in the DB.
    let count: One = sql_query("SELECT COUNT(*) AS n FROM accounts")
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(count.n, 0, "no rows should have been inserted");
}

// Reused typed row for the count assertion above.
#[derive(QueryableByName)]
struct One {
    #[diesel(sql_type = BigInt)]
    n: i64,
}
