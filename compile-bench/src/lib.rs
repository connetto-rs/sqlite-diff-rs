//! Compile-time and artifact-size benchmark crate.
//!
//! This crate exists solely to measure compilation time and compiled binary size
//! for two different approaches to generating SQLite changesets and patchsets:
//!
//! - **`rusqlite`** feature: Uses rusqlite's native session extension API
//!   (requires compiling the bundled SQLite C library).
//! - **`builder`** feature: Uses sqlite-diff-rs's pure-Rust builder API.
//!
//! Each feature gate exposes a module with `changeset() -> Vec<u8>` and
//! `patchset() -> Vec<u8>` functions that produce identical output using the
//! same schema and operations.

// ---------------------------------------------------------------------------
// Feature: rusqlite
// ---------------------------------------------------------------------------
#[cfg(feature = "rusqlite")]
pub mod rusqlite_approach {
    use rusqlite::Connection;

    const SCHEMA: &str = "
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_login INTEGER,
    is_active INTEGER NOT NULL DEFAULT 1,
    profile_data BLOB
);
CREATE TABLE posts (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER,
    view_count INTEGER NOT NULL DEFAULT 0,
    is_published INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (user_id) REFERENCES users(id)
);
CREATE TABLE comments (
    id INTEGER PRIMARY KEY,
    post_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    parent_id INTEGER,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (post_id) REFERENCES posts(id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (parent_id) REFERENCES comments(id)
);
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE post_tags (
    post_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY (post_id, tag_id),
    FOREIGN KEY (post_id) REFERENCES posts(id),
    FOREIGN KEY (tag_id) REFERENCES tags(id)
);
";

    const OPERATIONS: &[&str] = &[
        "INSERT INTO users (id, username, email, created_at, is_active) VALUES (1, 'alice', 'alice@example.com', 1000000, 1)",
        "INSERT INTO users (id, username, email, created_at, is_active) VALUES (2, 'bob', 'bob@example.com', 1000100, 1)",
        "INSERT INTO users (id, username, email, created_at, is_active) VALUES (3, 'charlie', 'charlie@example.com', 1000200, 1)",
        "INSERT INTO users (id, username, email, created_at, is_active) VALUES (4, 'diana', 'diana@example.com', 1000300, 0)",
        "INSERT INTO users (id, username, email, created_at, is_active) VALUES (5, 'eve', 'eve@example.com', 1000400, 1)",
        "INSERT INTO posts (id, user_id, title, content, created_at, is_published) VALUES (1, 1, 'First Post', 'Hello World!', 1000500, 1)",
        "INSERT INTO posts (id, user_id, title, content, created_at, is_published) VALUES (2, 1, 'Second Post', 'More content', 1000600, 1)",
        "INSERT INTO posts (id, user_id, title, content, created_at, is_published) VALUES (3, 2, 'Bob''s Post', 'My thoughts', 1000700, 1)",
        "INSERT INTO posts (id, user_id, title, content, created_at, is_published) VALUES (4, 3, 'Draft', 'Work in progress', 1000800, 0)",
        "INSERT INTO posts (id, user_id, title, content, created_at, is_published) VALUES (5, 5, 'Eve''s Post', 'Latest news', 1000900, 1)",
        "INSERT INTO tags (id, name) VALUES (1, 'rust')",
        "INSERT INTO tags (id, name) VALUES (2, 'database')",
        "INSERT INTO tags (id, name) VALUES (3, 'tutorial')",
        "INSERT INTO tags (id, name) VALUES (4, 'news')",
        "INSERT INTO tags (id, name) VALUES (5, 'discussion')",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (1, 1)",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (1, 3)",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (2, 1)",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (3, 5)",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (5, 4)",
        "INSERT INTO comments (id, post_id, user_id, content, created_at) VALUES (1, 1, 2, 'Great post!', 1001000)",
        "INSERT INTO comments (id, post_id, user_id, content, created_at) VALUES (2, 1, 3, 'Thanks for sharing', 1001100)",
        "INSERT INTO comments (id, post_id, user_id, content, created_at) VALUES (3, 2, 2, 'Interesting', 1001200)",
        "INSERT INTO comments (id, post_id, user_id, content, created_at) VALUES (4, 3, 1, 'Nice work', 1001300)",
        "INSERT INTO comments (id, post_id, user_id, content, created_at) VALUES (5, 1, 5, 'Reply to comment 1', 1001400)",
        "UPDATE users SET last_login = 1002000 WHERE id = 1",
        "UPDATE users SET last_login = 1002100 WHERE id = 2",
        "UPDATE posts SET view_count = 10 WHERE id = 1",
        "UPDATE posts SET view_count = 5 WHERE id = 2",
        "UPDATE posts SET updated_at = 1002200, content = 'Updated content' WHERE id = 2",
        "DELETE FROM comments WHERE id = 5",
        "DELETE FROM post_tags WHERE post_id = 3 AND tag_id = 5",
        "DELETE FROM users WHERE id = 4",
    ];

    /// Generate a changeset using rusqlite's session extension.
    pub fn changeset() -> Vec<u8> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let mut session = rusqlite::session::Session::new(&conn).unwrap();
        session.attach::<&str>(None).unwrap();
        for op in OPERATIONS {
            conn.execute(op, []).unwrap();
        }
        let mut bytes = Vec::new();
        session.changeset_strm(&mut bytes).unwrap();
        bytes
    }

    /// Generate a patchset using rusqlite's session extension.
    pub fn patchset() -> Vec<u8> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let mut session = rusqlite::session::Session::new(&conn).unwrap();
        session.attach::<&str>(None).unwrap();
        for op in OPERATIONS {
            conn.execute(op, []).unwrap();
        }
        let mut bytes = Vec::new();
        session.patchset_strm(&mut bytes).unwrap();
        bytes
    }
}

