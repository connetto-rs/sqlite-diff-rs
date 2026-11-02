//! Differential testing: compare our patchset builder against rusqlite's session extension.
//!
//! [`run_differential_test`] takes pre-built schemas and SQL DML statements,
//! digests the DML into a patchset via [`DiffSetBuilder::digest_sql`], runs
//! the same SQL through rusqlite with session tracking, and compares the two
//! patchsets byte for byte. Only patchset output is tested because the SQL
//! parser is patchset-only (changesets need old-value tracking).
//!
//! Feature-gated behind `testing`.

extern crate std;

use crate::schema::SimpleTable;
use crate::testing::byte_diff_report;
use crate::{DiffSetBuilder, DynTable, PatchsetFormat};
use alloc::string::String;
use alloc::vec::Vec;
use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::session::Session;
use rusqlite::{Batch, Connection, ErrorCode};

/// Run a differential test comparing our patchset builder output against
/// rusqlite's session extension.
///
/// `schemas` are pre-built [`SimpleTable`] definitions (one per table),
/// `create_sqls` are the matching `CREATE TABLE` strings for rusqlite, and
/// `dml_sqls` are the `INSERT`/`UPDATE`/`DELETE` statements to execute, each
/// string holding one or more statements.
///
/// Schemas are registered in our builder and DML is digested via
/// [`DiffSetBuilder::digest_sql`]. The same statements run in rusqlite with a
/// session attached to each schema's table in schema order, the table order
/// registration gives our builder, and the patchset bytes are compared byte for byte.
///
/// Returns `false` without comparing when a statement changes no row or a
/// constraint refuses it. `digest_sql` records the row a `WHERE` names without
/// knowing whether it exists, so such input has no counterpart session.
/// Returns `true` when the patchsets were compared and matched.
///
/// # Panics
///
/// Panics if the patchset bytes differ, or if SQLite rejects the SQL for any
/// reason other than a constraint (this is a test helper).
#[must_use]
pub fn run_differential_test(
    schemas: &[SimpleTable],
    create_sqls: &[&str],
    dml_sqls: &[&str],
) -> bool {
    if !every_statement_changes_a_row(create_sqls, dml_sqls) {
        return false;
    }

    // Build our patchset via digest_sql
    let mut our_patchset_builder: DiffSetBuilder<PatchsetFormat, SimpleTable, String, Vec<u8>> =
        DiffSetBuilder::default();

    // Register all schemas
    for schema in schemas {
        our_patchset_builder.add_table(schema);
    }

    // Digest each DML statement
    for &dml in dml_sqls {
        if our_patchset_builder.digest_sql(dml).is_err() {
            // Skip statements that fail to parse
        }
    }

    let our_patchset = our_patchset_builder.build();

    let sqlite_patchset = sqlite_patchset(schemas, create_sqls, dml_sqls);

    // Byte-for-byte comparison (patchset only)
    let ps_report = byte_diff_report("patchset", &sqlite_patchset, &our_patchset);

    assert!(
        sqlite_patchset == our_patchset,
        "Patchset bit parity failure in differential test!\n\n{ps_report}\n\nSQL:\n{}\n{}",
        create_sqls.join("\n"),
        dml_sqls.join("\n")
    );
    true
}

/// The patchset SQLite's session records for `dml_sqls`, with each schema's table attached in order.
fn sqlite_patchset(schemas: &[SimpleTable], create_sqls: &[&str], dml_sqls: &[&str]) -> Vec<u8> {
    let conn = Connection::open_in_memory().unwrap();
    for &sql in create_sqls {
        conn.execute_batch(sql).unwrap();
    }
    let mut session = Session::new(&conn).unwrap();
    for schema in schemas {
        session.attach(Some(schema.name())).unwrap();
    }
    for &sql in dml_sqls {
        conn.execute_batch(sql).unwrap();
    }
    let mut patchset = Vec::new();
    session.patchset_strm(&mut patchset).unwrap();
    patchset
}

/// Whether SQLite runs every DML statement and each one changes a row.
///
/// An error other than a constraint violation counts as runnable, so that the
/// session run that follows reports it.
fn every_statement_changes_a_row(create_sqls: &[&str], dml_sqls: &[&str]) -> bool {
    let conn = Connection::open_in_memory().unwrap();
    for &sql in create_sqls {
        conn.execute_batch(sql).unwrap();
    }
    for &sql in dml_sqls {
        let mut batch = Batch::new(&conn, sql);
        loop {
            let mut statement = match batch.next() {
                Ok(Some(statement)) => statement,
                Ok(None) => break,
                Err(_) => return true,
            };
            match statement.execute([]) {
                Ok(_) if conn.changes() == 0 => return false,
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == ErrorCode::ConstraintViolation =>
                {
                    return false;
                }
                Err(_) => return true,
            }
        }
    }
    true
}
