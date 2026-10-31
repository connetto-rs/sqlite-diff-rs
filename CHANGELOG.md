# Changelog

## 0.17.1

### Fixed

`ApplyOpsAsync` runs each op through `diesel_async::methods::ExecuteDsl`, so the `diesel-async` feature builds against diesel_async `main`, whose `RunQueryDsl` covers only the statement types diesel marks with `RunQueryDslSupport`. It still builds against diesel_async 0.9.

## 0.17.0

### Breaking

`TypeMap::register` and `TypeMap::with` take a `WireKind`, the new fieldless key a `WireType` reduces to through `WireType::kind`, and `TypeMap` dispatches on the payload's kind. One registration serves every parameter of a parameterised type, and the decoder reads the parameter from the payload's `wire_type`. Downstreams migrate by writing `WireKind::X` where they registered `WireType::X`.

The `wal2json` and `maxwell` features enable `serde_json/arbitrary_precision`, so every JSON number reaches a decoder with its digits exact. `DecimalTextDecoder` keeps a JSON number's digits, and the never-emitted `DecodeError::DecimalPrecisionLoss` is removed.

### Added

`WireType::ScaledDecimal { scale }` declares a decimal held as an integer count of minor units, `value × 10^scale`, as a `NUMERIC(p,s)` replica column stores it. Every source's `TypeMap::defaults()` registers the new `DecimalMinorUnitsDecoder` for it, which decodes `12.50` at scale 2 to `Integer(1250)` from PostgreSQL binary `numeric`, pgoutput text, and wal2json or Maxwell JSON strings and numbers alike, exponent notation included. A value that is not a whole number of minor units fitting an `i64`, `NaN` and an infinity are refused with the new `DecodeError::UnrepresentableDecimal`, never rounded.

## 0.16.2

### Added

`Update::set_old` sets a changeset column's old value and leaves its new value undefined, which is how SQLite's session records a primary key column of an `UPDATE` that keeps its key.

### Fixed

Consolidating two operations on one row, in a builder or through `|`, gives the operation `sqlite3changeset_concat` gives. An `UPDATE` followed by an `UPDATE` keeps, per column, the first defined old value and the last defined new value, so a column only the first one changed keeps its new value, in changesets and patchsets alike. A changeset `UPDATE` followed by a `DELETE` takes the delete's old value for every column the update left undefined. A changeset `UPDATE` that ends with no column changed is dropped, and a changeset `DELETE` followed by a differing `INSERT` becomes an `UPDATE` of the differing columns only. In a merged changeset `UPDATE` a primary key column holds its old value as the row identity, and a new value only when the key moved, which SQLite never records but the CDC digests do.

`Reverse` on a changeset keeps an `UPDATE`'s primary key in the old record when its new value is undefined, as `sqlite3changeset_invert` does, so reversing an `UPDATE` recorded by SQLite yields a valid changeset.

## 0.16.1

### Added

`ParsedDiffSet::sort_tables_by_key` stably orders table sections by a caller-provided key while preserving each section's schema and operations.

## 0.16.0

### Breaking

`digest_sql` returns the new `ParseError::PrimaryKeyUpdate` for an `UPDATE` whose `SET` gives a primary key column a value other than the one its `WHERE` names. SQLite's session records such a change as a `DELETE` of the old row and an `INSERT` of every column of the new one, and SQL text does not carry the other columns. Setting a key column to its `WHERE` value is still accepted.

A `-0.0` real keeps its sign when a changeset or patchset is built or parsed and in every wire `RealDecoder`, as SQLite records it for an untyped column. NaN still becomes `NULL`.

`differential_testing::run_differential_test` runs each string of SQL statement by statement and returns whether it compared the patchsets. It skips input where a statement changes no row or a constraint refuses it, because `digest_sql` assumes the row a `WHERE` names exists and such input has no counterpart session. The session helpers in `testing` accept several statements per string.

`SimpleTable::new` takes each column as a `(name, declared type)` pair, with `""` for an untyped column, and `SimpleTable::with_rowid_alias` builds a rowid table whose single `INTEGER PRIMARY KEY` column aliases the rowid. `NamedColumns` gains the required methods `column_affinity` and `rowid_alias`. `testing::SqlType` gains `Numeric` and `Untyped`, and `TypedSimpleTable` treats a lone `INTEGER` key as a rowid alias, as the DDL it emits makes it.