// ---------------------------------------------------------------------------
// Feature: builder
// ---------------------------------------------------------------------------
#[cfg(feature = "builder")]
pub mod builder_approach {
    use sqlite_diff_rs::{
        ChangeDelete, ChangeSet, ChangeUpdate, DiffFormat, DiffOps, DiffSetBuilder, Insert,
        PatchDelete, PatchSet, PatchUpdate, TableSchema, Value,
    };

    // Type aliases for cleaner code
    type Schema = TableSchema<String>;
    type Val = Value<String, Vec<u8>>;

    fn create_schemas() -> (Schema, Schema, Schema, Schema, Schema) {
        let users = TableSchema::new("users".into(), 7, vec![1, 0, 0, 0, 0, 0, 0]);
        let posts = TableSchema::new("posts".into(), 8, vec![1, 0, 0, 0, 0, 0, 0, 0]);
        let comments = TableSchema::new("comments".into(), 7, vec![1, 0, 0, 0, 0, 0, 0]);
        let tags = TableSchema::new("tags".into(), 2, vec![1, 0]);
        let post_tags = TableSchema::new("post_tags".into(), 2, vec![1, 2]);
        (users, posts, comments, tags, post_tags)
    }

    fn get_user_rows() -> Vec<Vec<Val>> {
        vec![
            vec![
                1i64.into(),
                "alice".into(),
                "alice@example.com".into(),
                1000000i64.into(),
                Value::Null,
                1i64.into(),
                Value::Null,
            ],
            vec![
                2i64.into(),
                "bob".into(),
                "bob@example.com".into(),
                1000100i64.into(),
                Value::Null,
                1i64.into(),
                Value::Null,
            ],
            vec![
                3i64.into(),
                "charlie".into(),
                "charlie@example.com".into(),
                1000200i64.into(),
                Value::Null,
                1i64.into(),
                Value::Null,
            ],
            vec![
                4i64.into(),
                "diana".into(),
                "diana@example.com".into(),
                1000300i64.into(),
                Value::Null,
                0i64.into(),
                Value::Null,
            ],
            vec![
                5i64.into(),
                "eve".into(),
                "eve@example.com".into(),
                1000400i64.into(),
                Value::Null,
                1i64.into(),
                Value::Null,
            ],
        ]
    }

    fn get_post_rows() -> Vec<Vec<Val>> {
        vec![
            vec![
                1i64.into(),
                1i64.into(),
                "First Post".into(),
                "Hello World!".into(),
                1000500i64.into(),
                Value::Null,
                0i64.into(),
                1i64.into(),
            ],
            vec![
                2i64.into(),
                1i64.into(),
                "Second Post".into(),
                "More content".into(),
                1000600i64.into(),
                Value::Null,
                0i64.into(),
                1i64.into(),
            ],
            vec![
                3i64.into(),
                2i64.into(),
                "Bob's Post".into(),
                "My thoughts".into(),
                1000700i64.into(),
                Value::Null,
                0i64.into(),
                1i64.into(),
            ],
            vec![
                4i64.into(),
                3i64.into(),
                "Draft".into(),
                "Work in progress".into(),
                1000800i64.into(),
                Value::Null,
                0i64.into(),
                0i64.into(),
            ],
            vec![
                5i64.into(),
                5i64.into(),
                "Eve's Post".into(),
                "Latest news".into(),
                1000900i64.into(),
                Value::Null,
                0i64.into(),
                1i64.into(),
            ],
        ]
    }

