# sqlite-diff-rs

[![Crates.io](https://img.shields.io/crates/v/sqlite-diff-rs.svg)](https://crates.io/crates/sqlite-diff-rs)
[![Documentation](https://docs.rs/sqlite-diff-rs/badge.svg)](https://docs.rs/sqlite-diff-rs)
[![CI](https://github.com/connetto-rs/sqlite-diff-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/connetto-rs/sqlite-diff-rs/actions/workflows/ci.yml)
[![AddressSanitizer](https://github.com/connetto-rs/sqlite-diff-rs/actions/workflows/asan.yml/badge.svg)](https://github.com/connetto-rs/sqlite-diff-rs/actions/workflows/asan.yml)
[![Codecov](https://codecov.io/gh/connetto-rs/sqlite-diff-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/connetto-rs/sqlite-diff-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/connetto-rs/sqlite-diff-rs/blob/main/LICENSE)

A Rust library for building SQLite [changeset and patchset](https://www.sqlite.org/sessionintro.html) binary formats programmatically.

## Overview

SQLite's [session extension](https://www.sqlite.org/session.html) defines a binary format for tracking and applying database changes. This crate constructs that binary data without linking SQLite, which is useful for offline sync (build a changeset on a server and apply it on a SQLite client), CDC pipelines (produce the input expected by `sqlite3_changeset_apply()` from your own change events), cross-database sync (convert PostgreSQL change streams from wal2json or Maxwell into the SQLite format), and generating test fixtures for changeset processing code.

This crate is not the SQLite [`sqldiff`](https://sqlite.org/sqldiff.html) tool, which compares two existing database files. This library constructs the changeset and patchset binary format programmatically from your own change data.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sqlite-diff-rs = "0.10"
```

## Quick Start

```rust
use sqlite_diff_rs::{DiffOps, Insert, PatchSet, SimpleTable};

// users (id INTEGER PRIMARY KEY, name TEXT): declared types drive SQLite's affinity
let users = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);

// Build a patchset with an INSERT
let patchset = PatchSet::<_, String, Vec<u8>>::new()
    .insert(
        Insert::from(users)
            .set(0, 1i64).unwrap()      // id = 1
            .set(1, "Alice").unwrap()   // name = "Alice"
    );

// Encode to binary format
let bytes: Vec<u8> = patchset.into();

// Apply with sqlite3_changeset_apply() or sqlite3session_patchset_apply()
```

### Schema-aware CDC ingest (0.2.0+)

`builder.digest(&event, &schema, &adapter)` folds one wire event (from `pg_walstream`, `wal2json`, or `maxwell`) into a builder. Same call site for every source.

```rust
# #![cfg(feature = "maxwell")]
# use sqlite_diff_rs::{
#     Affinity, DynTable, IndexableValues, NamedColumns, PatchSet, SchemaWithPK, SimpleTable,
#     TypeMap, Value, WireColumnTypes, WireSchema, WireType,
# };
# use sqlite_diff_rs::maxwell::{ConversionError, Maxwell, parse};
# #[derive(Debug, Clone, PartialEq, Eq, Hash)]
# struct UsersTable(SimpleTable);
# impl DynTable for UsersTable {
#     fn name(&self) -> &str { self.0.name() }
#     fn number_of_columns(&self) -> usize { self.0.number_of_columns() }
#     fn write_pk_flags(&self, buf: &mut [u8]) { self.0.write_pk_flags(buf); }
# }
# impl SchemaWithPK for UsersTable {
#     fn extract_pk<S: Clone, B: Clone>(&self, v: &impl IndexableValues<Text = S, Binary = B>) -> Vec<Value<S, B>> { self.0.extract_pk(v) }
#     fn number_of_primary_keys(&self) -> usize { self.0.number_of_primary_keys() }
#     fn primary_key_index(&self, col: usize) -> Option<usize> { self.0.primary_key_index(col) }
# }
# impl NamedColumns for UsersTable {
#     fn column_index(&self, name: &str) -> Option<usize> { self.0.column_index(name) }
#     fn column_affinity(&self, column_index: usize) -> Option<Affinity> { self.0.column_affinity(column_index) }
#     fn rowid_alias(&self) -> Option<usize> { self.0.rowid_alias() }
# }
# impl WireColumnTypes for UsersTable {
#     fn column_type(&self, i: usize) -> WireType { if i == 0 { WireType::Int } else { WireType::Text } }
# }
# struct MySchema { users: UsersTable }
# impl WireSchema for MySchema {
#     type Table = UsersTable;
#     fn get(&self, source_schema: Option<&str>, name: &str) -> Option<&UsersTable> { if source_schema == Some("db") && name == "users" { Some(&self.users) } else { None } }
# }
# let json_ev = r#"{"database":"db","table":"users","type":"insert","ts":0,"data":{"id":1,"name":"Alice"}}"#;
# let msg = parse(json_ev).unwrap();
# let schema = MySchema { users: UsersTable(SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0)) };
# let types = TypeMap::<Maxwell, String, Vec<u8>>::defaults();
let patchset = PatchSet::<UsersTable, String, Vec<u8>>::new()
    .digest(&msg, &schema, &types)?;
let bytes: Vec<u8> = patchset.build();
# assert!(!bytes.is_empty());
# Ok::<_, ConversionError>(())
```

The schema type implements `WireSchema` and its tables implement `WireColumnTypes`, declaring each column's semantic `WireType`. `TypeMap::defaults()` ships with mappings for bool, integers, reals, text, bytea, UUID, decimals as text or as integer minor units at a declared scale, temporals, and JSON.

## Features

| Feature | Description |
|---------|-------------|
| `testing` | Enables `rusqlite` integration for differential testing |
| `wal2json` | Parse PostgreSQL wal2json output into changesets |
| `pg-walstream` | Integration with `pg_walstream` crate |
| `maxwell` | Parse Maxwell CDC JSON events |
| `diesel` | Execute patchsets as backend-generic Diesel queries via a downstream [`Adapter`] |
| `diesel-async` | Apply patchsets and changesets through an async Diesel connection (`diesel-async`) |

Enable features in `Cargo.toml`:

```toml
[dependencies]
sqlite-diff-rs = { version = "0.10", features = ["wal2json"] }
```

## Binary Format Reference

<p align="center">
  <img src="docs/format_illustration.svg" alt="Changeset vs Patchset binary wire format" width="720" />
</p>

Both formats share the same container structure: one or more table sections, each with a table header followed by change records. The key difference is how much old-row data each operation carries.

Changesets (`'T'` / `0x54`) store the complete old state of every column. This makes them reversible, so INSERTs can be turned into DELETEs and vice versa.

Patchsets (`'P'` / `0x50`) omit old values for non-PK columns, producing a smaller, forward-only encoding that cannot be reversed.

| Aspect | Changeset (`'T'`) | Patchset (`'P'`) |
|--------|-------------------|------------------|
| INSERT | All column values | All column values |
| DELETE | All old column values | PK values only |
| UPDATE old | All old column values | PK values + Undefined for non-PK |
| UPDATE new | All new column values | All new column values |
| Reversible | Yes | No |
| Wire size | Larger (carries full old state) | Smaller (omits non-PK old values) |

See the [SQLite session extension docs](https://www.sqlite.org/session.html) for the full specification.

## Apply diffs with Diesel

The `diesel` feature turns each op of a `PatchSet` or `ChangeSet` into a backend-generic Diesel query. Downstream implements one `Adapter` per schema (the set of tables), and it maps `(table_name, column_index)` pairs to column identifiers and to per-column `Binder`s. Each `Binder` calls `push_bind_param` with the target `SqlType` the column expects, so values travel as native binary binds and the emitted SQL contains no `CAST` wrappers regardless of backend. The `ApplyOps` extension trait wraps the batch-execute and `conn.transaction` shapes so a full apply reads as one call. A `ChangeSet` additionally renders primary-key changes, including composite keys, because it carries both the old and the new value of every column. A `PatchSet` stores no new primary-key value, so those updates are changeset-only.

```rust,no_run
# #![cfg(feature = "diesel")]
use diesel::pg::Pg;
use diesel::prelude::*;
use diesel::query_builder::AstPass;
use diesel::result::QueryResult;
use diesel::sql_types::Bool;
use sqlite_diff_rs::{
    Adapter, ApplyOps, Binder, DefaultBinder, DiffOps, Insert, PatchSet, SimpleTable, Value,
};

// A binder for target BOOLEAN columns. Native SQL type, no CAST.
struct BoolBinder(bool);
impl Binder<Pg> for BoolBinder {
    fn walk<'b>(&'b self, out: &mut AstPass<'_, 'b, Pg>) -> QueryResult<()> {
        out.push_bind_param::<Bool, bool>(&self.0)
    }
}

// One adapter per schema, dispatching on (table_name, column_index).
struct MyAdapter;
impl<S: AsRef<str> + Sync, B: AsRef<[u8]> + Sync> Adapter<Pg, S, B> for MyAdapter {
    fn column_name(&self, _table: &str, index: usize) -> &str {
        ["id", "active"][index]
    }
    fn bind<'a>(
        &self,
        table: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> diesel::result::QueryResult<Box<dyn Binder<Pg> + Send + 'a>> {
        match (table, column_index, value) {
            ("users", 1, Value::Integer(v)) => Ok(Box::new(BoolBinder(*v != 0))),
            _ => Ok(Box::new(DefaultBinder::from(value))),
        }
    }
}

// Pass any Connection<Backend = Pg> (e.g. PgConnection::establish("postgres://...")):
fn apply<C: Connection<Backend = Pg>>(conn: &mut C) -> QueryResult<usize> {
    let schema = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
    let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(
        Insert::from(schema.clone())
            .set(0, 1_i64)
            .unwrap()
            .set(1, 1_i64)
            .unwrap(),
    );
    patchset
        .iter()
        .map(|op| op.with_adapter::<Pg, _>(&MyAdapter))
        .apply_transactional(conn)
}
# fn main() {}
```

Enable via `Cargo.toml`:

```toml
[dependencies]
sqlite-diff-rs = { version = "0.10", features = ["diesel"] }
diesel = { version = "2", features = ["postgres"] }
```

End-to-end tests against real SQLite, Postgres, and MySQL containers live under [`integration-tests/diesel-e2e/`](integration-tests/diesel-e2e/). The unit test files [`tests/diesel_patchset.rs`](tests/diesel_patchset.rs) and [`tests/diesel_changeset.rs`](tests/diesel_changeset.rs) hold fully runnable versions of the example above and of the changeset primary-key cases.

The `diesel-async` feature adds the same apply path over an async connection. The rendering is identical (adapters and binders included), so only the driver differs: enable `diesel-async` alongside `diesel`, bring [`ApplyOpsAsync`] into scope, and call `set.iter().apply_async(&mut conn).await` or `set.iter().apply_transactional_async(&mut conn).await` against any `diesel_async::AsyncConnection` (`AsyncPgConnection`, `AsyncMysqlConnection`, or a `SyncConnectionWrapper<SqliteConnection>`). The async end-to-end tests live beside the synchronous ones under [`integration-tests/diesel-e2e/`](integration-tests/diesel-e2e/).

## `no_std` Support

This crate is `no_std` compatible (requires `alloc`). It can be used in embedded or WebAssembly environments.

## License

MIT License, see [LICENSE](https://github.com/connetto-rs/sqlite-diff-rs/blob/main/LICENSE) for details.
