//! Backend-agnostic helpers for the E2E test suite.
//!
//! The schema and patchset builders below return fully constructed
//! [`SimpleTable`] and [`PatchSet`] values. The per-backend test files keep
//! the DDL and container startup. Scenario application, row loading, and
//! verification happen here, generic over the diesel connection, with async
//! twins over `diesel_async`.

use diesel::QueryableByName;
use diesel::sql_types::{BigInt, Binary, Double, Nullable, Text};
use sqlite_diff_rs::{DiffOps, Insert, PatchDelete, PatchSet, PatchUpdate, SimpleTable};

/// A `users` row as loaded by `sql_query`.
#[derive(QueryableByName, Debug, PartialEq)]
pub struct UserRow {
    /// `id` column.
    #[diesel(sql_type = BigInt)]
    pub id: i64,
    /// `name` column.
    #[diesel(sql_type = Text)]
    pub name: String,
    /// `email` column.
    #[diesel(sql_type = Nullable<Text>)]
    pub email: Option<String>,
    /// `score` column.
    #[diesel(sql_type = Nullable<Double>)]
    pub score: Option<f64>,
}

/// A `blobs` row as loaded by `sql_query`.
#[derive(QueryableByName, Debug, PartialEq)]
pub struct BlobRow {
    /// `id` column.
    #[diesel(sql_type = BigInt)]
    pub id: i64,
    /// `payload` column.
    #[diesel(sql_type = Nullable<Binary>)]
    pub payload: Option<Vec<u8>>,
}

/// A `kv` row as loaded by `sql_query`.
#[derive(QueryableByName, Debug, PartialEq)]
pub struct KvRow {
    /// `tenant_id` column.
    #[diesel(sql_type = BigInt)]
    pub tenant_id: i64,
    /// `user_id` column.
    #[diesel(sql_type = BigInt)]
    pub user_id: i64,
    /// `value` column.
    #[diesel(sql_type = Nullable<Text>)]
    pub value: Option<String>,
}

// The sync and async scenario helpers live in separate modules because
// `diesel::RunQueryDsl` and `diesel_async::RunQueryDsl` are both blanket
// traits and cannot share one import scope without making `.load` ambiguous.
mod sync {
    use diesel::query_builder::{QueryFragment, QueryId, SqlQuery};
    use diesel::query_dsl::LoadQuery;
    use diesel::sql_query;
    use diesel::{Connection, RunQueryDsl};
    use sqlite_diff_rs::{ApplyOps, PatchsetOp, SimpleTable};

    use crate::{
        BlobRow, KvRow, UserRow, blobs_schema, insert_blob_row, insert_three_users, kv_full_cycle,
        kv_schema, update_alice_delete_bob, users_schema,
    };

    /// Create the `users` table, insert three users, then apply the
    /// update/delete patchset, verifying the table after each step.
    pub fn users_full_cycle<Conn>(conn: &mut Conn, ddl: &str)
    where
        Conn: Connection,
        SqlQuery: LoadQuery<'static, Conn, UserRow>,
        for<'a> PatchsetOp<'a, SimpleTable, String, Vec<u8>>:
            QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>,
    {
        conn.batch_execute(ddl).unwrap();

        let schema = users_schema();
        assert_eq!(insert_three_users(&schema).iter().apply(conn).unwrap(), 3);

        let rows: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
            .load(conn)
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[0].name, "Alice");
        assert_eq!(rows[0].email.as_deref(), Some("alice@example.com"));
        assert_eq!(rows[0].score, Some(95.5));
        assert_eq!(rows[1].id, 2);
        assert_eq!(rows[1].email, None);
        assert_eq!(rows[2].name, "Carol'); DROP TABLE users; --");
        assert_eq!(rows[2].score, None);

        update_alice_delete_bob(&schema).iter().apply(conn).unwrap();