`digest_sql` returns the new `ParseError::DatatypeMismatch` when an `INSERT` gives a rowid alias a value that does not convert to an integer, which SQLite refuses, and the new `ParseError::MissingRowid` when an `INSERT` leaves the rowid alias `NULL` or out, because SQLite would assign a rowid the statement does not state.

### Added

`Affinity`, SQLite's column type affinity, with `Affinity::from_declared_type` for SQLite's declared-type rules and `Affinity::apply` for the conversion SQLite applies to a value stored in a column of that affinity, including SQLite's own text rendering of reals.

### Fixed

Changeset and patchset row ordering hashes composite primary keys in table-column order while key accessors retain primary-key ordinal order.

A table header whose varint column count lies near `usize::MAX` returns `ParseError::UnexpectedEof` instead of panicking, which it did on the overflowing bounds check in debug builds and on the wrapped slice range in release builds.

`digest_sql` records a negated real literal such as `-5.0` or `-0.0` as a real, and a negated integer literal past `i64::MIN` such as `-9223372036854775809` as a real, as SQLite does. Only `-9223372036854775808` becomes the integer `i64::MIN`.

An unterminated block comment with text after `/*` in `digest_sql` input runs to the end of the input, as in SQLite, so a statement whose closing parenthesis falls inside it is rejected.

`digest_sql` rejects two statements with no `;` between them, which SQLite refuses as a syntax error.

`digest_sql` rejects a bare `/*` at the end of input because SQLite rejects that syntax.

`digest_sql` input ends at its first NUL byte, as SQLite's does. A comment, string, quoted identifier or blob that a NUL cuts short is read the way SQLite reads it, and text after the NUL is ignored.

`digest_sql` converts every literal by its column's affinity before recording it, as SQLite does, so an integer written into a `TEXT` column is recorded as text and numeric text written into an `INTEGER` column as an integer. `WHERE` literals are converted the same way, and a `WHERE` no row can satisfy records nothing. Real literals are read with SQLite's own decimal rounding.

## 0.15.0

### Breaking

The optional `pg_walstream` dependency moves from 0.8 to 0.9, so the `pg_walstream` and `pg_walstream_reverse` module re-exports resolve to `pg_walstream` 0.9 types, and a consumer of the `pg-walstream` feature that also depends on `pg_walstream` directly must move to 0.9 with it. `pg_walstream` 0.9 decodes a pgoutput timestamp before 1970 into the right instant and clamps one beyond chrono's range to `DateTime::<Utc>::MIN_UTC` or `MAX_UTC`, so `PgOutputDecoder::decode_message` returns an event for every `BEGIN`, `COMMIT` or `PREPARE` message whatever timestamp it carries.

## 0.14.0

### Breaking

`TimestampVerbatimDecoder`, `TimestampTzVerbatimDecoder`, `DateVerbatimDecoder`, `TimeVerbatimDecoder` and `IntervalVerbatimDecoder` are renamed `TimestampDecoder`, `TimestampTzDecoder`, `DateDecoder`, `TimeDecoder` and `IntervalDecoder`, and each stores one text form per type on every source, so a replica holds the same string whether a value arrived as pgoutput text, a pgoutput binary payload, wal2json, Maxwell or a `PgBinary` query result. A `timestamptz` is the UTC instant `YYYY-MM-DD HH:MM:SS.ffffff+00:00`, which orders as text and which SQLite's date functions and diesel's SQLite chrono types read. A `timestamp` is `YYYY-MM-DD HH:MM:SS.ffffff`, a `date` is `YYYY-MM-DD`, a `time` is `HH:MM:SS.ffffff`, and a `timetz` is its UTC time followed by `+00:00`. An `interval` is the ISO 8601 duration Postgres prints under `IntervalStyle = iso_8601`, such as `P1Y2M3DT4H5M6.789S`. Text sources read Postgres's output under the default `DateStyle = ISO` and `IntervalStyle = postgres`, and Maxwell's offset-less `TIMESTAMP` reads as UTC. Other text is a `DecodeError::WrongPayloadKind`, and `infinity`, `-infinity`, a year outside 1 to 9999, a `time` of `24:00:00` and an infinite `interval` are the new `DecodeError::UnrepresentableTemporal`, because no replica reader can hold them.

