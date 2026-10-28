//! Testing utilities for bit-parity verification against rusqlite's session extension.
//!
//! Gated behind the `testing` feature.
//!
//! The module groups three kinds of helpers. [`session_changeset_and_patchset`],
//! [`byte_diff_report`], and [`assert_bit_parity`] handle byte-level comparison
//! against rusqlite. [`TypedSimpleTable`] and [`SqlType`] describe schemas with
//! enough type information to emit `CREATE TABLE` DDL. [`test_roundtrip`],
//! [`test_apply_roundtrip`], [`test_reverse_idempotent`], [`test_sql_roundtrip`],
//! and [`test_differential`] drive parse, serialize, apply, and reverse paths
//! from a single fuzz or regression input.

use core::fmt::{self, Write};
use core::ops::Deref;

extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use rusqlite::Connection;
use rusqlite::session::Session;
use std::io::Cursor;

use crate::DynTable;
use crate::PatchSet;
use crate::Reverse;
use crate::SchemaWithPK;
use crate::differential_testing::run_differential_test;
use crate::parser::ParsedDiffSet;
use crate::schema::SimpleTable;

// ---------------------------------------------------------------------------
// SqlType: SQLite column type affinities
// ---------------------------------------------------------------------------

/// `SQLite` column type affinities for use in `CREATE TABLE` DDL generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlType {
    /// `INTEGER` affinity.
    Integer,
    /// `TEXT` affinity.
    Text,
    /// `REAL` affinity.
    Real,
    /// `BLOB` affinity (accepts any value).
    Blob,
    /// `NUMERIC` affinity.
    Numeric,
    /// No declared type, which also has `BLOB` affinity.
    Untyped,
}

impl SqlType {
    /// The declared type as written in `CREATE TABLE`.
    #[must_use]
    pub const fn declared(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Text => "TEXT",
            Self::Real => "REAL",
            Self::Blob => "BLOB",
            Self::Numeric => "NUMERIC",
            Self::Untyped => "",
        }
    }
}

impl fmt::Display for SqlType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.declared())
    }
}

impl<'a> arbitrary::Arbitrary<'a> for SqlType {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(*u.choose(&[
            Self::Integer,
            Self::Text,
            Self::Real,
            Self::Blob,
            Self::Numeric,
            Self::Untyped,
        ])?)
    }
}

// ---------------------------------------------------------------------------
// TypedSimpleTable: SimpleTable + column types with Display for DDL
// ---------------------------------------------------------------------------

/// A [`SimpleTable`] augmented with column type information.
///
/// Implements [`Display`](fmt::Display) to emit a `CREATE TABLE` SQL statement,
/// enabling database setup from schema metadata alone (e.g. in fuzz harnesses).
///
/// Dereferences to [`SimpleTable`], so it can be used anywhere a `SimpleTable`
/// is expected.
///
/// # Example
///
/// ```rust
/// use sqlite_diff_rs::testing::{TypedSimpleTable, SqlType};
///
/// let table = TypedSimpleTable::new(
///     "users",
///     &[("id", SqlType::Integer), ("name", SqlType::Text)],
///     &[0],
/// );
/// assert_eq!(
///     table.to_string(),
///     "CREATE TABLE \"users\" (\"id\" INTEGER PRIMARY KEY, \"name\" TEXT)"
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedSimpleTable {
    table: SimpleTable,
    column_types: Vec<SqlType>,
}

impl TypedSimpleTable {
    /// Create a new typed table schema.
    ///
    /// # Arguments
    ///
    /// * `name` - the table name.
    /// * `columns` - pairs of `(column_name, column_type)` in order.
    /// * `pk_indices` - indices of primary key columns (in PK order).
    ///
    /// # Panics
    ///
    /// Panics if any `pk_indices` value is out of bounds.
    #[must_use]
    pub fn new(name: &str, columns: &[(&str, SqlType)], pk_indices: &[usize]) -> Self {
        let declared: Vec<(&str, &str)> = columns.iter().map(|&(n, t)| (n, t.declared())).collect();
        // The DDL inlines a lone key as `INTEGER PRIMARY KEY`, which aliases the rowid.
        let table = match pk_indices {
            &[key]
                if columns
                    .get(key)
                    .is_some_and(|&(_, t)| t == SqlType::Integer) =>
            {
                SimpleTable::with_rowid_alias(name, &declared, key)
            }
            _ => SimpleTable::new(name, &declared, pk_indices),
        };
        Self {
            table,
            column_types: columns.iter().map(|&(_, t)| t).collect(),
        }
    }

