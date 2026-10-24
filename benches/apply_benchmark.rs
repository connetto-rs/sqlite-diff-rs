#![allow(clippy::unreadable_literal)] // Test fixture IDs are more readable without separators

//! Benchmark measuring the time to apply changes to an `SQLite` database.
//!
//! Four application methods are compared: raw SQL via rusqlite with
//! autocommit per statement (`sql`), the same SQL wrapped in a single
//! transaction (`sql_tx`), a patchset applied via `conn.apply_strm()`, and a
//! changeset applied via `conn.apply_strm()`.
//!
//! Each method runs against two starting states (`empty`: schema only,
//! `populated`: 1000+ rows per table), against two primary-key strategies
//! (`int_pk`: sequential INTEGER, `uuid_pk`: `UUIDv7` stored as 16-byte
//! BLOB), and across batch sizes of 30, 100, and 1000 operations mixing
//! INSERT, UPDATE, and DELETE.
//!
//! On the `populated/1000` scenario the schema is also varied across four
//! configurations: `base` (no secondary indexes, no triggers, foreign keys
//! off), `indexed` (secondary indexes on FK and query columns), `triggers`
//! (audit-log triggers on the `users` table), and `fk`
//! (`PRAGMA foreign_keys=ON`).

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rusqlite::Connection;
use rusqlite::params;
use rusqlite::session::Session;
use rusqlite::types::{ToSqlOutput, ValueRef};
use std::fmt;
use std::io::Cursor;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Schema template
// ---------------------------------------------------------------------------

/// Schema DDL template. Every occurrence of `$ID` is replaced with the
/// concrete column type (`INTEGER` or `BLOB`) at runtime.
const SCHEMA_TEMPLATE: &str = "
CREATE TABLE users (
    id $ID PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_login INTEGER,
    is_active INTEGER NOT NULL DEFAULT 1,
    profile_data BLOB
);