A `timetz` value carries a `+00:00` offset, so an upload path that parses the replica's `time` text as plain `HH:MM:SS[.f]` refuses it, and such a column replicates but does not upload until that parser accepts the offset. Interval text in an `IntervalStyle` other than `postgres`, such as `postgres_verbose`, `sql_standard` or `iso_8601`, is a `WrongPayloadKind` error rather than stored as received, so on a server configured that way every text-mode change event carrying an `interval` fails. `PgBinary` snapshots and binary-mode pgoutput read the binary form and do not depend on the setting.

### Added

`PgBinary` decodes `numeric`, `timestamp`, `timestamptz`, `date`, `time`, `timetz`, `interval`, `json` and `jsonb` through `TypeMap::defaults()`, and a binary-mode `PgWalstream` payload of those types decodes through the same code, so a snapshot and a live change of one row store byte-identical values. A `numeric` is the text Postgres prints for it, `NaN` and `Infinity` included, and a `jsonb` is its normalized text.

## 0.13.0

### Breaking

`SchemaWithPK::primary_key_columns` returns `impl Iterator<Item = usize>` instead of `Vec<usize>`, so a caller that indexed or measured the result now collects it first. It is the crate's single answer to which columns form the primary key and in what key order, and it selects the column holding key position 0, then 1, up to the key width, so it allocates nothing and costs `O(k*n)` for a key of width `k` over `n` columns. `SimpleTable::pk_indices` is removed, and `primary_key_columns().collect()` replaces it.

The parser refuses a table header whose nonzero primary-key flag bytes are not the dense key ordinals `1..=n`, with a new `ParseError::InvalidPrimaryKeyFlags` carrying the table name and the byte offset of the flags. Bytes that parsed before can now error. Every header SQLite writes is a dense sequence by construction, so this refuses only input SQLite would not produce, such as the flag bytes `[255, 64, 0]` and `[15, 0, 63, 215, 61, 58, 56, 56, 50]` that two fuzz inputs carry. `TableSchema::new` panics on the same input, because a schema whose flags are not ordinals claims a key that no consumer can read and every encoder path assumes the invariant.

### Added

`ChangesetUpdatePairExt::is_changed` reads what an `(old, new)` pair means on the changeset wire, and `ChangesetOp::changed_column_indices` applies it across an UPDATE. A changeset UPDATE carries the old image of the key columns and of the changed columns, and the new image of the changed columns only, so undefined on both sides is a column outside the diff, an old-only pair is the row identity that SQLite writes for every key column of every UPDATE, both sides present differ exactly when the values do, and a new value without an old one counts as changed. The diesel changeset renderer calls the same method rather than restating the rule, so the published interpretation cannot drift from the SQL the crate emits.

## 0.12.0

### Added

`DiffFormat<S, B>` is a public, nameable bound for a diff format, either changeset or patchset. It is a supertrait marker over the crate-private `Format` trait with a blanket impl, so only `ChangesetFormat` and `PatchsetFormat` satisfy it and the `Old`/`DeleteData` associated types stay private. `DiffSetBuilder::build` and `DiffSet::build` are now single generic methods rather than one per format, so downstream code can write one function generic over `F: DiffFormat<String, Vec<u8>>` that folds a batch of wire events and serializes through `new`, `digest` and `build`, instead of duplicating the body once per format. The two formats stay semantically distinct: a changeset carries full old-row data and is reversible, while a patchset carries only primary keys for deletes and only changed columns for updates and is forward-only. `DiffSetBuilder` and `Digestable` keep their existing `Format` bound, which the new supertrait implies, so no existing code changes.

## 0.11.0

### Breaking

`WireSchema::get` now takes the source schema as its first argument, `get(&self, source_schema: Option<&str>, table_name: &str)`, so a table lookup can disambiguate names that repeat across schemas or databases. Existing implementations must accept and honor the new parameter.

## 0.10.0

### Fixed