    /// Create a `TypedSimpleTable` from a [`crate::parser::TableSchema`].
    ///
    /// Synthesizes generic column names (`c0`, `c1`, ...) and uses
    /// [`SqlType::Blob`] for every column (the most permissive `SQLite` type).
    /// This is primarily useful in fuzz harnesses that parse arbitrary binary
    /// changesets and need to create matching database tables.
    #[must_use]
    pub fn from_table_schema(schema: &crate::parser::TableSchema<String>) -> Self {
        let ncols = schema.number_of_columns();
        let mut pk_flags_buf = vec![0u8; ncols];
        schema.write_pk_flags(&mut pk_flags_buf);

        // Derive pk_indices from pk_flags (sorted by ordinal)
        let mut pk_cols: Vec<(usize, u8)> = pk_flags_buf
            .iter()
            .enumerate()
            .filter_map(|(i, &ord)| if ord > 0 { Some((i, ord)) } else { None })
            .collect();
        pk_cols.sort_by_key(|&(_, ord)| ord);
        let pk_indices: Vec<usize> = pk_cols.into_iter().map(|(i, _)| i).collect();

        let columns: Vec<(String, SqlType)> = (0..ncols)
            .map(|i| (format!("c{i}"), SqlType::Blob))
            .collect();
        let col_refs: Vec<(&str, SqlType)> =
            columns.iter().map(|(n, t)| (n.as_str(), *t)).collect();

        Self::new(schema.name(), &col_refs, &pk_indices)
    }

    /// The column types in order.
    #[must_use]
    pub fn column_types(&self) -> &[SqlType] {
        &self.column_types
    }
}

impl Deref for TypedSimpleTable {
    type Target = SimpleTable;

    fn deref(&self) -> &Self::Target {
        &self.table
    }
}

impl fmt::Display for TypedSimpleTable {
    /// Emit a `CREATE TABLE` DDL statement.
    ///
    /// For a single-column PK the `PRIMARY KEY` clause is inlined on the column.
    /// For composite PKs a trailing `PRIMARY KEY(...)` constraint is appended.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pk_indices: Vec<usize> = self.table.primary_key_columns().collect();
        let columns = self.table.column_names();
        let single_pk = pk_indices.len() == 1;

        write!(f, "CREATE TABLE \"{}\" (", self.table.name())?;

        for (i, (col_name, col_type)) in columns.iter().zip(&self.column_types).enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{col_name}\"")?;
            if *col_type != SqlType::Untyped {
                write!(f, " {col_type}")?;
            }
            if single_pk && pk_indices[0] == i {
                f.write_str(" PRIMARY KEY")?;
            }
        }

        if !single_pk && !pk_indices.is_empty() {
            f.write_str(", PRIMARY KEY(")?;
            for (j, &pk_idx) in pk_indices.iter().enumerate() {
                if j > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "\"{}\"", columns[pk_idx])?;
            }
            f.write_char(')')?;
        }

        f.write_char(')')
    }
}

impl<'a> arbitrary::Arbitrary<'a> for TypedSimpleTable {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // Table name: 1 to 8 lowercase alpha chars
        let name_len = u.int_in_range(1..=8)?;
        let name: String = (0..name_len)
            .map(|_| u.int_in_range(b'a'..=b'z').map(char::from))
            .collect::<arbitrary::Result<_>>()?;

        Self::arbitrary_with_name(u, &name)
    }
}

impl TypedSimpleTable {
    /// Generate an arbitrary table with a given name.
    ///
    /// This is shared between the single-table and multi-table `Arbitrary`
    /// implementations so that column count, types, and PK layout are still
    /// fuzz-driven while the caller controls naming.
    fn arbitrary_with_name(
        u: &mut arbitrary::Unstructured<'_>,
        name: &str,
    ) -> arbitrary::Result<Self> {
        use arbitrary::Arbitrary;

        // Column count: 1 to 8
        let ncols: usize = u.int_in_range(1..=8)?;
        let columns: Vec<(&str, SqlType)> = Vec::new(); // placeholder
        let mut col_data: Vec<(String, SqlType)> = Vec::with_capacity(ncols);
        for i in 0..ncols {
            let ty = SqlType::arbitrary(u)?;
            col_data.push((format!("c{i}"), ty));
        }
        let col_refs: Vec<(&str, SqlType)> =
            col_data.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        drop(columns);

        // PK: at least 1 column, up to ncols
        let npk: usize = u.int_in_range(1..=ncols)?;
        // Choose npk distinct indices from 0..ncols
        let mut available: Vec<usize> = (0..ncols).collect();
        let mut pk_indices = Vec::with_capacity(npk);
        for _ in 0..npk {
            // available.len() is guaranteed > 0 here because npk <= ncols and we remove one per iteration
            #[allow(clippy::range_minus_one)]
            let idx = u.int_in_range(0..=available.len() - 1)?;
            pk_indices.push(available.remove(idx));
        }

        Ok(Self::new(name, &col_refs, &pk_indices))
    }
}