CREATE TABLE posts (
    id $ID PRIMARY KEY,
    user_id $ID NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER,
    view_count INTEGER NOT NULL DEFAULT 0,
    is_published INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE TABLE comments (
    id $ID PRIMARY KEY,
    post_id $ID NOT NULL,
    user_id $ID NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    parent_id $ID,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (post_id) REFERENCES posts(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE TABLE tags (
    id $ID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE post_tags (
    post_id $ID NOT NULL,
    tag_id $ID NOT NULL,
    PRIMARY KEY (post_id, tag_id),
    FOREIGN KEY (post_id) REFERENCES posts(id),
    FOREIGN KEY (tag_id) REFERENCES tags(id)
);
";

/// Instantiate the schema DDL with a concrete ID column type.
fn make_schema(id_type: &str) -> String {
    SCHEMA_TEMPLATE.replace("$ID", id_type)
}

/// Secondary indexes DDL: realistic indexes on FK and query columns.
const INDEXES_DDL: &str = "
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_created_at ON users(created_at);
CREATE INDEX idx_posts_user_id ON posts(user_id);
CREATE INDEX idx_posts_created_at ON posts(created_at);
CREATE INDEX idx_comments_post_id ON comments(post_id);
CREATE INDEX idx_comments_user_id ON comments(user_id);
";

/// Trigger DDL template: audit-log triggers on the `users` table.
/// Uses `$ID` placeholder so the log table's `row_id` column matches the PK type.
const TRIGGER_DDL_TEMPLATE: &str = "
CREATE TABLE changes_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT NOT NULL,
    op TEXT NOT NULL,
    row_id $ID,
    ts INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TRIGGER trg_users_insert AFTER INSERT ON users BEGIN
    INSERT INTO changes_log (table_name, op, row_id) VALUES ('users', 'INSERT', NEW.id);
END;

CREATE TRIGGER trg_users_update AFTER UPDATE ON users BEGIN
    INSERT INTO changes_log (table_name, op, row_id) VALUES ('users', 'UPDATE', NEW.id);
END;

CREATE TRIGGER trg_users_delete AFTER DELETE ON users BEGIN
    INSERT INTO changes_log (table_name, op, row_id) VALUES ('users', 'DELETE', OLD.id);
END;
";

// ---------------------------------------------------------------------------
// Database configuration
// ---------------------------------------------------------------------------

/// Configuration for DB schema variants benchmarked.
struct DbConfig {
    /// Short label used in benchmark group names.
    label: &'static str,
    /// Add secondary indexes on FK and query columns.
    indexes: bool,
    /// Add audit-log triggers on `users`.
    triggers: bool,
    /// Enable `PRAGMA foreign_keys=ON`.
    foreign_keys: bool,
}

/// The set of configurations to benchmark.
const CONFIGS: &[DbConfig] = &[
    DbConfig {
        label: "base",
        indexes: false,
        triggers: false,
        foreign_keys: false,
    },
    DbConfig {
        label: "indexed",
        indexes: true,
        triggers: false,
        foreign_keys: false,
    },
    DbConfig {
        label: "triggers",
        indexes: false,
        triggers: true,
        foreign_keys: false,
    },
    DbConfig {
        label: "fk",
        indexes: false,
        triggers: false,
        foreign_keys: true,
    },
];

/// The five data tables whose changes we track in the session.
const DATA_TABLES: &[&str] = &["users", "posts", "comments", "tags", "post_tags"];

// ---------------------------------------------------------------------------
// ID abstraction
// ---------------------------------------------------------------------------

/// Primary-key strategy.
#[derive(Clone, Copy)]
enum IdKind {
    /// Sequential integer IDs (1, 2, 3, ...).
    Integer,
    /// `UUIDv7` stored as 16-byte BLOB.
    Uuid,
}

impl IdKind {
    /// The `SQLite` column type that replaces `$ID` in the schema template.
    fn sql_type(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Uuid => "BLOB",
        }
    }

    /// Short label used in benchmark group names.
    fn label(self) -> &'static str {
        match self {
            Self::Integer => "int_pk",
            Self::Uuid => "uuid_pk",
        }
    }
}

/// A primary or foreign key value: an integer or a 16-byte UUID blob.
#[derive(Clone, Copy)]
enum Id {
    Int(i64),
    Blob([u8; 16]),
}

impl fmt::Display for Id {
    /// Formats as a SQL literal: `42` or `X'0123...'`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Blob(b) => write!(f, "X'{}'", hex::encode(b)),
        }
    }
}

impl Id {
    /// Short human-readable label for embedding in generated string values.
    fn short_label(self) -> String {
        match self {
            Self::Int(n) => n.to_string(),
            Self::Blob(b) => hex::encode(&b[..4]),
        }
    }
}

impl rusqlite::types::ToSql for Id {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Self::Int(n) => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Integer(*n))),
            Self::Blob(b) => Ok(ToSqlOutput::Borrowed(ValueRef::Blob(b.as_slice()))),
        }
    }
}

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

/// Timestamp range for `UUIDv7`: 2000-01-01 to 2020-01-01 (ms since Unix epoch).
const TS_START_MS: u64 = 946_684_800_000;
const TS_END_MS: u64 = 1_577_836_800_000;

/// Generate `count` sorted `UUIDv7` values with timestamps sampled uniformly
/// between 2000 and 2020.
fn generate_uuidv7s(count: usize, rng: &mut StdRng) -> Vec<Uuid> {
    let mut uuids: Vec<Uuid> = (0..count)
        .map(|_| {
            let ms = rng.random_range(TS_START_MS..TS_END_MS);
            let secs = ms / 1000;
            let nanos = (ms % 1000) as u32 * 1_000_000;
            let ts = uuid::Timestamp::from_unix(uuid::NoContext, secs, nanos);
            Uuid::new_v7(ts)
        })
        .collect();
    uuids.sort();
    uuids
}

/// Generate `n` IDs of the given kind.
fn generate_ids(n: usize, kind: IdKind, rng: &mut StdRng) -> Vec<Id> {
    match kind {
        #[allow(clippy::cast_possible_wrap)]
        IdKind::Integer => (1..=n as i64).map(Id::Int).collect(),
        IdKind::Uuid => generate_uuidv7s(n, rng)
            .into_iter()
            .map(|u| Id::Blob(*u.as_bytes()))
            .collect(),
    }
}

