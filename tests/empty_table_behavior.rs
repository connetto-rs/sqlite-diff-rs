//! Tests verifying `SQLite`'s session extension behavior with empty table entries.
//!
//! These tests answer two key questions for achieving bit-for-bit parity with rusqlite:
//!
//! 1. When all operations on a table cancel out (e.g., INSERT then DELETE on the same row),
//!    does `SQLite`'s session extension still include that table in the changeset/patchset?
//!
//! 2. If a table's operations cancel out and then new operations are added to it,
//!    does the table appear in its original position or at the end?
//!
//! The answers guide how `DiffSetBuilder` should handle empty tables and table ordering.
#![cfg(feature = "testing")]

use rusqlite::Connection;
use rusqlite::session::Session;
use sqlite_diff_rs::testing::session_changeset_and_patchset;

// =============================================================================
// Helper functions
// =============================================================================

/// Extract table names in order from a raw changeset or patchset binary.
///
/// The binary format has table headers starting with 'T' (0x54) for changesets
/// or 'P' (0x50) for patchsets, followed by column count, PK flags, and a
/// null-terminated table name.
fn extract_table_names(data: &[u8]) -> Vec<String> {
    if data.is_empty() {
        return Vec::new();
    }

    // Use manual parsing that follows the binary format exactly
    let mut names = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let marker = data[pos];
        if marker != 0x54 && marker != 0x50 {
            // This shouldn't happen at the start of a table section
            break;
        }

        pos += 1; // skip marker

        if pos >= data.len() {
            break;
        }
        let col_count = data[pos] as usize;
        pos += 1;

        // Skip PK flags
        if pos + col_count > data.len() {
            break;
        }
        pos += col_count;

        // Read null-terminated name
        let name_start = pos;
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[name_start..pos]).to_string();
        names.push(name);
        pos += 1; // skip null

        // Skip change records
        // Each starts with opcode (1 byte) + indirect flag (1 byte) + values
        while pos < data.len() {
            let byte = data[pos];
            // If it's a table marker, break to outer loop
            if byte == 0x54 || byte == 0x50 {
                break;
            }
            // Must be an opcode
            if byte != 0x12 && byte != 0x09 && byte != 0x17 {
                // Unknown byte, bail
                return names;
            }
            pos += 1; // opcode
            if pos >= data.len() {
                return names;
            }
            pos += 1; // indirect flag

            // Skip values based on operation type
            let num_value_sets = match byte {
                0x12 => 1, // INSERT: one set of col_count values
                0x09 => {
                    // DELETE: changeset = one set, patchset = one set (PK only but encoded per-col)
                    if marker == 0x50 {
                        // Patchset DELETE: values for PK columns only
                        1
                    } else {
                        1
                    }
                }
                0x17 => 2, // UPDATE: two sets (old + new)
                _ => return names,
            };

            for _ in 0..num_value_sets {
                for _ in 0..col_count {
                    if pos >= data.len() {
                        return names;
                    }
                    // Value encoding: type byte + optional payload
                    let vtype = data[pos];
                    pos += 1;
                    match vtype {
                        0x00 | 0x05 => {} // Undefined/NULL - no payload
                        0x01 => {
                            // Integer: 8-byte big-endian
                            pos += 8;
                        }
                        0x02 => {
                            // Float: 8 bytes
                            pos += 8;
                        }
                        0x03 | 0x04 => {
                            // Text/Blob: varint length + data
                            let (len, consumed) = read_varint(&data[pos..]);
                            pos += consumed;
                            #[allow(clippy::cast_possible_truncation)]
                            {
                                pos += len as usize;
                            }
                        }
                        _ => return names, // Unknown value type
                    }
                }
            }
        }
    }

    names
}

/// Read a varint (`SQLite` format) and return (value, `bytes_consumed`).
fn read_varint(data: &[u8]) -> (u64, usize) {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().enumerate().take(9) {
        if i == 8 {
            // 9th byte: use all 8 bits
            value = (value << 8) | u64::from(byte);
            return (value, 9);
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return (value, i + 1);
        }
    }
    (value, data.len().min(9))
}