// ---------------------------------------------------------------------------
// FuzzSchemas: Vec<TypedSimpleTable> with guaranteed unique table names
// ---------------------------------------------------------------------------

/// A collection of 1 to 5 [`TypedSimpleTable`] schemas with unique table names.
///
/// Used as fuzz input for multi-table harnesses. Table names are deterministic
/// (`t0`, `t1`, ...) to avoid collisions. Column count, types, and PK layout
/// remain fuzz-driven.
///
/// Dereferences to `[TypedSimpleTable]` for ergonomic slice access.
#[derive(Debug, Clone)]
pub struct FuzzSchemas(pub Vec<TypedSimpleTable>);

impl Deref for FuzzSchemas {
    type Target = [TypedSimpleTable];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> arbitrary::Arbitrary<'a> for FuzzSchemas {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let ntables: usize = u.int_in_range(1..=5)?;
        let mut tables = Vec::with_capacity(ntables);
        for i in 0..ntables {
            let name = format!("t{i}");
            tables.push(TypedSimpleTable::arbitrary_with_name(u, &name)?);
        }
        Ok(Self(tables))
    }
}

// ---------------------------------------------------------------------------
// Shared fuzzer / regression-test helpers
// ---------------------------------------------------------------------------

/// Test binary roundtrip: parse, serialize, reparse, assert equality.
///
/// Returns early (no panic) if the input cannot be parsed.
///
/// # Panics
///
/// Panics if re-parsing our own output fails or if the reparsed data doesn't
/// match the original.
pub fn test_roundtrip(input: &[u8]) {
    let Ok(parsed) = ParsedDiffSet::try_from(input) else {
        return; // Invalid input is fine, we just shouldn't crash
    };

    let serialized: Vec<u8> = parsed.clone().into();
    let reparsed = ParsedDiffSet::try_from(serialized.as_slice())
        .expect("Re-parsing our own output should never fail");
    assert_eq!(parsed, reparsed, "Roundtrip mismatch");
}

/// Test that a changeset can be applied to an in-memory database without
/// crashing, in addition to binary roundtrip verification.
///
/// Each schema in `schemas` provides a `CREATE TABLE` DDL (via its
/// [`Display`](fmt::Display) impl) so the changeset has matching tables to
/// apply against.
///
/// Returns early if the input does not parse as a valid changeset/patchset,
/// skipping the (expensive) `SQLite` setup. When parsing succeeds the
/// **re-serialized** bytes (not the original fuzz input) are applied,
/// so we test that *our* output is accepted by `SQLite`.
///
/// Application errors are **not** treated as failures, since the changeset
/// may be semantically invalid for the given schemas. Panics or crashes are
/// bugs.
///
/// # Panics
///
/// Panics if re-parsing our serialized output fails or if the reparsed data
/// doesn't match the original.
pub fn test_apply_roundtrip(schemas: &[TypedSimpleTable], changeset_bytes: &[u8]) {
    // Parse the input. Bail early if it is not a valid changeset/patchset.
    // This avoids paying the SQLite-setup cost for the vast majority of
    // fuzzed inputs (random bytes almost never form valid changesets).
    let Ok(parsed) = ParsedDiffSet::try_from(changeset_bytes) else {
        return;
    };

    // Binary roundtrip check: serialize, reparse, assert equality.
    let serialized: Vec<u8> = parsed.clone().into();
    let reparsed = ParsedDiffSet::try_from(serialized.as_slice())
        .expect("Re-parsing our own output should never fail");
    assert_eq!(parsed, reparsed, "Roundtrip mismatch");

    // Create an in-memory database with all tables
    let Ok(conn) = Connection::open_in_memory() else {
        return;
    };
    for schema in schemas {
        let ddl = schema.to_string();
        if conn.execute(&ddl, []).is_err() {
            return; // Schema might be invalid (e.g. no PK)
        }
    }

    // Apply the *re-serialized* bytes. Errors are acceptable, panics are not.
    let _ = apply_changeset(&conn, &serialized);
}