/// Monotonic counter for generating new IDs during scenario generation.
struct IdCounter {
    kind: IdKind,
    next_int: i64,
}

impl IdCounter {
    fn new(kind: IdKind, existing_count: usize) -> Self {
        Self {
            kind,
            #[allow(clippy::cast_possible_wrap)]
            next_int: existing_count as i64 + 1,
        }
    }

    fn next(&mut self, rng: &mut StdRng) -> Id {
        match self.kind {
            IdKind::Integer => {
                let id = self.next_int;
                self.next_int += 1;
                Id::Int(id)
            }
            IdKind::Uuid => Id::Blob(*generate_uuidv7s(1, rng)[0].as_bytes()),
        }
    }
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// Create a fresh in-memory DB with the given schema DDL and config.
fn create_db(schema: &str, id_type: &str, config: &DbConfig) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    let fk = if config.foreign_keys { "ON" } else { "OFF" };
    conn.execute_batch(&format!(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys={fk};"
    ))
    .unwrap();
    conn.execute_batch(schema).unwrap();
    if config.indexes {
        conn.execute_batch(INDEXES_DDL).unwrap();
    }
    if config.triggers {
        let trigger_ddl = TRIGGER_DDL_TEMPLATE.replace("$ID", id_type);
        conn.execute_batch(&trigger_ddl).unwrap();
    }
    conn
}

/// Clone a database via the backup API (setup cost, excluded from timing).
/// Preserves the foreign-key setting from `config`.
fn clone_db(src: &Connection, config: &DbConfig) -> Connection {
    let mut dst = Connection::open_in_memory().unwrap();
    {
        let backup = rusqlite::backup::Backup::new(src, &mut dst).unwrap();
        backup
            .run_to_completion(100, std::time::Duration::ZERO, None)
            .unwrap();
    }
    let fk = if config.foreign_keys { "ON" } else { "OFF" };
    dst.execute_batch(&format!("PRAGMA foreign_keys={fk};"))
        .unwrap();
    dst
}

// ---------------------------------------------------------------------------
// ID pools
// ---------------------------------------------------------------------------

/// Per-entity collections of IDs, used for tracking population state and as a
/// pool to sample from during scenario generation.
struct IdPools {
    users: Vec<Id>,
    posts: Vec<Id>,
    comments: Vec<Id>,
    tags: Vec<Id>,
}