        let after: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
            .load(conn)
            .unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, 1);
        assert_eq!(after[0].name, "Alice");
        assert_eq!(after[0].email.as_deref(), Some("alice+new@example.com"));
        assert_eq!(after[0].score, Some(99.0));
        assert_eq!(after[1].id, 3);
    }

    /// Create the `blobs` table, insert the raw-bytes row, and verify the
    /// payload round-trips byte for byte.
    pub fn blobs_roundtrip<Conn>(conn: &mut Conn, ddl: &str)
    where
        Conn: Connection,
        SqlQuery: LoadQuery<'static, Conn, BlobRow>,
        for<'a> PatchsetOp<'a, SimpleTable, String, Vec<u8>>:
            QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>,
    {
        conn.batch_execute(ddl).unwrap();

        let schema = blobs_schema();
        insert_blob_row(&schema).iter().apply(conn).unwrap();

        let rows: Vec<BlobRow> = sql_query("SELECT id, payload FROM blobs ORDER BY id")
            .load(conn)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 1);
        assert_eq!(
            rows[0].payload.as_deref(),
            Some(&[0x00, b'\'', 0x7F, 0x80, 0xFE, 0xFF][..])
        );
    }

    /// Create the `kv` table, run the composite-PK insert/update/delete
    /// cycle, and verify the surviving row.
    pub fn kv_lifecycle<Conn>(conn: &mut Conn, ddl: &str)
    where
        Conn: Connection,
        SqlQuery: LoadQuery<'static, Conn, KvRow>,
        for<'a> PatchsetOp<'a, SimpleTable, String, Vec<u8>>:
            QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>,
    {
        conn.batch_execute(ddl).unwrap();

        let schema = kv_schema();
        kv_full_cycle(&schema).iter().apply(conn).unwrap();

        let rows: Vec<KvRow> =
            sql_query("SELECT tenant_id, user_id, value FROM kv ORDER BY tenant_id, user_id")
                .load(conn)
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tenant_id, 1);
        assert_eq!(rows[0].user_id, 20);
        assert_eq!(rows[0].value.as_deref(), Some("two-updated"));
    }
}

mod async_ {
    use diesel::query_builder::{QueryFragment, QueryId, SqlQuery};
    use diesel::sql_query;
    use diesel_async::{AsyncConnection, RunQueryDsl};
    use sqlite_diff_rs::{ApplyOpsAsync, PatchsetOp, SimpleTable};

    use crate::{
        KvRow, UserRow, insert_three_users, kv_full_cycle, kv_schema, update_alice_delete_bob,
        users_schema,
    };

    /// Async twin of [`crate::users_full_cycle`].
    pub async fn users_full_cycle_async<Conn>(conn: &mut Conn, ddl: &str)
    where
        Conn: AsyncConnection,
        SqlQuery: diesel_async::methods::LoadQuery<'static, Conn, UserRow>,
        for<'a> PatchsetOp<'a, SimpleTable, String, Vec<u8>>:
            QueryFragment<Conn::Backend> + QueryId + Send,
    {
        conn.batch_execute(ddl).await.unwrap();

        let schema = users_schema();
        assert_eq!(
            insert_three_users(&schema)
                .iter()
                .apply_async(conn)
                .await
                .unwrap(),
            3
        );

        let rows: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
            .load(conn)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[0].name, "Alice");
        assert_eq!(rows[0].email.as_deref(), Some("alice@example.com"));
        assert_eq!(rows[0].score, Some(95.5));
        assert_eq!(rows[1].id, 2);
        assert_eq!(rows[1].email, None);
        assert_eq!(rows[2].name, "Carol'); DROP TABLE users; --");
        assert_eq!(rows[2].score, None);

        update_alice_delete_bob(&schema)
            .iter()
            .apply_transactional_async(conn)
            .await
            .unwrap();

        let after: Vec<UserRow> = sql_query("SELECT id, name, email, score FROM users ORDER BY id")
            .load(conn)
            .await
            .unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, 1);
        assert_eq!(after[0].name, "Alice");
        assert_eq!(after[0].email.as_deref(), Some("alice+new@example.com"));
        assert_eq!(after[0].score, Some(99.0));
        assert_eq!(after[1].id, 3);
    }

    /// Async twin of [`crate::kv_lifecycle`].
    pub async fn kv_lifecycle_async<Conn>(conn: &mut Conn, ddl: &str)
    where
        Conn: AsyncConnection,
        SqlQuery: diesel_async::methods::LoadQuery<'static, Conn, KvRow>,
        for<'a> PatchsetOp<'a, SimpleTable, String, Vec<u8>>:
            QueryFragment<Conn::Backend> + QueryId + Send,
    {
        conn.batch_execute(ddl).await.unwrap();

        let schema = kv_schema();
        kv_full_cycle(&schema)
            .iter()
            .apply_async(conn)
            .await
            .unwrap();

        let rows: Vec<KvRow> =
            sql_query("SELECT tenant_id, user_id, value FROM kv ORDER BY tenant_id, user_id")
                .load(conn)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tenant_id, 1);
        assert_eq!(rows[0].user_id, 20);
        assert_eq!(rows[0].value.as_deref(), Some("two-updated"));
    }
}

pub use async_::kv_lifecycle_async;
pub use async_::users_full_cycle_async;
pub use sync::blobs_roundtrip;
pub use sync::kv_lifecycle;
pub use sync::users_full_cycle;

/// `users(id INT PK, name TEXT, email TEXT NULL, score REAL NULL)`.
#[must_use]
pub fn users_schema() -> SimpleTable {
    SimpleTable::with_rowid_alias(
        "users",
        &[
            ("id", "INTEGER"),
            ("name", "TEXT"),
            ("email", "TEXT"),
            ("score", "REAL"),
        ],
        0,
    )
}