Patchset UPDATE records now write every column in column order. The encoder previously wrote all primary key columns first and the remaining columns after, a layout that agrees with SQLite only when the primary key is the first column, so a table whose key sits anywhere else produced records SQLite reads with the values landing on the wrong columns. A table header now writes its column count as a varint instead of a raw byte, which panicked outright at 256 columns and silently emitted a malformed header between 128 and 255. Both defects are now verified byte for byte against SQLite's own session extension in both directions, the encoder against SQLite's bytes and the parser against them, so a pair of cancelling errors cannot hide. Anything 0.9.0 wrote for an affected table was already malformed and cannot be read back.

The base64 decoder rejects malformed input it used to accept: an effective length one above a multiple of four, an all-padding payload, and a trailing character carrying bits that no decoded byte can hold (RFC 4648 section 3.5).

### Changed

The Maxwell and wal2json decoders now come from the extracted `maxwell-cdc` and `wal2json-events` crates, which moves four public shapes. `maxwell::Message` is `#[non_exhaustive]`, so an exhaustive match no longer compiles. `maxwell::RowChange::data` is a `serde_json::Map` rather than a `BTreeMap`. `wal2json::MessageV2` is an enum rather than a struct. `wal2json::Column` is `#[non_exhaustive]`, so a struct literal no longer compiles.

`parser::ParseError`, `errors::Error` and `builders::sql::ParseError` are now `#[non_exhaustive]`, matching their `DecodeError` and `ConversionError` siblings, so a match on any of them needs a wildcard arm.

`digest_sql` rejects three inputs it used to accept quietly: an INSERT whose column list is shorter than its VALUES list (the missing columns became NULL), a WHERE naming only part of a composite primary key (the result was an operation that could never match a row), and an OR in a WHERE (one branch was kept and the rest discarded). A failure anywhere in a multi-statement input now leaves the builder untouched rather than applying the statements that happened to parse first.

The minimum supported Rust version is 1.87, which is what the `pg-walstream` feature has required since 0.5.0. The manifest claimed 1.85.

`testing::compare_db_states` is removed. It had no caller and mis-parsed any table name containing a space.

## 0.9.0

### Added