/// Test reverse idempotency: `reverse(reverse(x)) == x`.
///
/// Parses the input as a binary changeset (patchsets are skipped because they
/// do not support [`Reverse`]) and verifies that double-reversing yields a
/// structurally equal changeset, that the binary representations match after
/// double reverse, that the operation count is preserved by reversal, and
/// that empty changesets reverse to empty. Returns early (no panic) if the
/// input cannot be parsed or is a patchset.
///
/// # Panics
///
/// Panics if any of the reversal invariants are violated (double-reverse
/// not equal to original, operation count changes, etc.).
pub fn test_reverse_idempotent(input: &[u8]) {
    let Ok(parsed) = ParsedDiffSet::try_from(input) else {
        return;
    };

    // Only changesets support Reverse
    let ParsedDiffSet::Changeset(changeset) = parsed else {
        return;
    };

    let reversed = changeset.clone().reverse();
    let double_reversed = reversed.clone().reverse();

    assert_eq!(
        changeset, double_reversed,
        "Double reverse should equal original"
    );

    let original_bytes = changeset.build();
    let double_reversed_bytes = double_reversed.build();
    assert_eq!(
        original_bytes, double_reversed_bytes,
        "Binary representation should be identical after double reverse"
    );

    assert_eq!(
        changeset.len(),
        reversed.len(),
        "Reversed changeset should have same number of operations"
    );

    if changeset.is_empty() {
        assert!(
            reversed.is_empty(),
            "Empty changeset should reverse to empty"
        );
    }
}

/// Test SQL-digest roundtrip: digest SQL into a patchset, serialize, reparse.
///
/// Builds a [`PatchSet`] with the given schemas, digests the SQL, returns
/// early if digestion fails or the result is empty, then serializes to
/// binary and reparses, asserting byte equality.
///
/// # Panics
///
/// Panics if the serialized patchset cannot be re-parsed or if the binary
/// representation changes after roundtrip.
pub fn test_sql_roundtrip(schemas: &[TypedSimpleTable], sql: &str) {
    let mut builder: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
    for schema in schemas {
        builder.add_table(&**schema);
    }

    if builder.digest_sql(sql).is_err() {
        return;
    }
    if builder.is_empty() {
        return;
    }

    let bytes = builder.build();
    let reparsed = ParsedDiffSet::try_from(bytes.as_slice())
        .expect("Serialized patchset should be re-parseable");
    let reparsed_bytes: Vec<u8> = reparsed.into();
    assert_eq!(
        bytes, reparsed_bytes,
        "Binary round-trip mismatch after SQL digest"
    );
}

/// Test differential (bit-parity) between our patchset output and rusqlite's.
///
/// Builds a [`PatchSet`] with the given schemas, digests the SQL, returns
/// early if digestion fails or the result is empty, then delegates to
/// `run_differential_test` to compare our bytes against rusqlite's session
/// extension output, which it skips for input without a counterpart session.
pub fn test_differential(schemas: &[TypedSimpleTable], sql: &str) {
    let mut builder: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
    for schema in schemas {
        builder.add_table(&**schema);
    }

    if builder.digest_sql(sql).is_err() || builder.is_empty() {
        return;
    }

    let create_sqls: Vec<String> = schemas.iter().map(ToString::to_string).collect();
    let create_sql_refs: Vec<&str> = create_sqls.iter().map(String::as_str).collect();
    let simples: Vec<SimpleTable> = schemas.iter().map(|s| (**s).clone()).collect();
    let _compared = run_differential_test(&simples, &create_sql_refs, &[sql]);
}