// =============================================================================
// Question 1: Does SQLite preserve empty table entries in changesets/patchsets?
// =============================================================================

/// When a single table has INSERT + DELETE on the same row (cancelling out),
/// `SQLite` should produce an empty changeset (no table header at all).
#[test]
fn test_sqlite_empty_table_after_insert_delete_changeset() {
    let (changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "DELETE FROM users WHERE id = 1",
    ]);

    // Key question: is the changeset empty, or does it contain a table header
    // for "users" with zero operations?
    eprintln!("Changeset bytes after INSERT+DELETE cancel: {changeset:02x?}");
    eprintln!("Changeset length: {}", changeset.len());

    let table_names = extract_table_names(&changeset);
    eprintln!("Tables found in changeset: {table_names:?}");

    // SQLite's session extension should produce an empty changeset when all ops cancel
    assert!(
        changeset.is_empty(),
        "Expected empty changeset when INSERT+DELETE cancel out, but got {} bytes: {:02x?}\nTables: {:?}",
        changeset.len(),
        changeset,
        table_names,
    );
}

/// Same test for patchset format.
#[test]
fn test_sqlite_empty_table_after_insert_delete_patchset() {
    let (_, patchset) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "DELETE FROM users WHERE id = 1",
    ]);

    eprintln!("Patchset bytes after INSERT+DELETE cancel: {patchset:02x?}");
    eprintln!("Patchset length: {}", patchset.len());

    let table_names = extract_table_names(&patchset);
    eprintln!("Tables found in patchset: {table_names:?}");

    assert!(
        patchset.is_empty(),
        "Expected empty patchset when INSERT+DELETE cancel out, but got {} bytes: {:02x?}\nTables: {:?}",
        patchset.len(),
        patchset,
        table_names,
    );
}

/// When one table cancels out but another has real changes, the cancelled table
/// should not appear in the output.
#[test]
fn test_sqlite_cancelled_table_not_in_changeset_multi_table() {
    let (changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT)",
        // users: INSERT + DELETE = cancel
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "DELETE FROM users WHERE id = 1",
        // posts: INSERT stays
        "INSERT INTO posts (id, title) VALUES (1, 'Hello')",
    ]);

    let table_names = extract_table_names(&changeset);
    eprintln!("Tables in changeset (multi-table, one cancelled): {table_names:?}");
    eprintln!("Changeset bytes: {changeset:02x?}");

    assert!(
        !table_names.iter().any(|name| name == "users"),
        "Cancelled table 'users' should NOT appear in changeset. Tables found: {table_names:?}"
    );
    assert!(
        table_names.iter().any(|name| name == "posts"),
        "Active table 'posts' should appear in changeset. Tables found: {table_names:?}"
    );
}

/// Same for patchset.
#[test]
fn test_sqlite_cancelled_table_not_in_patchset_multi_table() {
    let (_, patchset) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT)",
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "DELETE FROM users WHERE id = 1",
        "INSERT INTO posts (id, title) VALUES (1, 'Hello')",
    ]);

    let table_names = extract_table_names(&patchset);
    eprintln!("Tables in patchset (multi-table, one cancelled): {table_names:?}");

    assert!(
        !table_names.iter().any(|name| name == "users"),
        "Cancelled table 'users' should NOT appear in patchset. Tables found: {table_names:?}"
    );
    assert!(
        table_names.iter().any(|name| name == "posts"),
        "Active table 'posts' should appear in patchset. Tables found: {table_names:?}"
    );
}

// =============================================================================
// Question 2: Table ordering after cancel + re-add
// =============================================================================

