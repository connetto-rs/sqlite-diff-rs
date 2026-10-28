# sqlite-diff-rs — Copilot Instructions

> **Purpose**: Behavioral guide + architecture reference for AI coding agents working
> on this codebase. Section A tells you *how to work*; Section B tells you *what the code does*.

---

# A. Working Guidelines

## A1. Verification Workflow

After **every** code change, run:

```bash
cargo test --all-features
```

This exercises the full test suite including bit-parity checks against rusqlite.
If only touching non-`testing`-gated code, a quick `cargo test` (no features)
can catch compile errors faster, but always finish with `--all-features`.

Before committing, also confirm:

```bash
cargo clippy --all-features -- -D warnings
```

The project enforces **clippy::pedantic** at deny level. Key lint overrides
(allowed) are: `unreadable_literal`, `missing_panics_doc`, `new_ret_no_self`,
`struct_excessive_bools`, `should_panic_without_expect`, `doc_markdown`.
`clone_on_copy` is **forbidden**.

## A2. Rust Constraints

- **Edition 2024** — use `let` chains, `gen` blocks, etc. where appropriate.
- **`#![no_std]`** with `extern crate alloc` — never use `std::` types in
  library code. `std` is only available behind `#[cfg(feature = "testing")]`
  or in `#[cfg(test)]` modules.
- **Every public item must have a doc comment** — `missing_docs = "forbid"`.
- All rustdoc code blocks must compile — `invalid_rust_codeblocks = "forbid"`.

## A3. Critical Invariants — Do Not Break

1. **Bit-perfect parity**: Binary output must be byte-identical to rusqlite's
   session extension for the same operations. The `bit_parity.rs` integration
   tests enforce this. If you change anything in `builders/change.rs` (especially
   `session_row_order`, `build`, or `add_operation`), run the full test suite
   and confirm all `bit_parity_*` tests pass.

2. **Row ordering via hash simulation**: `session_row_order()` in
   `builders/change.rs` replicates SQLite's internal hash table. The algorithm
   (256 initial buckets, doubles at ≥ half-full, prepend to chains, walk
   buckets 0‥n−1) is load-bearing. Do not refactor it without differential
   fuzzing confirmation.

3. **Consolidation rules**: `Operation::merge` in `builders/operation.rs`, one
   per format and reached through `Format::merge`, matches SQLite's
   `sqlite3changeset_concat()`. See the consolidation table in Section B4.
   `tests/concat_parity.rs` compares both formats with SQLite byte for byte.