/// Create an in-memory `SQLite` database, execute statements with a session,
/// and return the raw changeset and patchset bytes.
///
/// DDL (`CREATE TABLE`) is executed before the session starts.
/// DML (`INSERT`/`UPDATE`/`DELETE`) is executed inside the session, and a
/// string may hold several statements.
///
/// # Panics
///
/// Panics if database creation, statement execution, or session operations fail.
#[must_use]
pub fn session_changeset_and_patchset(statements: &[&str]) -> (Vec<u8>, Vec<u8>) {
    fn run_session(statements: &[&str], extract: impl Fn(&mut Session<'_>) -> Vec<u8>) -> Vec<u8> {
        let conn = Connection::open_in_memory().unwrap();
        for &sql in statements {
            if sql.trim().to_uppercase().starts_with("CREATE TABLE") {
                conn.execute_batch(sql).unwrap();
            }
        }
        let mut session = Session::new(&conn).unwrap();
        session.attach::<&str>(None).unwrap();
        for &sql in statements {
            if !sql.trim().to_uppercase().starts_with("CREATE TABLE") {
                conn.execute_batch(sql).unwrap();
            }
        }
        extract(&mut session)
    }

    let changeset = run_session(statements, |session| {
        let mut buf = Vec::new();
        session.changeset_strm(&mut buf).unwrap();
        buf
    });
    let patchset = run_session(statements, |session| {
        let mut buf = Vec::new();
        session.patchset_strm(&mut buf).unwrap();
        buf
    });

    (changeset, patchset)
}

/// Create an in-memory `SQLite` database, run pre-session setup DML, attach a
/// session, run tracked DML, and return the raw changeset and patchset bytes.
///
/// Unlike [`session_changeset_and_patchset`], this helper takes two SQL lists:
/// `setup` runs before `Session::attach` (so its writes are NOT recorded), and
/// `tracked` runs after (so only those writes end up in the emitted bytes).
/// This lets tests exercise a standalone `UPDATE` or `DELETE` against a
/// pre-existing row, which is the only way to see SQLite's real patchset
/// UPDATE wire layout: `INSERT` + `UPDATE` inside the same session
/// consolidates to `INSERT`, hiding the UPDATE format.
///
/// # Panics
///
/// Panics if database creation, statement execution, or session operations fail.
#[must_use]
pub fn session_changeset_and_patchset_with_setup(
    setup: &[&str],
    tracked: &[&str],
) -> (Vec<u8>, Vec<u8>) {
    fn run_session(
        setup: &[&str],
        tracked: &[&str],
        extract: impl Fn(&mut Session<'_>) -> Vec<u8>,
    ) -> Vec<u8> {
        let conn = Connection::open_in_memory().unwrap();
        for &sql in setup {
            conn.execute_batch(sql).unwrap();
        }
        let mut session = Session::new(&conn).unwrap();
        session.attach::<&str>(None).unwrap();
        for &sql in tracked {
            conn.execute_batch(sql).unwrap();
        }
        extract(&mut session)
    }

    let changeset = run_session(setup, tracked, |session| {
        let mut buf = Vec::new();
        session.changeset_strm(&mut buf).unwrap();
        buf
    });
    let patchset = run_session(setup, tracked, |session| {
        let mut buf = Vec::new();
        session.patchset_strm(&mut buf).unwrap();
        buf
    });

    (changeset, patchset)
}

/// Pretty-print a byte-level diff between two changeset/patchset buffers.
///
/// Returns a human-readable string describing where they differ.
#[must_use]
pub fn byte_diff_report(label: &str, expected: &[u8], actual: &[u8]) -> String {
    if expected == actual {
        return format!("{label}: MATCH ({} bytes)", expected.len());
    }

    let mut report = format!(
        "{label}: MISMATCH\n  expected len: {}\n  actual len:   {}\n",
        expected.len(),
        actual.len()
    );

    // Find first divergence point
    let min_len = expected.len().min(actual.len());
    let first_diff = (0..min_len).find(|&i| expected[i] != actual[i]);

    if let Some(pos) = first_diff {
        let _ = writeln!(
            report,
            "  first diff at byte {pos}: expected 0x{:02x}, actual 0x{:02x}",
            expected[pos], actual[pos]
        );
        // Show context around the diff
        let start = pos.saturating_sub(4);
        let end = (pos + 8).min(min_len);
        let _ = writeln!(
            report,
            "  expected[{start}..{end}]: {:02x?}",
            &expected[start..end]
        );
        let _ = writeln!(
            report,
            "  actual  [{start}..{end}]: {:02x?}",
            &actual[start..end]
        );
    } else {
        report.push_str("  common prefix matches, difference is in length only\n");
    }

    let _ = writeln!(report, "  expected: {expected:02x?}");
    let _ = writeln!(report, "  actual:   {actual:02x?}");

    report
}

/// Assert byte-for-byte equality between our output and rusqlite's output,
/// for both changeset and patchset.
///
/// # Panics
///
/// Panics with a detailed diff report if the bytes don't match.
pub fn assert_bit_parity(sql_statements: &[&str], our_changeset: &[u8], our_patchset: &[u8]) {
    let (sqlite_changeset, sqlite_patchset) = session_changeset_and_patchset(sql_statements);

    let cs_report = byte_diff_report("changeset", &sqlite_changeset, our_changeset);
    let ps_report = byte_diff_report("patchset", &sqlite_patchset, our_patchset);

    assert!(
        sqlite_changeset == our_changeset && sqlite_patchset == our_patchset,
        "Bit parity failure!\n\n{cs_report}\n{ps_report}\n\nSQL:\n{}",
        sql_statements.join("\n")
    );
}

/// Run bit-parity test by digesting SQL into a `PatchSet` via `digest_sql`,
/// serializing to bytes, and comparing the patchset with rusqlite's output.
/// Only patchset parity is tested because SQL digestion is patchset-only.
///
/// The `schemas` must be pre-built [`SimpleTable`]s matching the CREATE TABLE
/// statements in `sql_statements`. The builder is seeded with these schemas
/// before digesting.
///
/// # Panics
///
/// Panics if parsing fails or if the bytes don't match.
pub fn assert_patchset_sql_parity(schemas: &[SimpleTable], sql_statements: &[&str]) {
    let mut patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new();

    // Build a lookup map so we can register tables on first DML reference
    let schema_map: std::collections::HashMap<&str, &SimpleTable> =
        schemas.iter().map(|s| (s.name(), s)).collect();

    // Digest DML statements, registering each table on first touch
    for dml in sql_statements
        .iter()
        .filter(|s| !s.trim().to_uppercase().starts_with("CREATE"))
    {
        // Extract the table name from the DML to ensure proper registration order
        let upper = dml.trim().to_uppercase();
        let table_name = if upper.starts_with("INSERT INTO") {
            dml.trim()["INSERT INTO".len()..].split_whitespace().next()
        } else if upper.starts_with("UPDATE") {
            dml.trim()["UPDATE".len()..].split_whitespace().next()
        } else if upper.starts_with("DELETE FROM") {
            dml.trim()["DELETE FROM".len()..].split_whitespace().next()
        } else {
            None
        };

        if let Some(name) = table_name
            && let Some(schema) = schema_map.get(name)
        {
            patchset.add_table(schema);
        }
        patchset.digest_sql(dml).unwrap();
    }

    let our_patchset: Vec<u8> = patchset.build();
    let (_, sqlite_patchset) = session_changeset_and_patchset(sql_statements);

    let ps_report = byte_diff_report("patchset", &sqlite_patchset, &our_patchset);
    assert!(
        sqlite_patchset == our_patchset,
        "Patchset bit parity failure!\n\n{ps_report}\n\nSQL:\n{}",
        sql_statements.join("\n")
    );
}

/// Apply a changeset or patchset to a database connection.
///
/// Uses `SQLITE_CHANGESET_ABORT` on conflict.
///
/// # Errors
///
/// Returns an error if the changeset application fails.
pub fn apply_changeset(conn: &Connection, changeset: &[u8]) -> Result<(), rusqlite::Error> {
    use rusqlite::session::{ChangesetItem, ConflictAction, ConflictType};
    let mut cursor = Cursor::new(changeset);
    conn.apply_strm(
        &mut cursor,
        None::<fn(&str) -> bool>,
        |_conflict_type: ConflictType, _item: ChangesetItem| ConflictAction::SQLITE_CHANGESET_ABORT,
    )
}

/// Query all rows from a table as a sorted vector of string-formatted values.
///
/// Rows are sorted for order-independent comparison.
pub fn get_all_rows(conn: &Connection, table_name: &str) -> Vec<Vec<String>> {
    let query = format!("SELECT * FROM {table_name} ORDER BY rowid");
    let Ok(mut stmt) = conn.prepare(&query) else {
        return Vec::new();
    };

    let column_count = stmt.column_count();
    let rows_result = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..column_count {
            let value: rusqlite::types::Value = row.get(i).unwrap_or(rusqlite::types::Value::Null);
            values.push(format!("{value:?}"));
        }
        Ok(values)
    });

    let mut rows: Vec<Vec<String>> = match rows_result {
        Ok(mapped) => mapped.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    };

    // Sort for order-independent comparison
    rows.sort();
    rows
}