/// After a table's operations cancel out and then new operations are added,
/// does the table appear in its original position or at the end?
///
/// Scenario:
///   1. INSERT into `table_a` (first touched)
///   2. INSERT into `table_b` (second touched)
///   3. DELETE from `table_a` (cancels out `table_a`)
///   4. INSERT into `table_a` again (re-adds `table_a`)
///
/// Question: In the final changeset, is `table_a` before or after `table_b`?
#[test]
fn test_sqlite_table_order_after_cancel_and_readd_changeset() {
    let (changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE table_a (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE table_b (id INTEGER PRIMARY KEY, val TEXT)",
        // table_a is first touched
        "INSERT INTO table_a (id, val) VALUES (1, 'a1')",
        // table_b is second touched
        "INSERT INTO table_b (id, val) VALUES (1, 'b1')",
        // Cancel table_a's operation
        "DELETE FROM table_a WHERE id = 1",
        // Re-add to table_a
        "INSERT INTO table_a (id, val) VALUES (2, 'a2')",
    ]);

    let table_names = extract_table_names(&changeset);
    eprintln!("Table order in changeset after cancel+re-add: {table_names:?}");
    eprintln!("Changeset bytes: {changeset:02x?}");

    assert_eq!(
        table_names.len(),
        2,
        "Expected 2 tables, got: {table_names:?}"
    );

    // Document which order SQLite uses
    if table_names[0] == "table_a" {
        eprintln!("RESULT: SQLite preserves original table ordering (table_a first)");
    } else {
        eprintln!(
            "RESULT: SQLite reorders tables (table_b first after table_a cancelled and re-added)"
        );
    }

    // Verify both tables are present regardless of order
    assert!(table_names.iter().any(|name| name == "table_a"));
    assert!(table_names.iter().any(|name| name == "table_b"));
}

/// Same test for patchset format.
#[test]
fn test_sqlite_table_order_after_cancel_and_readd_patchset() {
    let (_, patchset) = session_changeset_and_patchset(&[
        "CREATE TABLE table_a (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE table_b (id INTEGER PRIMARY KEY, val TEXT)",
        "INSERT INTO table_a (id, val) VALUES (1, 'a1')",
        "INSERT INTO table_b (id, val) VALUES (1, 'b1')",
        "DELETE FROM table_a WHERE id = 1",
        "INSERT INTO table_a (id, val) VALUES (2, 'a2')",
    ]);

    let table_names = extract_table_names(&patchset);
    eprintln!("Table order in patchset after cancel+re-add: {table_names:?}");

    assert_eq!(
        table_names.len(),
        2,
        "Expected 2 tables, got: {table_names:?}"
    );
    assert!(table_names.iter().any(|name| name == "table_a"));
    assert!(table_names.iter().any(|name| name == "table_b"));
}

// =============================================================================
// Question 2b: Table ordering. Is original order preserved even when all ops
// cancel and a new one is added?
// =============================================================================