    fn get_tag_rows() -> Vec<Vec<Val>> {
        vec![
            vec![1i64.into(), "rust".into()],
            vec![2i64.into(), "database".into()],
            vec![3i64.into(), "tutorial".into()],
            vec![4i64.into(), "news".into()],
            vec![5i64.into(), "discussion".into()],
        ]
    }

    fn get_post_tag_rows() -> Vec<Vec<Val>> {
        vec![
            vec![1i64.into(), 1i64.into()],
            vec![1i64.into(), 3i64.into()],
            vec![2i64.into(), 1i64.into()],
            vec![3i64.into(), 5i64.into()],
            vec![5i64.into(), 4i64.into()],
        ]
    }

    fn get_comment_rows() -> Vec<Vec<Val>> {
        vec![
            vec![
                1i64.into(),
                1i64.into(),
                2i64.into(),
                "Great post!".into(),
                1001000i64.into(),
                Value::Null,
                0i64.into(),
            ],
            vec![
                2i64.into(),
                1i64.into(),
                3i64.into(),
                "Thanks for sharing".into(),
                1001100i64.into(),
                Value::Null,
                0i64.into(),
            ],
            vec![
                3i64.into(),
                2i64.into(),
                2i64.into(),
                "Interesting".into(),
                1001200i64.into(),
                Value::Null,
                0i64.into(),
            ],
            vec![
                4i64.into(),
                3i64.into(),
                1i64.into(),
                "Nice work".into(),
                1001300i64.into(),
                Value::Null,
                0i64.into(),
            ],
            vec![
                5i64.into(),
                1i64.into(),
                5i64.into(),
                "Reply to comment 1".into(),
                1001400i64.into(),
                Value::Null,
                0i64.into(),
            ],
        ]
    }

    fn add_inserts<F>(
        mut builder: DiffSetBuilder<F, Schema, String, Vec<u8>>,
        users: &Schema,
        posts: &Schema,
        comments: &Schema,
        tags: &Schema,
        post_tags: &Schema,
    ) -> DiffSetBuilder<F, Schema, String, Vec<u8>>
    where
        F: DiffFormat<String, Vec<u8>>,
        DiffSetBuilder<F, Schema, String, Vec<u8>>: DiffOps<Schema, String, Vec<u8>, Format = F>,
    {
        for (schema, rows) in [
            (users, get_user_rows()),
            (posts, get_post_rows()),
            (tags, get_tag_rows()),
            (post_tags, get_post_tag_rows()),
            (comments, get_comment_rows()),
        ] {
            for row in rows {
                let mut insert = Insert::from(schema.clone());
                for (i, val) in row.iter().enumerate() {
                    insert = insert.set(i, val.clone()).unwrap();
                }
                builder = builder.insert(insert);
            }
        }
        builder
    }

    /// Apply a batch of `(old_row, new_row)` updates against one table.
    fn apply_changeset_updates(
        mut builder: ChangeSet<Schema, String, Vec<u8>>,
        table: &Schema,
        updates: &[(&[Val], &[Val])],
    ) -> ChangeSet<Schema, String, Vec<u8>> {
        for (old, new) in updates {
            let mut update = ChangeUpdate::from(table.clone());
            for (i, (o, n)) in old.iter().zip(new.iter()).enumerate() {
                update = update.set(i, o.clone(), n.clone()).unwrap();
            }
            builder = builder.update(update);
        }
        builder
    }

    /// Apply a batch of `(column, new_value)` updates against one table.
    fn apply_patchset_updates(
        mut builder: PatchSet<Schema, String, Vec<u8>>,
        table: &Schema,
        updates: &[&[(usize, Val)]],
    ) -> PatchSet<Schema, String, Vec<u8>> {
        for cols in updates {
            let mut update = PatchUpdate::from(table.clone());
            for (i, val) in *cols {
                update = update.set(*i, val.clone()).unwrap();
            }
            builder = builder.update(update);
        }
        builder
    }