/// Run all crash files in a directory through a test function, with timing
/// and auto-copy from the cargo-fuzz artifacts directory.
///
/// Shared implementation behind the per-target regression tests (for example
/// `roundtrip`, `apply_roundtrip`). Ensures `crash_dir` exists, copies any
/// file from `fuzz_source_dir` that is not already there, runs
/// `test_fn` on every file in `crash_dir` enforcing `time_limit` per input,
/// and panics with a summary if any input fails or exceeds the time limit.
/// Returns the number of files tested.
///
/// # Panics
///
/// Panics if any test input causes `test_fn` to panic, or if any input exceeds
/// the time limit.
pub fn run_crash_dir_regression(
    crash_dir: &str,
    fuzz_source_dir: &str,
    time_limit: std::time::Duration,
    test_fn: impl Fn(&[u8]),
) -> usize {
    use std::fs;
    use std::time::Instant;

    // Ensure crash_inputs directory exists
    let _ = fs::create_dir_all(crash_dir);

    // Copy any new crash files from the fuzz artifacts
    if let Ok(fuzz_entries) = fs::read_dir(fuzz_source_dir) {
        for entry in fuzz_entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let dest = format!(
                    "{}/{}",
                    crash_dir,
                    path.file_name().unwrap().to_string_lossy()
                );
                if !std::path::Path::new(&dest).exists() {
                    let _ = fs::copy(&path, &dest);
                }
            }
        }
    }

    let Ok(entries) = fs::read_dir(crash_dir) else {
        return 0;
    };

    let mut tested = 0;
    let mut failures: Vec<String> = Vec::new();
    let mut slow_inputs: Vec<String> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: read error: {e}", path.display()));
                continue;
            }
        };

        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let start = Instant::now();

        // Use catch_unwind to collect panics without aborting the loop
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            test_fn(&data);
        }));

        if let Err(panic) = result {
            let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            failures.push(format!("{filename}: {msg}"));
        }

        let elapsed = start.elapsed();
        if elapsed > time_limit {
            slow_inputs.push(format!(
                "{filename}: {:.3}s (limit: {:.1}s) [{} bytes]",
                elapsed.as_secs_f64(),
                time_limit.as_secs_f64(),
                data.len(),
            ));
        }

        tested += 1;
    }

    assert!(
        failures.is_empty(),
        "Failures in {}/{tested} crash files:\n{}",
        failures.len(),
        failures.join("\n")
    );

    assert!(
        slow_inputs.is_empty(),
        "Timeout-class bugs: {}/{tested} inputs exceeded {:.1}s limit:\n{}",
        slow_inputs.len(),
        time_limit.as_secs_f64(),
        slow_inputs.join("\n")
    );

    tested
}