/// This test checks whether the session extension keeps the table in its
/// original "first touched" position even after all rows cancel out,
/// or whether it removes and re-appends it.
///
/// Three tables are used to make the ordering unambiguous:
///   Touch order: A, B, C
///   Cancel A, then re-add A.
///   Expected final order if preserved: A, B, C
///   Expected final order if re-appended: B, C, A
#[test]
fn test_sqlite_table_order_three_tables_cancel_first_changeset() {
    let (changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE alpha (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE beta  (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE gamma (id INTEGER PRIMARY KEY, val TEXT)",
        // Touch order: alpha, beta, gamma
        "INSERT INTO alpha (id, val) VALUES (1, 'a')",
        "INSERT INTO beta  (id, val) VALUES (1, 'b')",
        "INSERT INTO gamma (id, val) VALUES (1, 'c')",
        // Cancel alpha
        "DELETE FROM alpha WHERE id = 1",
        // Re-add to alpha
        "INSERT INTO alpha (id, val) VALUES (2, 'a2')",
    ]);

    let table_names = extract_table_names(&changeset);
    eprintln!("Three-table order (changeset): {table_names:?}");

    assert_eq!(table_names.len(), 3);

    // Record the actual behavior for documentation
    if table_names == vec!["alpha", "beta", "gamma"] {
        eprintln!("CONFIRMED: SQLite preserves original table order even after cancel+re-add");
    } else if table_names == vec!["beta", "gamma", "alpha"] {
        eprintln!("CONFIRMED: SQLite re-appends table after cancel+re-add (moved to end)");
    } else {
        eprintln!("UNEXPECTED: SQLite table order is {table_names:?}");
    }
}

/// Same three-table test for patchset.
#[test]
fn test_sqlite_table_order_three_tables_cancel_first_patchset() {
    let (_, patchset) = session_changeset_and_patchset(&[
        "CREATE TABLE alpha (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE beta  (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE gamma (id INTEGER PRIMARY KEY, val TEXT)",
        "INSERT INTO alpha (id, val) VALUES (1, 'a')",
        "INSERT INTO beta  (id, val) VALUES (1, 'b')",
        "INSERT INTO gamma (id, val) VALUES (1, 'c')",
        "DELETE FROM alpha WHERE id = 1",
        "INSERT INTO alpha (id, val) VALUES (2, 'a2')",
    ]);

    let table_names = extract_table_names(&patchset);
    eprintln!("Three-table order (patchset): {table_names:?}");

    assert_eq!(table_names.len(), 3);
}

// =============================================================================
// Parity tests: verify our DiffSetBuilder matches SQLite's behavior
// =============================================================================

/// Verify that our `DiffSetBuilder` produces the same result as `SQLite` when
/// operations cancel out on a single table.
#[test]
fn test_our_builder_empty_after_cancel() {
    use sqlite_diff_rs::SimpleTable;
    use sqlite_diff_rs::{ChangeDelete, ChangeSet, DiffOps, Insert};

    let schema = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);

    // Changeset: INSERT + DELETE should produce empty output
    let insert = Insert::<_, String, Vec<u8>>::from(schema.clone())
        .set(0, 1i64)
        .unwrap()
        .set(1, "Alice")
        .unwrap();
    let delete = ChangeDelete::<_, String, Vec<u8>>::from(schema.clone())
        .set(0, 1i64)
        .unwrap()
        .set(1, "Alice")
        .unwrap();

    let changeset_builder: ChangeSet<SimpleTable, String, Vec<u8>> = ChangeSet::new();
    let changeset_builder = changeset_builder.insert(insert);
    let changeset_builder = changeset_builder.delete(delete);
    let changeset_bytes: Vec<u8> = changeset_builder.build();

    // Should match SQLite: empty bytes
    let (sqlite_changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "DELETE FROM users WHERE id = 1",
    ]);

    assert_eq!(
        changeset_bytes, sqlite_changeset,
        "Our changeset after cancel should match SQLite's.\nOurs: {changeset_bytes:02x?}\nSQLite: {sqlite_changeset:02x?}",
    );
}

/// Verify table ordering parity with `SQLite` after cancel + re-add.
#[test]
fn test_our_builder_table_order_matches_sqlite_after_cancel_readd() {
    use sqlite_diff_rs::SimpleTable;
    use sqlite_diff_rs::{ChangeDelete, ChangeSet, DiffOps, Insert, ParsedDiffSet};

    let schema_a =
        SimpleTable::with_rowid_alias("table_a", &[("id", "INTEGER"), ("val", "TEXT")], 0);
    let schema_b =
        SimpleTable::with_rowid_alias("table_b", &[("id", "INTEGER"), ("val", "TEXT")], 0);

    // Build: insert A, insert B, delete A, insert A again
    let insert_a = Insert::<_, String, Vec<u8>>::from(schema_a.clone())
        .set(0, 1i64)
        .unwrap()
        .set(1, "a1")
        .unwrap();
    let insert_b = Insert::<_, String, Vec<u8>>::from(schema_b.clone())
        .set(0, 1i64)
        .unwrap()
        .set(1, "b1")
        .unwrap();
    let delete_a1 = ChangeDelete::<_, String, Vec<u8>>::from(schema_a.clone())
        .set(0, 1i64)
        .unwrap()
        .set(1, "a1")
        .unwrap();
    let insert_a2 = Insert::<_, String, Vec<u8>>::from(schema_a.clone())
        .set(0, 2i64)
        .unwrap()
        .set(1, "a2")
        .unwrap();

    let our_builder: ChangeSet<SimpleTable, String, Vec<u8>> = ChangeSet::new();
    let our_builder = our_builder.insert(insert_a);
    let our_builder = our_builder.insert(insert_b);
    let our_builder = our_builder.delete(delete_a1);
    let our_builder = our_builder.insert(insert_a2);
    let our_bytes: Vec<u8> = our_builder.build();

    // Get SQLite's version
    let (sqlite_bytes, _) = session_changeset_and_patchset(&[
        "CREATE TABLE table_a (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE table_b (id INTEGER PRIMARY KEY, val TEXT)",
        "INSERT INTO table_a (id, val) VALUES (1, 'a1')",
        "INSERT INTO table_b (id, val) VALUES (1, 'b1')",
        "DELETE FROM table_a WHERE id = 1",
        "INSERT INTO table_a (id, val) VALUES (2, 'a2')",
    ]);

    let our_table_order = extract_table_names(&our_bytes);
    let sqlite_table_order = extract_table_names(&sqlite_bytes);

    eprintln!("Our table order:    {our_table_order:?}");
    eprintln!("SQLite table order: {sqlite_table_order:?}");

    // Semantic comparison via parser (order-independent)
    if !our_bytes.is_empty() && !sqlite_bytes.is_empty() {
        let our_parsed =
            ParsedDiffSet::try_from(our_bytes.as_slice()).expect("Failed to parse our changeset");
        let sqlite_parsed = ParsedDiffSet::try_from(sqlite_bytes.as_slice())
            .expect("Failed to parse SQLite changeset");

        assert_eq!(
            our_parsed, sqlite_parsed,
            "Semantic mismatch between our builder and SQLite"
        );
    }

    // Table order comparison
    assert_eq!(
        our_table_order, sqlite_table_order,
        "Table ordering mismatch!\nOurs: {our_table_order:?}\nSQLite: {sqlite_table_order:?}\n\
         Our bytes: {our_bytes:02x?}\nSQLite bytes: {sqlite_bytes:02x?}",
    );
}