A new `PgBinary` wire source decodes PostgreSQL binary result fields (the form diesel's Postgres backend hands back) straight into `Value` through the same `WireType` and `Decoder` machinery the CDC sources use, so a query result and a CDC event reach a byte-identical representation by construction. The payload `PgBinaryColumn` carries the caller's catalog `WireType` explicitly (binary `int4` and `float4` are both four opaque bytes), a `None` `raw` short-circuits to `Value::Null`, and `TypeMap::<PgBinary, _, _>::defaults()` covers the v1 set `bool`, `int`, `real`, `text`, `bytea`, and `uuid` (as a 16-byte blob). The remaining types stay `NotYetImplemented` until their numeric binary layouts render back to text byte-identically to the verbatim decoders. A cross-source parity test guards that `PgBinary` never diverges from `PgWalstream`.

### Changed

The three CDC wire sources now share one internal decoding core instead of carrying near-identical copies. `wal2json::ConversionError`, `maxwell::ConversionError`, and `pg_walstream::ConversionError` are now a single re-exported `#[non_exhaustive]` type with the union of the previous variants, so matching on it must account for the shared set. The per-source `build_insert`/`build_patchset_update`/`build_changeset_delete`/`build_patch_delete` helpers, the float and PostgreSQL-binary scalar decoders, and the JSON-source (`Maxwell`, `Wal2Json`) `text`/`decimal`/`uuid`/`json` decoders are now shared, and the duplicated `pk_indices` computation in the SQL and diesel renderers now calls the existing `SchemaWithPK::primary_key_columns`. No wire-format or decoded-value behavior changes.

## 0.8.0

### Added

`ParsedDiffSet::rename_tables` renames table sections in place. A callback maps each section name to a new one (or `None` to keep it), and the count of renamed sections is returned. Only the name changes, so a diffset captured against one schema can apply where the same tables carry different physical names (for example an RLS translation that renames the storage table), which `sqlite3changeset_apply` cannot remap on its own.

## 0.6.1

### Fixed

Changeset UPDATE digests now always capture the old primary key, so a changeset produced from a wire event applies through the diesel changeset path even when the update does not change the key. The wal2json digest previously dropped the old-row image entirely (it now reads `identity` in the v2 format and `oldkeys` in the v1 format). The Maxwell digest treats a column absent from `old` as unchanged, taking its old value from the new value, since Maxwell lists only changed columns in `old`. The pgoutput digest keeps a primary-key column's old value from the new tuple when `old_data` is absent under `REPLICA IDENTITY DEFAULT`. The patchset builders are unchanged, since they use only the new image.

## 0.6.0

### Added

The `diesel` feature now renders `ChangeSet` operations as backend-generic Diesel queries, alongside the existing `PatchSet` support. `ChangesetOp` implements `QueryFragment`, `QueryId`, and `RunQueryDsl`, and `ChangesetOp::with_adapter` builds a `BoundChangesetOp` just as `PatchsetOp::with_adapter` builds a `BoundPatchsetOp`. Both bound types are aliases of the new generic `BoundOp`, and `ApplyOps` drives either one. Because a changeset carries the old and new value of every column, it renders primary-key changes (including composite keys) as `UPDATE ... SET <changed columns> WHERE <old key>`, which a patchset cannot represent. A changeset `UPDATE` writes only the columns whose value actually changed, so applying it never rewrites (or spuriously triggers on) a column that did not move.

## 0.5.1

### Fixed

The wal2json `bytea` decoder (`PgByteaTextModeDecoder`) now accepts wal2json's bare lowercase hex form (for example `0001deadff`) in addition to the Postgres-style `\x`-prefixed hex form, so `bytea` columns round-trip on the wal2json vehicle. The pgoutput and Maxwell paths are unchanged.

## 0.5.0

The optional `pg_walstream` dependency moves from 0.7 to 0.8.

### Breaking

The `pg_walstream` and `pg_walstream_reverse` module re-exports (`EventType`, `RowData`, `ColumnValue`, `ChangeEvent`, `ReplicaIdentity`, `ColumnData`, `ColumnInfo`, `LogicalReplicationMessage`, `TupleData`, `Oid`, and the rest) now resolve to `pg_walstream` 0.8 types. Consumers of the `pg-walstream` feature that also depend on `pg_walstream` directly must move to 0.8. Enabling the feature raises the minimum supported Rust version to 1.87, which `pg_walstream` 0.8 requires.

## 0.4.0

Source-independent semantic type key for `digest`. A catalog carrying semantic column types now drives `DiffSetBuilder::digest` for every wire source without translating to a source-native key.

### Added

`WireType` enum (`Bool`, `Int`, `Real`, `Text`, `Bytes`, `Uuid`, `Decimal`, `Timestamp`, `TimestampTz`, `Date`, `Time`, `Interval`, `Json`, `Jsonb`), re-exported from the crate root. It is the single decoder-dispatch key shared by `pg_walstream`, `wal2json`, and `maxwell`.

`TypeMap::defaults()` now registers a `WireType::Uuid` decoder for every source (`UuidText36Decoder`), which fixes the previously missing wal2json `uuid` mapping.

### Breaking

`WireSource` drops the associated `TypeKey` and its `type_key` method in favor of `fn wire_type(payload) -> WireType`.

`WireColumnTypes` and `WireSchema` are no longer generic over the source. `WireColumnTypes::column_type_key(idx) -> Src::TypeKey` becomes `WireColumnTypes::column_type(idx) -> WireType`, and `WireSchema<Src>` becomes `WireSchema`. `Digestable` and `DiffSetBuilder::digest` drop `Src` from their schema and column-type bounds.

`TypeMap` is keyed by `WireType` instead of `Src::TypeKey`, so `register` and `with` take a `WireType`.

The per-column payload structs replace their native type field with `wire_type: WireType`: `PgWalstreamColumn` drops `oid` and `type_modifier`, `Wal2JsonColumn` drops `pg_type_name`, and `MaxwellColumn` drops `mysql_type`. Binary integer and float widths on `pg_walstream` are now inferred from the payload byte length. The wal2json paren-stripping and maxwell `tinyint(1)` type-name normalizations are gone because the schema declares the semantic type directly.

Downstreams migrate by implementing the source-independent `WireColumnTypes` and `WireSchema` and registering decoders under `WireType`.

## 0.3.0

`wal2json::MessageV2` now carries the optional wal2json LSN.

### Added

`lsn: Option<String>` field on `wal2json::MessageV2`, populated from the message's `lsn` when wal2json runs with `include-lsn=true` and `None` otherwise. The value stays a raw `hi/lo` hex string (for example `0/16B2270`) so the module carries no Postgres-specific numeric LSN type, and the consumer decides how to parse it. The `Digestable` wire output is byte-identical whether or not the field is present.

### Breaking

Struct-literal construction of `MessageV2` must now supply `lsn` (for example `lsn: None`). Deserialization from wal2json JSON is unaffected because the field is `#[serde(default)]`.

## 0.2.0

Schema-aware forward conversion for CDC wire formats. Downstream users register a type-to-decoder mapping once and consume `pg_walstream`, `wal2json`, or `maxwell` interchangeably.

### Removed

`debezium` module and feature. Users on Postgres have `pg_walstream` and `wal2json`. Users on MySQL have `maxwell`.

The 0.1.x `TryFrom<..>` impls on `Insert`, `Update`, `ChangeDelete`, and `PatchDelete` for pg_walstream, wal2json, and maxwell events. Use `builder.digest(&event, &schema, &adapter)` instead.

`SnifferDecoder` and `SnifferAdapter` (migration bridge for the deleted `TryFrom` impls).

Legacy `digest_pg_walstream`, `digest_wal2json_v2`, `digest_wal2json_v1_change`, and `digest_maxwell` methods on `DiffSetBuilder`.

Per-source `maxwell`, `pg_walstream`, and `wal2json` Criterion benches. They benched the deleted `TryFrom` path.

### Added

`sqlite_diff_rs::wire` module with:

- `WireSource` sealed trait implemented by `PgWalstream`, `Wal2Json`, `Maxwell`. Each carries an associated `Payload` struct and `TypeKey`.
- `Decoder<Src, S, B>` trait. Zero-sized unit types for the built-ins, state-carrying structs for custom user decoders.
- `WireAdapter<Src, S, B>` trait. Single-method dispatcher. `TypeMap` is the primary implementer.
- `TypeMap<Src, S, B>` generic hashmap-backed registry keyed by `Src::TypeKey`.
- `TypeMapDefaults<S, B>` companion trait giving `TypeMap::<Src>::defaults()` a pre-populated registry.
- `DecodeError` shared enum wrapped by each format's existing `ConversionError` via a new `Decode(_)` variant.

Built-in decoders shipping in 0.2.0:

- `BoolDecoder`
- `IntDecoder`, `Int64OverflowToTextDecoder`
- `RealDecoder` (NaN normalizes to `Null`, `-0.0` to `0.0`)
- `TextDecoder` (strict UTF-8)
- `PgByteaBinaryDecoder`, `PgByteaTextModeDecoder`, `MySqlBinaryDecoder` (vendored base64 and PG `\xHEX` decoders, no external deps)
- `UuidBlob16Decoder`, `UuidText36Decoder` (vendored parser, no `uuid` dep)
- `DecimalTextDecoder`
- `TimestampVerbatimDecoder`, `TimestampTzVerbatimDecoder`, `DateVerbatimDecoder`, `TimeVerbatimDecoder`, `IntervalVerbatimDecoder`
- `JsonVerbatimDecoder`, `JsonCanonicalDecoder` (canonical does recursive key sort)
- `NullDecoder`

New traits `WireColumnTypes<Src>` (schema-side per-column type key), `WireSchema<Src>` (table-name lookup), and `Digestable<F, T, S, B>` (event dispatch, implemented in-crate for `pg_walstream::EventType`, `wal2json::MessageV2`, `wal2json::ChangeV1`, `maxwell::Message` times both formats).

One unified digest entry point: `builder.digest(&event, &schema, &adapter)`. Replaces the 0.1.x `digest_pg_walstream` / `digest_wal2json_v2` / `digest_wal2json_v1_change` / `digest_maxwell` methods. `RelationInfo` is no longer a digest argument (OIDs come from the schema).

`ConversionError::TableNotFound(String)` added to each format's error type.

`IndexableValues` promoted from `pub(crate)` to `pub` so external code can implement `SchemaWithPK`.


### MSRV

Rust 1.85 (edition 2024). Unchanged from 0.1.x.

## 0.1.4 and earlier

See git history.