// ---------------------------------------------------------------------------
// Wire adapter fuzz helpers (0.2.0+)
// ---------------------------------------------------------------------------

/// Every [`WireType`](crate::wire::WireType) variant, used by the wire
/// fuzz helpers to exercise each default decoder. `ScaledDecimal` appears
/// at scale 0, at a typical scale and at one where any nonzero integer
/// part overflows `i64`, and `Array` with elements decoding to an integer,
/// a boolean integer, a real, text and a blob JSON cannot hold.
const ALL_WIRE_TYPES: [crate::wire::WireType; 22] = [
    crate::wire::WireType::Bool,
    crate::wire::WireType::Int,
    crate::wire::WireType::Real,
    crate::wire::WireType::Text,
    crate::wire::WireType::Bytes,
    crate::wire::WireType::Uuid,
    crate::wire::WireType::Decimal,
    crate::wire::WireType::ScaledDecimal { scale: 0 },
    crate::wire::WireType::ScaledDecimal { scale: 2 },
    crate::wire::WireType::ScaledDecimal { scale: 19 },
    crate::wire::WireType::Timestamp,
    crate::wire::WireType::TimestampTz,
    crate::wire::WireType::Date,
    crate::wire::WireType::Time,
    crate::wire::WireType::Interval,
    crate::wire::WireType::Json,
    crate::wire::WireType::Jsonb,
    crate::wire::WireType::Array {
        element: crate::wire::ArrayElement::Int,
    },
    crate::wire::WireType::Array {
        element: crate::wire::ArrayElement::Bool,
    },
    crate::wire::WireType::Array {
        element: crate::wire::ArrayElement::Real,
    },
    crate::wire::WireType::Array {
        element: crate::wire::ArrayElement::Text,
    },
    crate::wire::WireType::Array {
        element: crate::wire::ArrayElement::Bytes,
    },
];