    fn add_changeset_updates(
        mut builder: ChangeSet<Schema, String, Vec<u8>>,
        users: &Schema,
        posts: &Schema,
    ) -> ChangeSet<Schema, String, Vec<u8>> {
        let user_updates: &[(&[Val], &[Val])] = &[
            (
                &[
                    1i64.into(),
                    "alice".into(),
                    "alice@example.com".into(),
                    1000000i64.into(),
                    Value::Null,
                    1i64.into(),
                    Value::Null,
                ],
                &[
                    1i64.into(),
                    "alice".into(),
                    "alice@example.com".into(),
                    1000000i64.into(),
                    1002000i64.into(),
                    1i64.into(),
                    Value::Null,
                ],
            ),
            (
                &[
                    2i64.into(),
                    "bob".into(),
                    "bob@example.com".into(),
                    1000100i64.into(),
                    Value::Null,
                    1i64.into(),
                    Value::Null,
                ],
                &[
                    2i64.into(),
                    "bob".into(),
                    "bob@example.com".into(),
                    1000100i64.into(),
                    1002100i64.into(),
                    1i64.into(),
                    Value::Null,
                ],
            ),
        ];
        builder = apply_changeset_updates(builder, users, user_updates);

        let post_updates: &[(&[Val], &[Val])] = &[
            (
                &[
                    1i64.into(),
                    1i64.into(),
                    "First Post".into(),
                    "Hello World!".into(),
                    1000500i64.into(),
                    Value::Null,
                    0i64.into(),
                    1i64.into(),
                ],
                &[
                    1i64.into(),
                    1i64.into(),
                    "First Post".into(),
                    "Hello World!".into(),
                    1000500i64.into(),
                    Value::Null,
                    10i64.into(),
                    1i64.into(),
                ],
            ),
            (
                &[
                    2i64.into(),
                    1i64.into(),
                    "Second Post".into(),
                    "More content".into(),
                    1000600i64.into(),
                    Value::Null,
                    0i64.into(),
                    1i64.into(),
                ],
                &[
                    2i64.into(),
                    1i64.into(),
                    "Second Post".into(),
                    "More content".into(),
                    1000600i64.into(),
                    Value::Null,
                    5i64.into(),
                    1i64.into(),
                ],
            ),
            (
                &[
                    2i64.into(),
                    1i64.into(),
                    "Second Post".into(),
                    "More content".into(),
                    1000600i64.into(),
                    Value::Null,
                    5i64.into(),
                    1i64.into(),
                ],
                &[
                    2i64.into(),
                    1i64.into(),
                    "Second Post".into(),
                    "Updated content".into(),
                    1000600i64.into(),
                    1002200i64.into(),
                    5i64.into(),
                    1i64.into(),
                ],
            ),
        ];
        builder = apply_changeset_updates(builder, posts, post_updates);
        builder
    }

    fn add_changeset_deletes(
        mut builder: ChangeSet<Schema, String, Vec<u8>>,
        users: &Schema,
        comments: &Schema,
        post_tags: &Schema,
    ) -> ChangeSet<Schema, String, Vec<u8>> {
        // Delete comment 5
        let mut delete = ChangeDelete::from(comments.clone());
        for (i, val) in [
            5i64.into(),
            1i64.into(),
            5i64.into(),
            Value::from("Reply to comment 1"),
            1001400i64.into(),
            Value::Null,
            0i64.into(),
        ]
        .iter()
        .enumerate()
        {
            delete = delete.set(i, val.clone()).unwrap();
        }
        builder = builder.delete(delete);

        // Delete post_tag (3, 5)
        let mut delete = ChangeDelete::from(post_tags.clone());
        for (i, val) in [Value::from(3i64), Value::from(5i64)].iter().enumerate() {
            delete = delete.set(i, val.clone()).unwrap();
        }
        builder = builder.delete(delete);

        // Delete user 4
        let mut delete = ChangeDelete::from(users.clone());
        for (i, val) in [
            4i64.into(),
            "diana".into(),
            "diana@example.com".into(),
            1000300i64.into(),
            Value::Null,
            0i64.into(),
            Value::Null,
        ]
        .iter()
        .enumerate()
        {
            delete = delete.set(i, val.clone()).unwrap();
        }
        builder = builder.delete(delete);
        builder
    }

    /// Generate a changeset using the builder API.
    pub fn changeset() -> Vec<u8> {
        let (users, posts, comments, tags, post_tags) = create_schemas();
        let builder = ChangeSet::new();
        let builder = add_inserts(builder, &users, &posts, &comments, &tags, &post_tags);
        let builder = add_changeset_updates(builder, &users, &posts);
        let builder = add_changeset_deletes(builder, &users, &comments, &post_tags);
        builder.build()
    }