4. **NaN → Null, −0.0 kept**: SQLite never stores NaN, so encoding, decoding
   and the wire `RealDecoder`s map it to Null (fuzz regression #2). `-0.0`
   keeps its sign, as SQLite records it for an untyped column (fuzz regression #4).

## A4. Adding a New Feature — Checklist

- [ ] Add doc comments on every public item.
- [ ] If the feature touches binary encoding, add a `bit_parity_*` test in
      `tests/bit_parity.rs` that compares against rusqlite.
- [ ] If the feature adds a new fuzz-testable surface, add a harness in
      `fuzz/fuzz_targets/`.
- [ ] Run `cargo test --all-features` and `cargo clippy --all-features -- -D warnings`.

## A5. Code Style Patterns

- **Builder pattern**: Operations (`Insert`, `ChangeDelete`, `Update`) use
  consuming builder chains: `Insert::from(schema).set(0, val).unwrap().set(1, val).unwrap()`.
- **Generic value types**: `Value<S, B>` is generic over string and blob storage.
  Most concrete code uses `Value<String, Vec<u8>>`. When writing new code,
  keep generic bounds as tight as possible (`AsRef<str>`, `AsRef<[u8]>`).
- **Error handling**: Use `Result` with the crate's error types. Panics are only
  acceptable in `debug_assert!` or test code.
- **Feature gating**: Anything that depends on `rusqlite` or `arbitrary` must be
  behind `#[cfg(feature = "testing")]`.

## A6. File Layout Conventions

| Area | Location |
|------|----------|
| Library source | `src/` |
| Integration tests | `tests/` |
| Fuzz targets | `fuzz/fuzz_targets/` |
| Fuzz crash regressions | `tests/crash_inputs/<harness>/` |
| Benchmarks | `benches/` |
| Full-stack demo | `integration-tests/` |

---

# B. Architecture Reference

## B1. Project Overview

`sqlite-diff-rs` is a `#![no_std]`-compatible Rust library for **building and parsing
SQLite changeset and patchset binary formats** without requiring a live SQLite database.
It implements the [SQLite session extension](https://www.sqlite.org/session.html) wire
format, enabling:

- Offline sync (server builds changesets → client applies via `sqlite3_changeset_apply()`)
- Testing (generate changeset/patchset fixtures programmatically)
- CDC pipelines (produce binary changesets from custom change events)

**Crate name**: `sqlite-diff-rs` (on crates.io)
**Edition**: 2024, **License**: MIT
**Repository**: <https://github.com/connetto-rs/sqlite-diff-rs>

### Key Design Decisions

- **`#![no_std]`** with `extern crate alloc` — works in WASM and embedded.
- **Zero-copy generics** — `Value<S, B>` is generic over string (`S: AsRef<str>`)
  and blob (`B: AsRef<[u8]>`) types, enabling both owned (`String`, `Vec<u8>`) and
  borrowed (`&str`, `&[u8]`) usage.
- **Bit-perfect parity with SQLite** — the library replicates SQLite's session
  extension hash-table ordering, so binary output is byte-identical to rusqlite's.
- **Consolidation semantics** — multiple operations on the same PK are merged
  using SQLite's `sqlite3changegroup_add()` rules.

---

## B2. Binary Format

Both changesets (`'T'` / `0x54`) and patchsets (`'P'` / `0x50`) share a container
structure of table sections, each with a header followed by change records.

### Table Header

```
├── Marker: 'T' (0x54) for changeset, 'P' (0x50) for patchset
├── Column count (1 byte)
├── PK flags (1 byte per column: ordinal position in composite PK, 0 = not PK)
└── Table name (null-terminated UTF-8)
```

### Change Records

```
├── Operation code: INSERT=0x12, DELETE=0x09, UPDATE=0x17
├── Indirect flag (1 byte, always 0 in this library)
└── Values (format-dependent)
```

### Value Encoding (NOT the same as SQLite database record format!)

| Type byte | Meaning | Payload |
|-----------|---------|---------|
| 0 | Undefined (unchanged column in UPDATE) | none |
| 1 | INTEGER | 8 bytes big-endian i64 |
| 2 | FLOAT | 8 bytes big-endian IEEE 754 |
| 3 | TEXT | varint length + UTF-8 bytes |
| 4 | BLOB | varint length + raw bytes |
| 5 | NULL | none |

### Changeset vs Patchset Differences

| Aspect | Changeset (`'T'`) | Patchset (`'P'`) |
|--------|-------------------|------------------|
| INSERT | All column values | All column values |
| DELETE | All old column values | PK values only |
| UPDATE old | All old column values | PK values only (in column order, no padding for non-PK) |
| UPDATE new | All new column values | Non-PK columns only (in column order); each entry is either the new value or `0x00` (undefined) if unchanged |
| Reversible | Yes | No |
| Wire size | Larger | Smaller |

### Varint Encoding

SQLite changesets use big-endian 7-bit continuation encoding:
- High bit = 1 means more bytes follow
- The remaining 7 bits carry data, MSB first

Constants: `src/encoding/constants.rs` — op codes and table markers.

---

## B3. Source Code Map

### `src/lib.rs` — Crate root

Re-exports the main public API. Key re-exports:

| Symbol | From | Purpose |
|--------|------|---------|
| `DiffSetBuilder` | `builders::change` | Core builder for changesets/patchsets |
| `ChangeSet` | `builders::change` | Type alias: `DiffSetBuilder<ChangesetFormat, T, S, B>` |
| `PatchSet` | `builders::change` | Type alias: `DiffSetBuilder<PatchsetFormat, T, S, B>` |
| `Insert` | `builders::insert_operation` | Insert operation builder |
| `ChangeDelete` | `builders::delete_operation` | Delete operation builder (changeset) |
| `Update` | `builders::update_operation` | Update operation builder |
| `ChangeUpdate` | lib.rs | Type alias: `Update<T, ChangesetFormat, S, B>` |
| `PatchUpdate` | lib.rs | Type alias: `Update<T, PatchsetFormat, S, B>` |
| `Reverse` | `builders::operation` | Trait for reversing operations |
| `Value` | `encoding::serial` | The core value enum |
| `ParsedDiffSet` | `parser` | Parsed changeset/patchset enum |
| `TableSchema` | `parser` | Schema parsed from binary data |
| `DynTable` | `schema::dyn_table` | Dynamic table trait |
| `SchemaWithPK` | `schema::dyn_table` | Schema trait with PK extraction |
| `SimpleTable` | `schema::simple_table` | Schema with column names, declared types and rowid alias (for SQL) |
| `Error` | `errors` | Crate error type |

### Feature flags

| Feature | Enables | Dependencies |
|---------|---------|--------------|
| *(default)* | Core library only | — |
| `testing` | `testing` and `differential_testing` modules | `rusqlite`, `arbitrary` |

---

### `src/encoding/` — Binary encoding utilities

| File | Purpose |
|------|---------|
| `constants.rs` | Op codes (`INSERT=0x12`, `DELETE=0x09`, `UPDATE=0x17`) and markers (`CHANGESET=b'T'`, `PATCHSET=b'P'`) |
| `varint.rs` | `encode_varint()` / `decode_varint()` — big-endian 7-bit continuation integers |
| `serial.rs` | `Value<S, B>` enum + `encode_value()` / `decode_value()` / `encode_undefined()` |
| `serial/display.rs` | `Display` impl for `Value` |

`Value<S, B>` variants: `Null`, `Integer(i64)`, `Real(f64)`, `Text(S)`, `Blob(B)`.

`MaybeValue<S, B>` = `Option<Value<S, B>>` — `None` means "undefined" (unchanged in UPDATE).

**Important**: NaN is decoded as Null, since SQLite never stores it. `-0.0` keeps its sign.
See fuzz regressions #2 and #4.

---

### `src/schema/` — Table schema traits

| File | Type | Purpose |
|------|------|---------|
| `dyn_table.rs` | `DynTable` trait | Object-safe: `name()`, `number_of_columns()`, `write_pk_flags()` |
| `dyn_table.rs` | `SchemaWithPK` trait | Extends `DynTable` with `extract_pk()`, `number_of_primary_keys()`, `primary_key_index()` |
| `dyn_table.rs` | `IndexableValues` trait | Internal: get a `Value` by column index from various collection types |
| `simple_table.rs` | `SimpleTable` struct | Schema with column names, per-column `Affinity` and an optional rowid alias, wrapping `TableSchema<String>` |
| `simple_table.rs` | `NamedColumns` trait | `column_index(name)`, `column_affinity(index)`, `rowid_alias()`, needed for SQL digestion |

**PK flags format**: Each byte is the 1-based ordinal position in the composite PK
(e.g., `[2, 1, 0]` means col 0 is 2nd PK, col 1 is 1st PK, col 2 is not PK).
A value of `0` means the column is not part of the PK.

**Schema hierarchy**:
```
DynTable (object-safe, basic schema)
  └── SchemaWithPK (extract_pk, not object-safe due to generics)
        └── NamedColumns (column_index by name, needed for SQL parsing)
```

Concrete implementors:
- `TableSchema<S>` — from binary parser, no column names (implements `DynTable`, `SchemaWithPK`)
- `SimpleTable` wraps `TableSchema<String>` with column names and affinities (implements all three traits)

---

### `src/builders/` — Builder pattern for constructing changesets/patchsets

| File | Type | Purpose |
|------|------|---------|
| `format.rs` | `Format` trait | Sealed marker: `ChangesetFormat` vs `PatchsetFormat` |
| `format.rs` | `ChangesetFormat` | `TABLE_MARKER = b'T'`, `Old = MaybeValue`, `DeleteData = Vec<Value>` |
| `format.rs` | `PatchsetFormat` | `TABLE_MARKER = b'P'`, `Old = ()`, `DeleteData = ()` |
| `operation.rs` | `Operation<F, S, B>` | Schema-less enum: `Insert(Vec<Value>)`, `Delete(F::DeleteData)`, `Update(Vec<(F::Old, MaybeValue)>)` |
| `operation.rs` | `Reverse` trait | `reverse()` on changeset builders, via `Operation::invert`, which matches `sqlite3changeset_invert()` |
| `operation.rs` | `Operation::merge` | Consolidation rules (op, op, table → Option<op>) for both formats |
| `insert_operation.rs` | `Insert<T, S, B>` | Builder: `From<T>`, `.set(col, value)` chain |
| `delete_operation.rs` | `ChangeDelete<T, S, B>` | Changeset delete builder: `.set(col, value)` chain |
| `update_operation.rs` | `Update<T, F, S, B>` | Update builder: `.set(col, old, new)`, `.set_old(col, old)` and `.set_new(col, new)` for changeset, `.set(col, new)` for patchset |
| `change.rs` | `DiffSetBuilder<F, T, S, B>` | **Core builder** — `insert()`, `delete()`, `update()`, `build()`, `digest_sql()` |

#### `DiffSetBuilder` internals

```rust
struct DiffSetBuilder<F: Format<S, B>, T: SchemaWithPK, S, B> {
    tables: IndexMap<T, IndexMap<Vec<Value<S, B>>, Operation<F, S, B>>>,
}
```

- Outer `IndexMap<T, ...>` — tables in first-touch insertion order
- Inner `IndexMap<Vec<Value<S, B>>, Operation<F, S, B>>` — rows keyed by PK values

**Consolidation** (`add_operation`): When a new operation targets an existing PK,
the library applies SQLite's `sqlite3changeset_concat()` rules via `Format::merge`:

| Existing | New | Result |
|----------|-----|--------|
| INSERT | INSERT | Keep first |
| INSERT | UPDATE | INSERT with updated values |
| INSERT | DELETE | Remove both (no-op) |
| UPDATE | INSERT | Keep update |
| UPDATE | UPDATE | Per column the first defined old and last defined new value. A non-key column left equal becomes undefined, and the UPDATE is dropped when nothing changes |
| UPDATE | DELETE | DELETE of the update's old values, the delete's where undefined |
| DELETE | INSERT | UPDATE of the differing columns, key in the old record only, no-op if same |
| DELETE | UPDATE | Keep delete |
| DELETE | DELETE | Keep first |

**Row ordering in `build()`**: The `session_row_order()` function simulates SQLite's
internal hash table to produce byte-identical output. SQLite uses a hash table that:
- Starts at 256 buckets, doubles when entries ≥ buckets/2
- New entries are prepended to bucket chains
- Iteration walks buckets 0..n-1, following chains head-to-tail
- Hash function: `session_hash_pk()` using `HASH_APPEND(h, add) = (h << 3) ^ h ^ add`

#### SQL Digestion (`digest_sql`)

Only available for `PatchsetFormat` with `NamedColumns` schemas:

```rust
impl DiffSetBuilder<PatchsetFormat, T: NamedColumns, S, Vec<u8>> {
    fn digest_sql(&mut self, input: &str) -> Result<&mut Self, ParseError>;
}
```

Uses a lightweight SQL parser (`src/builders/sql/`) that handles only:
- `INSERT INTO table (cols...) VALUES (vals...)`
- `UPDATE table SET col=val,... WHERE pk_col=val AND ...`
- `DELETE FROM table WHERE pk_col=val AND ...`

The SQL parser consists of `lexer.rs` (tokenizer) and `parser.rs` (statement parser).

---

### `src/parser.rs` — Binary format parser

Parses raw `&[u8]` changeset/patchset bytes into `DiffSetBuilder` instances.

Key type: `ParsedDiffSet` — an enum wrapping either a changeset or patchset builder:
```rust
enum ParsedDiffSet {
    Changeset(DiffSetBuilder<ChangesetFormat, TableSchema<String>, String, Vec<u8>>),
    Patchset(DiffSetBuilder<PatchsetFormat, TableSchema<String>, String, Vec<u8>>),
}
```

Conversions:
- `ParsedDiffSet::try_from(&[u8])` — parse binary
- `Vec<u8>::from(ParsedDiffSet)` — serialize back

Empty `ParsedDiffSet` values are considered equal regardless of format variant
(since empty bytes can't distinguish changeset from patchset).

---

### `src/errors.rs` — Error types

Single error type: `Error::ColumnIndexOutOfBounds(usize, usize)`.

Parse errors are in `parser::ParseError` and `builders::sql::ParseError`.

---

### `src/testing.rs` — Test utilities (feature-gated: `testing`)

| Helper | Purpose |
|--------|---------|
| `SqlType` | Declared column types (`Integer`, `Text`, `Real`, `Blob`, `Numeric`, `Untyped`) |
| `TypedSimpleTable` | `SimpleTable` plus column types. A lone `INTEGER` key is a rowid alias. `Display` emits `CREATE TABLE` DDL, and it implements `Arbitrary` |
| `session_changeset_and_patchset(sqls)` | Execute SQL in rusqlite, capture raw changeset + patchset bytes |
| `byte_diff_report(label, expected, actual)` | Human-readable byte-level diff |
| `assert_bit_parity(sqls, our_cs, our_ps)` | Assert byte-equality with rusqlite output |
| `assert_patchset_sql_parity(schemas, sqls)` | Digest SQL → build patchset → compare with rusqlite |
| `test_roundtrip(bytes)` | Parse → serialize → reparse → assert equal |
| `test_apply_roundtrip(schema, bytes)` | Roundtrip + apply changeset to in-memory DB |
| `test_reverse_idempotent(bytes)` | `reverse(reverse(x)) == x` for changesets |
| `test_sql_roundtrip(schema, sql)` | SQL digest → serialize → reparse → assert equal |
| `test_differential(schema, sql)` | Compare our patchset with rusqlite byte-for-byte |
| `apply_changeset(conn, bytes)` | Apply changeset via `conn.apply_strm()` |
| `get_all_rows(conn, table)` | Query all rows for comparison |
| `run_crash_dir_regression(...)` | Run fuzz crash files with timeout enforcement |

### `src/differential_testing.rs` — Differential testing (feature-gated: `testing`)

`run_differential_test(schemas, create_sqls, dml_sqls)`:
1. Builds our patchset via `digest_sql`
2. Runs same SQL in rusqlite with session tracking
3. Compares bytes

---

## B4. Test Suites

### Unit tests (in-module `#[cfg(test)]`)

- `src/builders/change.rs` — consolidation rules, build output format, reverse trait
- `src/parser.rs` — parsing of headers, operations, table schemas

### Integration tests (`tests/`)

| File | Tests | Requires |
|------|-------|----------|
| `bit_parity.rs` | Byte-identical output vs rusqlite for all operation types | `testing` feature |
| `empty_table_behavior.rs` | Verifies SQLite's behavior when operations cancel out | `testing` feature |
| `reverse.rs` | Reverse trait: apply → reverse → original state | `testing` feature |
| `sql_parsing.rs` | SQL digestion via `digest_sql` | `testing` feature |
| `fuzz_regression.rs` | Replay fuzz crash inputs as regression tests | `testing` feature |

### Running tests

```bash
# All tests (requires testing feature for most)
cargo test --all-features

# Core library tests only (no rusqlite dependency)
cargo test
```

---

## B5. Fuzz Harnesses (`fuzz/`)

Uses [libFuzzer](https://llvm.org/docs/LibFuzzer.html) via `cargo-fuzz`, run by ClusterFuzzLite weekly on `main`.

| Harness | Input | What it tests |
|---------|-------|---------------|
| `roundtrip` | `&[u8]` | Parse → serialize → reparse equality |
| `reverse_idempotent` | `&[u8]` | `reverse(reverse(x)) == x` |
| `apply_roundtrip` | `(TypedSimpleTable, Vec<u8>)` | Roundtrip + apply to DB |
| `sql_roundtrip` | `(TypedSimpleTable, String)` | SQL digest → binary roundtrip |
| `differential` | `(TypedSimpleTable, String)` | Byte-parity with rusqlite |

Crash inputs auto-copy to `tests/crash_inputs/<harness>/` and replay in
`tests/fuzz_regression.rs`.

---

## B6. Benchmarks (`benches/`)

Uses [Criterion.rs](https://github.com/bheisler/criterion.rs).

| Benchmark | What it measures |
|-----------|-----------------|
| `builder_vs_rusqlite` | Builder construction speed vs rusqlite session extension |
| `apply_benchmark` | Time to apply changes: raw SQL vs SQL-in-tx vs patchset vs changeset. Varies by PK type (int/UUID), table state (empty/populated), batch size (30/100/1000), and DB config (base/indexed/triggers/FK) |

---

## B7. Integration Tests Workspace (`integration-tests/`)

A full-stack demo app proving the library works end-to-end:

| Crate | Stack | Purpose |
|-------|-------|---------|
| `chat-shared` | `serde` | Shared message types |
| `chat-backend` | `axum` + `tokio` | WebSocket server, broadcasts patchsets |
| `chat-frontend` | `yew` + `sqlite-wasm-rs` | WASM client, applies patchsets to local SQLite |

Also includes:
- `payload-size-bench` — measures patchset/changeset wire size vs raw JSON
- `apply-bench-report` — generates benchmark plots

---

## B8. Common Patterns

### Creating a changeset

```rust
use sqlite_diff_rs::{ChangeSet, Insert, ChangeDelete, Update, ChangesetFormat, SimpleTable};

let schema = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
let mut cs: ChangeSet<SimpleTable, String, Vec<u8>> = ChangeSet::new();

// Insert
let ins = Insert::from(schema.clone()).set(0, 1i64).unwrap().set(1, "Alice").unwrap();
cs.insert(ins);

// Delete
let del = ChangeDelete::from(schema.clone()).set(0, 2i64).unwrap().set(1, "Bob").unwrap();
cs.delete(del);

// Update
let upd = Update::<_, ChangesetFormat, String, Vec<u8>>::from(schema.clone())
    .set(0, 1i64, 1i64).unwrap()
    .set(1, "Alice", "Alicia").unwrap();
cs.update(upd);

let bytes: Vec<u8> = cs.build();
```

### Creating a patchset via SQL digestion

```rust
use sqlite_diff_rs::{PatchSet, SimpleTable};

let schema = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
let mut ps: PatchSet<SimpleTable, String, Vec<u8>> = PatchSet::new();
ps.add_table(&schema);
ps.digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice')").unwrap();
ps.digest_sql("UPDATE users SET name = 'Alicia' WHERE id = 1").unwrap();
let bytes: Vec<u8> = ps.build();
```

### Parsing binary data

```rust
use sqlite_diff_rs::ParsedDiffSet;

let parsed = ParsedDiffSet::try_from(bytes.as_slice()).unwrap();
match parsed {
    ParsedDiffSet::Changeset(builder) => { /* ... */ }
    ParsedDiffSet::Patchset(builder) => { /* ... */ }
}
```

### Reversing a changeset

```rust
use sqlite_diff_rs::Reverse;

let reversed = changeset_builder.reverse();
let undo_bytes: Vec<u8> = reversed.build();
```

---

## B9. Known Subtleties

1. **Row ordering**: Binary output must match SQLite's hash-table iteration order.
   `session_row_order()` in `builders/change.rs` simulates the hash table growth
   and bucket-chain prepend semantics. Any change to insertion logic must preserve this.

2. **PK flags vs PK indices**: PK flags are 1-based ordinals (stored in binary format).
   PK indices are 0-based column indices in key order. `SchemaWithPK::primary_key_columns()`
   converts between them with an allocation-free selection walk, and the parser refuses a
   header whose nonzero flags are not the dense ordinals `1..=n`.

3. **Empty table serialization**: Tables with no operations are NOT serialized
   (matching SQLite behavior). `DiffSetBuilder::PartialEq` ignores empty tables.
   Tables are kept in memory to preserve insertion order if operations are added later.

4. **NaN and -0.0**: `decode_value` maps NaN to Null and keeps the sign of `-0.0`,
   which SQLite records for an untyped column. Fuzz regressions #2 and #4 pin both.

5. **Patchset-only SQL parsing**: `digest_sql` only works with `PatchsetFormat` because
   SQL DML doesn't provide old-row values needed for changesets.

6. **FORMAT trait sealing**: `Format` is `pub(crate)` and only implemented for
   `ChangesetFormat` and `PatchsetFormat`. The associated types (`Old`, `DeleteData`)
   differ between formats.

7. **`IndexMap` with hashbrown**: Uses `indexmap` with `hashbrown::DefaultHashBuilder`
   for `no_std` compatibility (no `std::collections::HashMap`).

---

## B10. Dependencies

### Runtime (default features)

| Crate | Purpose |
|-------|---------|
| `thiserror` | Error derive macros |
| `hashbrown` | `no_std` hash map |
| `indexmap` | Insertion-ordered map (table and row ordering) |

### Optional (`testing` feature)

| Crate | Purpose |
|-------|---------|
| `rusqlite` | Reference SQLite implementation for differential testing |
| `arbitrary` | Structured fuzzing input generation |

### Dev-only

| Crate | Purpose |
|-------|---------|
| `rand`, `uuid`, `hex` | Test data generation |
| `criterion` | Benchmarking |