/// Feed arbitrary bytes into every built-in decoder for the [`PgBinary`](crate::PgBinary)
/// source via [`TypeMap::defaults`](crate::wire::TypeMap::defaults), as both a present
/// binary field and a SQL NULL. Asserts nothing panics.
pub fn test_wire_pg_binary(input: &[u8]) {
    use crate::wire::{PgBinary, PgBinaryColumn, TypeMap, WireAdapter};

    let types: TypeMap<PgBinary, alloc::string::String, Vec<u8>> = TypeMap::defaults();

    for wire_type in ALL_WIRE_TYPES {
        for raw in [Some(input), None] {
            let _ = types.decode(PgBinaryColumn {
                column_name: "c",
                wire_type,
                raw,
            });
        }
    }
}

/// Feed arbitrary bytes into every built-in decoder for the
/// `pg_walstream` source via [`TypeMap::defaults`](crate::wire::TypeMap::defaults). Asserts nothing
/// panics.
#[cfg(feature = "pg-walstream")]
pub fn test_wire_pg_walstream(input: &[u8]) {
    use crate::pg_walstream::{ColumnValue, PgWalstream, PgWalstreamColumn};
    use crate::wire::{TypeMap, WireAdapter};

    // Text-mode ColumnValue is the more common production wire shape
    // and covers every decoder path that touches vendored code
    // (hex-escape, base64, UUID parse, int/real parse, JSON canon).
    let Ok(text) = core::str::from_utf8(input) else {
        return;
    };
    let text_cv = ColumnValue::text(text);

    let types: TypeMap<PgWalstream, alloc::string::String, Vec<u8>> = TypeMap::defaults();

    for wire_type in ALL_WIRE_TYPES {
        let _ = types.decode(PgWalstreamColumn {
            column_name: "c",
            wire_type,
            data: &text_cv,
        });
    }
}

/// Feed arbitrary bytes as a JSON string into every wal2json type-key
/// via [`TypeMap::defaults`](crate::wire::TypeMap::defaults). Also tries the input as raw JSON.
#[cfg(feature = "wal2json")]
pub fn test_wire_wal2json(input: &[u8]) {
    use crate::wal2json::{Wal2Json, Wal2JsonColumn};
    use crate::wire::{TypeMap, WireAdapter};

    let Ok(text) = core::str::from_utf8(input) else {
        return;
    };

    // Two payload flavors: raw string (many decoders accept this) and
    // parsed-JSON (bool/int/real/json flavors need this).
    let string_val = serde_json::Value::String(text.into());
    let parsed_val =
        serde_json::from_str::<serde_json::Value>(text).unwrap_or(serde_json::Value::Null);

    let types: TypeMap<Wal2Json, alloc::string::String, Vec<u8>> = TypeMap::defaults();

    for wire_type in ALL_WIRE_TYPES {
        for value in [&string_val, &parsed_val] {
            let _ = types.decode(Wal2JsonColumn {
                column_name: "c",
                wire_type,
                value,
            });
        }
    }
}

/// Feed arbitrary bytes as a JSON string into every maxwell type-key
/// via [`TypeMap::defaults`](crate::wire::TypeMap::defaults).
#[cfg(feature = "maxwell")]
pub fn test_wire_maxwell(input: &[u8]) {
    use crate::maxwell::{Maxwell, MaxwellColumn};
    use crate::wire::{TypeMap, WireAdapter};

    let Ok(text) = core::str::from_utf8(input) else {
        return;
    };

    let string_val = serde_json::Value::String(text.into());
    let parsed_val =
        serde_json::from_str::<serde_json::Value>(text).unwrap_or(serde_json::Value::Null);

    let types: TypeMap<Maxwell, alloc::string::String, Vec<u8>> = TypeMap::defaults();

    for wire_type in ALL_WIRE_TYPES {
        for value in [&string_val, &parsed_val] {
            let _ = types.decode(MaxwellColumn {
                column_name: "c",
                wire_type,
                value,
            });
        }
    }
}