impl IdPools {
    fn empty() -> Self {
        Self {
            users: Vec::new(),
            posts: Vec::new(),
            comments: Vec::new(),
            tags: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Population
// ---------------------------------------------------------------------------

/// Populate the schema with `n` rows per table, returning the generated IDs.
fn populate_db(conn: &Connection, n: usize, kind: IdKind, rng: &mut StdRng) -> IdPools {
    let users = generate_ids(n, kind, rng);
    let posts = generate_ids(n, kind, rng);
    let comments = generate_ids(n, kind, rng);
    let tags = generate_ids(n, kind, rng);

    conn.execute_batch("BEGIN").unwrap();

    // users
    {
        let mut stmt = conn
            .prepare_cached(
                "INSERT INTO users (id, username, email, created_at, last_login, is_active) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .unwrap();
        for (i, id) in users.iter().enumerate() {
            let ts: i64 = rng.random_range(1000000..2000000);
            stmt.execute(params![
                *id,
                format!("user_{i}"),
                format!("user_{i}@example.com"),
                ts,
                Option::<i64>::None,
                1i64
            ])
            .unwrap();
        }
    }

    // posts
    {
        let mut stmt = conn
            .prepare_cached(
                "INSERT INTO posts (id, user_id, title, content, created_at, view_count, is_published) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .unwrap();
        for (i, id) in posts.iter().enumerate() {
            let uid = users[rng.random_range(0..n)];
            let ts: i64 = rng.random_range(1000000..2000000);
            stmt.execute(params![
                *id,
                uid,
                format!("Post title {i}"),
                format!("Post content for {i}"),
                ts,
                0i64,
                1i64
            ])
            .unwrap();
        }
    }

    // tags
    {
        let mut stmt = conn
            .prepare_cached("INSERT INTO tags (id, name) VALUES (?1, ?2)")
            .unwrap();
        for (i, id) in tags.iter().enumerate() {
            stmt.execute(params![*id, format!("tag_{i}")]).unwrap();
        }
    }

    // post_tags (one per post, random tag)
    {
        let mut stmt = conn
            .prepare_cached("INSERT INTO post_tags (post_id, tag_id) VALUES (?1, ?2)")
            .unwrap();
        for post in &posts {
            let tid = tags[rng.random_range(0..n)];
            stmt.execute(params![post, tid]).unwrap();
        }
    }

    // comments
    {
        let mut stmt = conn
            .prepare_cached(
                "INSERT INTO comments (id, post_id, user_id, content, created_at, is_deleted) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .unwrap();
        for (i, id) in comments.iter().enumerate() {
            let pid = posts[rng.random_range(0..n)];
            let uid = users[rng.random_range(0..n)];
            let ts: i64 = rng.random_range(1000000..2000000);
            stmt.execute(params![*id, pid, uid, format!("Comment {i}"), ts, 0i64])
                .unwrap();
        }
    }

    conn.execute_batch("COMMIT").unwrap();

    IdPools {
        users,
        posts,
        comments,
        tags,
    }
}

// ---------------------------------------------------------------------------
// Scenario generation
// ---------------------------------------------------------------------------

/// A prepared scenario: SQL statements and their binary changeset/patchset.
struct Scenario {
    /// Raw SQL statements (one per line, separated by `;\n`).
    sql: String,
    /// Same SQL wrapped in `BEGIN; ... COMMIT;`.
    sql_tx: String,
    changeset: Vec<u8>,
    patchset: Vec<u8>,
}

/// Pick a random element from a slice, or `None` if empty.
fn pick_random(ids: &[Id], rng: &mut StdRng) -> Option<Id> {
    if ids.is_empty() {
        None
    } else {
        Some(ids[rng.random_range(0..ids.len())])
    }
}

/// Build the SQL for a fresh `comments` row and record its id in `all_comments`.
fn new_comment_sql(
    rng: &mut StdRng,
    ctr_comment: &mut IdCounter,
    all_posts: &[Id],
    all_users: &[Id],
    all_comments: &mut Vec<Id>,
) -> String {
    let id = ctr_comment.next(rng);
    let label = id.short_label();
    let pid = pick_random(all_posts, rng).unwrap();
    let uid = pick_random(all_users, rng).unwrap();
    let ts: i64 = rng.random_range(1000000..2000000);
    all_comments.push(id);
    format!(
        "INSERT INTO comments (id, post_id, user_id, content, created_at, is_deleted) \
         VALUES ({id}, {pid}, {uid}, 'Comment {label}', {ts}, 0)"
    )
}

/// Generate a mixed batch of `op_count` operations (INSERT/UPDATE/DELETE)
/// spread across all five tables. Returns the same logical changes in three
/// representations: SQL text, changeset bytes, and patchset bytes.
///
/// The changeset/patchset bytes are produced by rusqlite's session extension,
/// guaranteeing a bit-accurate binary format.
#[allow(clippy::too_many_lines)]
fn generate_scenario(
    template_db: &Connection,
    config: &DbConfig,
    kind: IdKind,
    existing: &IdPools,
    op_count: usize,
    rng: &mut StdRng,
) -> Scenario {
    let conn = clone_db(template_db, config);

    let mut session = Session::new(&conn).unwrap();
    // Attach only the five data tables (not `changes_log`) so trigger
    // side-effects are not captured in the diff. During apply the target
    // DB has the same triggers, so the audit rows are produced naturally.
    for table in DATA_TABLES {
        session.attach(Some(*table)).unwrap();
    }

    let mut sql_parts: Vec<String> = Vec::with_capacity(op_count);

    // Mutable pools: existing rows + any rows we insert during this scenario.
    let mut all_users = existing.users.clone();
    let mut all_posts = existing.posts.clone();
    let mut all_comments = existing.comments.clone();
    let mut all_tags = existing.tags.clone();

    // Per-table counters for fresh IDs.
    let mut ctr_user = IdCounter::new(kind, existing.users.len());
    let mut ctr_post = IdCounter::new(kind, existing.posts.len());
    let mut ctr_comment = IdCounter::new(kind, existing.comments.len());
    let mut ctr_tag = IdCounter::new(kind, existing.tags.len());

    // ------------------------------------------------------------------
    // Seed: if the DB starts empty, ensure at least one user, post, and
    // tag exist so FK references always have valid targets.
    // ------------------------------------------------------------------
    if all_users.is_empty() {
        let id = ctr_user.next(rng);
        let ts: i64 = rng.random_range(1000000..2000000);
        let sql = format!(
            "INSERT INTO users (id, username, email, created_at, is_active) \
             VALUES ({id}, 'seed_u', 'seed@x.com', {ts}, 1)"
        );
        conn.execute(&sql, []).unwrap();
        sql_parts.push(sql);
        all_users.push(id);
    }
    if all_posts.is_empty() {
        let id = ctr_post.next(rng);
        let uid = all_users[0];
        let ts: i64 = rng.random_range(1000000..2000000);
        let sql = format!(
            "INSERT INTO posts (id, user_id, title, content, created_at, view_count, is_published) \
             VALUES ({id}, {uid}, 'seed_p', 'seed', {ts}, 0, 0)"
        );
        conn.execute(&sql, []).unwrap();
        sql_parts.push(sql);
        all_posts.push(id);
    }
    if all_tags.is_empty() {
        let id = ctr_tag.next(rng);
        let sql = format!("INSERT INTO tags (id, name) VALUES ({id}, 'seed_tag')");
        conn.execute(&sql, []).unwrap();
        sql_parts.push(sql);
        all_tags.push(id);
    }

    // ------------------------------------------------------------------
    // Main operation loop: 50% INSERT, 30% UPDATE, 20% DELETE.
    // ------------------------------------------------------------------
    for _ in 0..op_count {
        let roll: u32 = rng.random_range(0..100);

        if roll < 50 {
            // INSERT: distribute uniformly across the five tables.
            let table_roll: u32 = rng.random_range(0..5);
            let sql = match table_roll {
                0 => {
                    let id = ctr_user.next(rng);
                    let label = id.short_label();
                    let ts: i64 = rng.random_range(1000000..2000000);
                    all_users.push(id);
                    format!(
                        "INSERT INTO users (id, username, email, created_at, is_active) \
                         VALUES ({id}, 'new_user_{label}', 'new_{label}@example.com', {ts}, 1)"
                    )
                }
                1 => {
                    let id = ctr_post.next(rng);
                    let label = id.short_label();
                    let uid = pick_random(&all_users, rng).unwrap();
                    let ts: i64 = rng.random_range(1000000..2000000);
                    all_posts.push(id);
                    format!(
                        "INSERT INTO posts (id, user_id, title, content, created_at, view_count, is_published) \
                         VALUES ({id}, {uid}, 'New post {label}', 'Content {label}', {ts}, 0, 1)"
                    )
                }
                2 => new_comment_sql(
                    rng,
                    &mut ctr_comment,
                    &all_posts,
                    &all_users,
                    &mut all_comments,
                ),
                3 => {
                    let id = ctr_tag.next(rng);
                    let label = id.short_label();
                    all_tags.push(id);
                    format!("INSERT INTO tags (id, name) VALUES ({id}, 'new_tag_{label}')")
                }
                _ => {
                    let pid = pick_random(&all_posts, rng).unwrap();
                    let tid = pick_random(&all_tags, rng).unwrap();
                    format!(
                        "INSERT OR IGNORE INTO post_tags (post_id, tag_id) VALUES ({pid}, {tid})"
                    )
                }
            };
            conn.execute(&sql, []).unwrap();
            sql_parts.push(sql);
        } else if roll < 80 {
            // UPDATE: pick from all known rows (existing + newly inserted).
            let table_roll: u32 = rng.random_range(0..3);
            let sql = match table_roll {
                0 => {
                    let id = pick_random(&all_users, rng).unwrap();
                    let ts: i64 = rng.random_range(2000000..3000000);
                    format!("UPDATE users SET last_login = {ts} WHERE id = {id}")
                }
                1 => {
                    let id = pick_random(&all_posts, rng).unwrap();
                    let vc: i64 = rng.random_range(1..1000);
                    format!("UPDATE posts SET view_count = {vc} WHERE id = {id}")
                }
                _ => {
                    if let Some(id) = pick_random(&all_comments, rng) {
                        format!("UPDATE comments SET is_deleted = 1 WHERE id = {id}")
                    } else {
                        // No comments yet, fall back to an INSERT.
                        new_comment_sql(
                            rng,
                            &mut ctr_comment,
                            &all_posts,
                            &all_users,
                            &mut all_comments,
                        )
                    }
                }
            };
            let _ = conn.execute(&sql, []);
            sql_parts.push(sql);
        } else {
            // DELETE: only target pre-existing rows to avoid cancelling a
            // freshly inserted row (which would leave no trace in the diff).
            //
            // When foreign_keys is ON we restrict DELETEs to the `comments`
            // table (a leaf, nothing references it) to avoid FK violations.
            let deleted = if existing.users.is_empty() {
                false
            } else {
                let sql = if config.foreign_keys {
                    // FK-safe: only delete from the leaf table.
                    let id = pick_random(&existing.comments, rng).unwrap();
                    format!("DELETE FROM comments WHERE id = {id}")
                } else {
                    let table_roll: u32 = rng.random_range(0..3);
                    match table_roll {
                        0 => {
                            let id = pick_random(&existing.users, rng).unwrap();
                            format!("DELETE FROM users WHERE id = {id}")
                        }
                        1 => {
                            let id = pick_random(&existing.posts, rng).unwrap();
                            format!("DELETE FROM posts WHERE id = {id}")
                        }
                        _ => {
                            let id = pick_random(&existing.comments, rng).unwrap();
                            format!("DELETE FROM comments WHERE id = {id}")
                        }
                    }
                };
                let _ = conn.execute(&sql, []);
                sql_parts.push(sql);
                true
            };

            if !deleted {
                // Nothing to delete, fall back to an INSERT.
                let id = ctr_user.next(rng);
                let label = id.short_label();
                let ts: i64 = rng.random_range(1000000..2000000);
                let sql = format!(
                    "INSERT INTO users (id, username, email, created_at, is_active) \
                     VALUES ({id}, 'fb_user_{label}', 'fb_{label}@x.com', {ts}, 1)"
                );
                conn.execute(&sql, []).unwrap();
                sql_parts.push(sql);
                all_users.push(id);
            }
        }
    }

    // Extract changeset / patchset bytes from the session.
    let mut changeset = Vec::new();
    session.changeset_strm(&mut changeset).unwrap();
    let mut patchset = Vec::new();
    session.patchset_strm(&mut patchset).unwrap();

    let sql = sql_parts.join(";\n") + ";";
    let sql_tx = format!("BEGIN;\n{sql}\nCOMMIT;");

    Scenario {
        sql,
        sql_tx,
        changeset,
        patchset,
    }
}

// ---------------------------------------------------------------------------
// Apply helpers
// ---------------------------------------------------------------------------

/// Apply a changeset or patchset to a database connection.
fn apply_diff(conn: &Connection, diff: &[u8]) {
    use rusqlite::session::{ChangesetItem, ConflictAction, ConflictType};
    let mut cursor = Cursor::new(diff);
    conn.apply_strm(
        &mut cursor,
        None::<fn(&str) -> bool>,
        |_ct: ConflictType, _item: ChangesetItem| ConflictAction::SQLITE_CHANGESET_OMIT,
    )
    .unwrap();
}

/// Execute a batch of SQL statements.
fn apply_sql(conn: &Connection, sql: &str) {
    conn.execute_batch(sql).unwrap();
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Register the four bench functions (sql, `sql_tx`, patchset, changeset) for
/// a single `(template, config, scenario)` combination.
fn register_benches(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    template: &Connection,
    config: &DbConfig,
    scenario: &Scenario,
) {
    let sql_ref = &scenario.sql;
    let sql_tx_ref = &scenario.sql_tx;
    let patchset_ref = &scenario.patchset;
    let changeset_ref = &scenario.changeset;

    group.bench_function("sql", |b| {
        b.iter_batched(
            || clone_db(template, config),
            |conn| apply_sql(&conn, sql_ref),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("sql_tx", |b| {
        b.iter_batched(
            || clone_db(template, config),
            |conn| apply_sql(&conn, sql_tx_ref),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("patchset", |b| {
        b.iter_batched(
            || clone_db(template, config),
            |conn| apply_diff(&conn, patchset_ref),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("changeset", |b| {
        b.iter_batched(
            || clone_db(template, config),
            |conn| apply_diff(&conn, changeset_ref),
            BatchSize::SmallInput,
        );
    });
}

/// `base` configuration benchmarks: all `kind` x `state` x `op_count`
/// combinations with the vanilla schema (no indexes, no triggers, FK off).
fn bench_apply_base(c: &mut Criterion) {
    let config = &CONFIGS[0]; // "base"

    for kind in [IdKind::Integer, IdKind::Uuid] {
        let schema = make_schema(kind.sql_type());

        for (state_label, pop_size) in [("empty", 0usize), ("populated", 1000usize)] {
            let mut rng_template = StdRng::seed_from_u64(match kind {
                IdKind::Integer => 42,
                IdKind::Uuid => 77,
            });
            let template = create_db(&schema, kind.sql_type(), config);
            let existing = if pop_size > 0 {
                populate_db(&template, pop_size, kind, &mut rng_template)
            } else {
                IdPools::empty()
            };

            for op_count in [30usize, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000] {
                let group_name = format!("apply/{}/{state_label}/{op_count}", kind.label());
                let mut group = c.benchmark_group(&group_name);

                let mut rng_ops = StdRng::seed_from_u64(match kind {
                    IdKind::Integer => 123 + op_count as u64,
                    IdKind::Uuid => 456 + op_count as u64,
                });
                let scenario =
                    generate_scenario(&template, config, kind, &existing, op_count, &mut rng_ops);

                register_benches(&mut group, &template, config, &scenario);
                group.finish();
            }
        }
    }
}

/// Variant configuration benchmarks: `indexed`, `triggers`, `fk`. Run only
/// on the most representative scenario (`populated/1000`) for both PK kinds.
fn bench_apply_variants(c: &mut Criterion) {
    let pop_size: usize = 1000;
    let op_count: usize = 1000;

    for config in &CONFIGS[1..] {
        for kind in [IdKind::Integer, IdKind::Uuid] {
            let schema = make_schema(kind.sql_type());
            let mut rng_template = StdRng::seed_from_u64(match kind {
                IdKind::Integer => 42,
                IdKind::Uuid => 77,
            });
            let template = create_db(&schema, kind.sql_type(), config);
            let existing = populate_db(&template, pop_size, kind, &mut rng_template);

            let group_name = format!(
                "apply/{}/populated/{op_count}/{}",
                kind.label(),
                config.label,
            );
            let mut group = c.benchmark_group(&group_name);

            let mut rng_ops = StdRng::seed_from_u64(match kind {
                IdKind::Integer => 123 + op_count as u64,
                IdKind::Uuid => 456 + op_count as u64,
            });
            let scenario =
                generate_scenario(&template, config, kind, &existing, op_count, &mut rng_ops);

            register_benches(&mut group, &template, config, &scenario);
            group.finish();
        }
    }
}

criterion_group!(benches, bench_apply_base, bench_apply_variants);
criterion_main!(benches);