    fn add_patchset_updates(
        mut builder: PatchSet<Schema, String, Vec<u8>>,
        users: &Schema,
        posts: &Schema,
    ) -> PatchSet<Schema, String, Vec<u8>> {
        let user_updates: &[&[(usize, Val)]] = &[
            &[(0, 1i64.into()), (4, 1002000i64.into())],
            &[(0, 2i64.into()), (4, 1002100i64.into())],
        ];
        builder = apply_patchset_updates(builder, users, user_updates);

        let post_updates: &[&[(usize, Val)]] = &[
            &[(0, 1i64.into()), (6, 10i64.into())],
            &[(0, 2i64.into()), (6, 5i64.into())],
            &[
                (0, 2i64.into()),
                (3, "Updated content".into()),
                (5, 1002200i64.into()),
            ],
        ];
        builder = apply_patchset_updates(builder, posts, post_updates);
        builder
    }

    fn add_patchset_deletes(
        builder: PatchSet<Schema, String, Vec<u8>>,
        users: &Schema,
        comments: &Schema,
        post_tags: &Schema,
    ) -> PatchSet<Schema, String, Vec<u8>> {
        builder
            .delete(PatchDelete::new(comments.clone(), vec![5i64.into()]))
            .delete(PatchDelete::new(
                post_tags.clone(),
                vec![3i64.into(), 5i64.into()],
            ))
            .delete(PatchDelete::new(users.clone(), vec![4i64.into()]))
    }

    /// Generate a patchset using the builder API.
    pub fn patchset() -> Vec<u8> {
        let (users, posts, comments, tags, post_tags) = create_schemas();
        let builder = PatchSet::new();
        let builder = add_inserts(builder, &users, &posts, &comments, &tags, &post_tags);
        let builder = add_patchset_updates(builder, &users, &posts);
        let builder = add_patchset_deletes(builder, &users, &comments, &post_tags);
        builder.build()
    }
}

// ---------------------------------------------------------------------------
// Exported C symbols so the linker retains the actual code in the cdylib.
// Without these, the linker strips all code and every feature produces an
// identically-sized (empty) shared library.
// ---------------------------------------------------------------------------

#[cfg(feature = "rusqlite")]
mod rusqlite_exports {
    use super::rusqlite_approach;

    #[unsafe(no_mangle)]
    pub extern "C" fn rusqlite_changeset_len() -> usize {
        rusqlite_approach::changeset().len()
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn rusqlite_patchset_len() -> usize {
        rusqlite_approach::patchset().len()
    }
}

#[cfg(feature = "builder")]
mod builder_exports {
    use super::builder_approach;

    #[unsafe(no_mangle)]
    pub extern "C" fn builder_changeset_len() -> usize {
        builder_approach::changeset().len()
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn builder_patchset_len() -> usize {
        builder_approach::patchset().len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(feature = "rusqlite")]
    mod rusqlite_tests {
        use crate::rusqlite_approach;

        #[test]
        fn test_rusqlite_changeset() {
            let changeset = rusqlite_approach::changeset();
            assert!(!changeset.is_empty(), "changeset should not be empty");
        }

        #[test]
        fn test_rusqlite_patchset() {
            let patchset = rusqlite_approach::patchset();
            assert!(!patchset.is_empty(), "patchset should not be empty");
        }
    }

    #[cfg(feature = "builder")]
    mod builder_tests {
        use crate::builder_approach;

        #[test]
        fn test_builder_changeset() {
            let changeset = builder_approach::changeset();
            assert!(!changeset.is_empty(), "changeset should not be empty");
        }

        #[test]
        fn test_builder_patchset() {
            let patchset = builder_approach::patchset();
            assert!(!patchset.is_empty(), "patchset should not be empty");
        }
    }

    #[cfg(all(feature = "rusqlite", feature = "builder"))]
    mod comparison_tests {
        use crate::{builder_approach, rusqlite_approach};

        #[test]
        fn test_changeset_output_matches() {
            let rusqlite = rusqlite_approach::changeset();
            let builder = builder_approach::changeset();
            assert_eq!(
                rusqlite, builder,
                "rusqlite and builder changesets should match"
            );
        }

        #[test]
        fn test_patchset_output_matches() {
            let rusqlite = rusqlite_approach::patchset();
            let builder = builder_approach::patchset();
            assert_eq!(
                rusqlite, builder,
                "rusqlite and builder patchsets should match"
            );
        }
    }
}