// =============================================================================
// Edge case: multiple cancellations
// =============================================================================

/// What happens when multiple inserts and deletes happen on the same table,
/// with only some rows surviving?
#[test]
fn test_sqlite_partial_cancel_preserves_table() {
    let (changeset, _) = session_changeset_and_patchset(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO users (id, name) VALUES (1, 'Alice')",
        "INSERT INTO users (id, name) VALUES (2, 'Bob')",
        "DELETE FROM users WHERE id = 1",
        // Row 2 survives => table should appear with INSERT for row 2
    ]);

    let table_names = extract_table_names(&changeset);
    eprintln!("Partial cancel changeset tables: {table_names:?}");
    eprintln!("Partial cancel changeset bytes: {changeset:02x?}");

    assert!(
        table_names.iter().any(|name| name == "users"),
        "Table with surviving operations should appear in changeset"
    );
    assert_ne!(changeset, [] as [u8; 0]);
}

/// UPDATE that reverts to original values: does `SQLite` keep it or discard it?
#[test]
fn test_sqlite_update_revert_to_original() {
    // We need the row to exist before the session starts,
    // then update and revert within the session.
    let conn = Connection::open_in_memory().expect("Failed to open DB");
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
        .unwrap();
    conn.execute("INSERT INTO users (id, name) VALUES (1, 'Alice')", [])
        .unwrap();

    let mut session = Session::new(&conn).unwrap();
    session.attach::<&str>(None).unwrap();

    // Update then revert
    conn.execute("UPDATE users SET name = 'Bob' WHERE id = 1", [])
        .unwrap();
    conn.execute("UPDATE users SET name = 'Alice' WHERE id = 1", [])
        .unwrap();

    let mut changeset_bytes = Vec::new();
    session.changeset_strm(&mut changeset_bytes).unwrap();

    let table_names = extract_table_names(&changeset_bytes);
    eprintln!("Update-revert changeset tables: {table_names:?}");
    eprintln!("Update-revert changeset bytes: {changeset_bytes:02x?}");

    // SQLite compares final vs original state, so a revert should produce empty changeset
    assert!(
        changeset_bytes.is_empty(),
        "Expected empty changeset when UPDATE reverts to original. Got {} bytes: {:02x?}\nTables: {:?}",
        changeset_bytes.len(),
        changeset_bytes,
        table_names,
    );
}

// =============================================================================
// Helper
// =============================================================================