/// `blobs(id INT PK, payload BLOB NULL)`.
#[must_use]
pub fn blobs_schema() -> SimpleTable {
    SimpleTable::with_rowid_alias("blobs", &[("id", "INTEGER"), ("payload", "BLOB")], 0)
}

/// `kv(tenant_id INT, user_id INT, value TEXT NULL, PRIMARY KEY(tenant_id, user_id))`.
#[must_use]
pub fn kv_schema() -> SimpleTable {
    SimpleTable::new(
        "kv",
        &[
            ("tenant_id", "INTEGER"),
            ("user_id", "INTEGER"),
            ("value", "TEXT"),
        ],
        &[0, 1],
    )
}

/// Insert three users covering every scalar `Value` variant:
///
/// * `id=1` Alice, real score, non-null email.
/// * `id=2` Bob, null email, real score.
/// * `id=3` "Carol'); DROP TABLE users; --" (injection payload), null score.
#[must_use]
pub fn insert_three_users(schema: &SimpleTable) -> PatchSet<SimpleTable, String, Vec<u8>> {
    PatchSet::<SimpleTable, String, Vec<u8>>::new()
        .insert(
            Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, "Alice")
                .unwrap()
                .set(2, "alice@example.com")
                .unwrap()
                .set(3, 95.5_f64)
                .unwrap(),
        )
        .insert(
            Insert::from(schema.clone())
                .set(0, 2_i64)
                .unwrap()
                .set(1, "Bob")
                .unwrap()
                .set_null(2)
                .unwrap()
                .set(3, 87.0_f64)
                .unwrap(),
        )
        .insert(
            Insert::from(schema.clone())
                .set(0, 3_i64)
                .unwrap()
                .set(1, "Carol'); DROP TABLE users; --")
                .unwrap()
                .set(2, "carol@example.com")
                .unwrap()
                .set_null(3)
                .unwrap(),
        )
}

/// Update Alice's email and score, delete Bob, leave Carol untouched.
#[must_use]
pub fn update_alice_delete_bob(schema: &SimpleTable) -> PatchSet<SimpleTable, String, Vec<u8>> {
    PatchSet::<SimpleTable, String, Vec<u8>>::new()
        .update(
            PatchUpdate::<_, String, Vec<u8>>::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(2, "alice+new@example.com")
                .unwrap()
                .set(3, 99.0_f64)
                .unwrap(),
        )
        .delete(PatchDelete::new(schema.clone(), vec![2_i64.into()]))
}

/// Insert a single blob row with a raw byte payload including `NUL`, `0xFF`,
/// and single-quote characters.
#[must_use]
pub fn insert_blob_row(schema: &SimpleTable) -> PatchSet<SimpleTable, String, Vec<u8>> {
    let payload = vec![0x00_u8, b'\'', 0x7F, 0x80, 0xFE, 0xFF];
    PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(schema.clone())
            .set(0, 1_i64)
            .unwrap()
            .set(1, payload)
            .unwrap(),
    )
}

/// Insert two `kv` rows with distinct composite PKs, update the second, and
/// delete the first.
#[must_use]
pub fn kv_full_cycle(schema: &SimpleTable) -> PatchSet<SimpleTable, String, Vec<u8>> {
    PatchSet::<SimpleTable, String, Vec<u8>>::new()
        .insert(
            Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 10_i64)
                .unwrap()
                .set(2, "one")
                .unwrap(),
        )
        .insert(
            Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 20_i64)
                .unwrap()
                .set(2, "two")
                .unwrap(),
        )
        .update(
            PatchUpdate::<_, String, Vec<u8>>::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, 20_i64)
                .unwrap()
                .set(2, "two-updated")
                .unwrap(),
        )
        .delete(PatchDelete::new(
            schema.clone(),
            vec![1_i64.into(), 10_i64.into()],
        ))
}

/// Two `users` rows with distinct primary keys but a colliding `UNIQUE`
/// email, so applying the second insert violates the constraint. Used to
/// exercise transactional rollback and non-transactional partial-apply.
#[must_use]
pub fn duplicate_email_patchset() -> PatchSet<SimpleTable, String, Vec<u8>> {
    let schema = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("email", "TEXT")], 0);
    PatchSet::<SimpleTable, String, Vec<u8>>::new()
        .insert(
            Insert::from(schema.clone())
                .set(0, 1_i64)
                .unwrap()
                .set(1, "dup@example.com")
                .unwrap(),
        )
        .insert(
            Insert::from(schema.clone())
                .set(0, 2_i64)
                .unwrap()
                .set(1, "dup@example.com")
                .unwrap(),
        )
}
