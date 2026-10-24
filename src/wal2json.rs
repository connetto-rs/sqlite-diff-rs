//! wal2json message parsing and conversion to `SQLite` changeset operations.
//!
//! [wal2json](https://github.com/eulerto/wal2json) is a `PostgreSQL` logical
//! replication output plugin. This module deserializes its messages and
//! converts them into changeset operations compatible with this crate's
//! builders.
//!
//! Two formats are supported: v1 emits one transaction-level JSON object with
//! a full `change` array, and v2 emits one JSON object per row tuple.
//!
//! Wal2json does not carry trigger-origin metadata, so converted ops default
//! to `indirect = false`. Override via the [`Indirect`](crate::Indirect) trait
//! if you know out-of-band that the event was trigger-induced.
//!
//! # Example
//!
//! ```
//! use sqlite_diff_rs::wal2json::{MessageV2, parse_v2};
//!
//! let json = r#"{"action":"I","schema":"public","table":"users","columns":[{"name":"id","type":"integer","value":1},{"name":"name","type":"text","value":"Alice"}]}"#;
//!
//! let MessageV2::Insert(row) = parse_v2(json).unwrap() else {
//!     panic!("expected an insert");
//! };
//! assert_eq!(row.table, "users");
//! assert_eq!(row.columns.unwrap().len(), 2);
//! ```

pub use wal2json_events::{
    Action, ChangeV1, Column, ColumnArrays, LogicalMessageV2, MessageV2, OldKeys, RowV2,
    TransactionBoundary, TransactionV1, TruncateV2, parse_v1, parse_v2,
};

pub use crate::wire::ConversionError;

use crate::wire::{
    Sealed, WireColumnItem, WireSource, WireType, build_changeset_delete, build_insert,
    build_patch_delete, build_patchset_update, resolve_table,
};

/// One wal2json column adapted to the shared [`WireColumnItem`] contract.
struct Wal2JsonItem<'a> {
    name: &'a str,
    value: &'a serde_json::Value,
}

impl WireColumnItem<Wal2Json> for Wal2JsonItem<'_> {
    fn name(&self) -> &str {
        self.name
    }

    fn payload(&self, wire_type: WireType) -> Wal2JsonColumn<'_> {
        Wal2JsonColumn {
            column_name: self.name,
            wire_type,
            value: self.value,
        }
    }
}

/// Marker type for the `wal2json` source.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wal2Json;

impl Sealed for Wal2Json {}

impl WireSource for Wal2Json {
    type Payload<'a> = Wal2JsonColumn<'a>;

    fn wire_type(payload: &Self::Payload<'_>) -> WireType {
        payload.wire_type
    }

    fn column_name<'a>(payload: &'a Self::Payload<'_>) -> &'a str {
        payload.column_name
    }
}

/// Per-column payload for the `wal2json` source.
///
/// v2 populates from [`Column`] fields directly. v1 populates from the
/// parallel `columnnames`/`columntypes`/`columnvalues` arrays on
/// [`ChangeV1`].
#[derive(Debug, Clone, Copy)]
pub struct Wal2JsonColumn<'a> {
    /// Column name.
    pub column_name: &'a str,
    /// Semantic column type driving decoder dispatch.
    pub wire_type: WireType,
    /// Column value as a JSON value.
    pub value: &'a serde_json::Value,
}

impl Wal2JsonColumn<'_> {
    /// Ergonomic helper for calling a specific [`Decoder`](crate::wire::Decoder) on this
    /// payload without fully-qualified syntax. Fixes the `Src` generic
    /// to [`Wal2Json`] so the compiler can pick the impl.
    ///
    /// # Errors
    ///
    /// Propagates the decoder's [`DecodeError`](crate::wire::DecodeError).
    pub fn decoded_by<D, S, B>(self, decoder: &D) -> Result<Value<S, B>, crate::wire::DecodeError>
    where
        D: crate::wire::Decoder<Wal2Json, S, B>,
    {
        decoder.decode(self)
    }
}

use crate::builders::{
    ChangeDelete, ChangesetFormat, DeleteArg, DiffOps, DiffSetBuilder, Format, Insert, PatchDelete,
    PatchsetFormat, Update,
};
use crate::encoding::Value;
use crate::schema::NamedColumns;
use crate::wire::{Digestable, WireAdapter, WireColumnTypes, WireSchema};
use core::fmt::Debug;
use core::hash::Hash;

impl<T, S, B> Digestable<ChangesetFormat, T, S, B> for MessageV2
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Wal2Json;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<ChangesetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<ChangesetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Wal2Json, S, B>,
    {
        digest_v2(
            self,
            builder,
            schema,
            adapter,
            build_changeset_update_from_v2,
            build_changeset_delete_from_columns,
        )
    }
}

impl<T, S, B> Digestable<PatchsetFormat, T, S, B> for MessageV2
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Wal2Json;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<PatchsetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<PatchsetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Wal2Json, S, B>,
    {
        digest_v2(
            self,
            builder,
            schema,
            adapter,
            |columns, _identity, table, adapter| {
                build_patchset_update_from_v2(columns, table, adapter)
            },
            build_patch_delete_from_columns,
        )
    }
}

impl<T, S, B> Digestable<ChangesetFormat, T, S, B> for ChangeV1
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Wal2Json;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<ChangesetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<ChangesetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Wal2Json, S, B>,
    {
        digest_v1(
            self,
            builder,
            schema,
            adapter,
            build_changeset_update_from_v1,
            build_changeset_delete_from_v1,
        )
    }
}

impl<T, S, B> Digestable<PatchsetFormat, T, S, B> for ChangeV1
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Wal2Json;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<PatchsetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<PatchsetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Wal2Json, S, B>,
    {
        digest_v1(
            self,
            builder,
            schema,
            adapter,
            |columns, _oldkeys, table, adapter| {
                build_patchset_update_from_v1(columns, table, adapter)
            },
            build_patch_delete_from_v1,
        )
    }
}

/// Digests one wal2json v2 message into `builder`, building its UPDATE and DELETE rows
/// with the format's `update` and `delete`.
fn digest_v2<F, T, S, B, Sch, A>(
    message: &MessageV2,
    builder: DiffSetBuilder<F, T, S, B>,
    schema: &Sch,
    adapter: &A,
    update: impl FnOnce(
        &[Column],
        Option<&[Column]>,
        &T,
        &A,
    ) -> Result<Update<T, F, S, B>, ConversionError>,
    delete: impl FnOnce(&[Column], &T, &A) -> Result<DeleteArg<F, T, S, B>, ConversionError>,
) -> Result<DiffSetBuilder<F, T, S, B>, ConversionError>
where
    F: Format<S, B>,
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
    Sch: WireSchema<Table = T>,
    A: WireAdapter<Wal2Json, S, B>,
    DiffSetBuilder<F, T, S, B>: DiffOps<T, S, B, Format = F>,
{
    match message {
        MessageV2::Insert(row) => {
            let table = resolve_table(schema, row.schema.as_deref(), row.table.as_str())?;
            let columns = row
                .columns
                .as_deref()
                .ok_or(ConversionError::MissingColumns)?;
            let insert = build_insert_from_v2(columns, table, adapter)?;
            Ok(DiffOps::insert(builder, insert))
        }
        MessageV2::Update(row) => {
            let table = resolve_table(schema, row.schema.as_deref(), row.table.as_str())?;
            let columns = row
                .columns
                .as_deref()
                .ok_or(ConversionError::MissingColumns)?;
            let update = update(columns, row.identity.as_deref(), table, adapter)?;
            Ok(DiffOps::update(builder, update))
        }
        MessageV2::Delete(row) => {
            let table = resolve_table(schema, row.schema.as_deref(), row.table.as_str())?;
            let identity = row
                .identity
                .as_deref()
                .ok_or(ConversionError::MissingColumns)?;
            let delete = delete(identity, table, adapter)?;
            Ok(DiffOps::delete(builder, delete))
        }
        MessageV2::Begin(_)
        | MessageV2::Commit(_)
        | MessageV2::Truncate(_)
        | MessageV2::Message(_) => Ok(builder),
    }
}

/// Digests one wal2json v1 change into `builder`, building its UPDATE and DELETE rows
/// with the format's `update` and `delete`.
fn digest_v1<F, T, S, B, Sch, A>(
    change: &ChangeV1,
    builder: DiffSetBuilder<F, T, S, B>,
    schema: &Sch,
    adapter: &A,
    update: impl FnOnce(&ColumnArrays, &OldKeys, &T, &A) -> Result<Update<T, F, S, B>, ConversionError>,
    delete: impl FnOnce(&OldKeys, &T, &A) -> Result<DeleteArg<F, T, S, B>, ConversionError>,
) -> Result<DiffSetBuilder<F, T, S, B>, ConversionError>
where
    F: Format<S, B>,
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
    Sch: WireSchema<Table = T>,
    A: WireAdapter<Wal2Json, S, B>,
    DiffSetBuilder<F, T, S, B>: DiffOps<T, S, B, Format = F>,
{
    match change {
        ChangeV1::Insert {
            schema: source_schema,
            table,
            columns,
            ..
        } => {
            let table = resolve_table(schema, source_schema.as_deref(), table.as_str())?;
            let insert = build_insert_from_v1(columns, table, adapter)?;
            Ok(DiffOps::insert(builder, insert))
        }
        ChangeV1::Update {
            schema: source_schema,
            table,
            columns,
            oldkeys,
            ..
        } => {
            let table = resolve_table(schema, source_schema.as_deref(), table.as_str())?;
            let update = update(columns, oldkeys, table, adapter)?;
            Ok(DiffOps::update(builder, update))
        }
        ChangeV1::Delete {
            schema: source_schema,
            table,
            oldkeys,
            ..
        } => {
            let table = resolve_table(schema, source_schema.as_deref(), table.as_str())?;
            let delete = delete(oldkeys, table, adapter)?;
            Ok(DiffOps::delete(builder, delete))
        }
        ChangeV1::Message { .. } => Ok(builder),
    }
}

// -- v2 helpers ---------------------------------------------------------------

const JSON_NULL: serde_json::Value = serde_json::Value::Null;

/// wal2json omits a column's `value` only in the `pk` list, which is never digested here, so a
/// column in `columns` or `identity` without one is malformed input rather than a SQL NULL.
fn require_values(columns: &[Column]) -> Result<(), ConversionError> {
    if columns.iter().any(|column| column.value.is_none()) {
        return Err(ConversionError::MissingData("value", "row"));
    }
    Ok(())
}

/// The value of a column that [`require_values`] has already accepted.
fn column_value(column: &Column) -> &serde_json::Value {
    column.value.as_ref().unwrap_or(&JSON_NULL)
}

fn build_insert_from_v2<T, S, B, A>(
    columns: &[Column],
    table: &T,
    adapter: &A,
) -> Result<Insert<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    require_values(columns)?;
    build_insert(
        columns.iter().map(|c| Wal2JsonItem {
            name: c.name.as_str(),
            value: column_value(c),
        }),
        table,
        adapter,
    )
}

fn build_changeset_update_from_v2<T, S, B, A>(
    columns: &[Column],
    identity: Option<&[Column]>,
    table: &T,
    adapter: &A,
) -> Result<Update<T, ChangesetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + AsRef<str>,
    B: Clone + Debug + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    require_values(columns)?;
    if let Some(identity) = identity {
        require_values(identity)?;
    }
    let mut update: Update<T, ChangesetFormat, S, B> = Update::from(table.clone());
    for col in columns {
        let col_idx = table
            .column_index(&col.name)
            .ok_or_else(|| ConversionError::ColumnNotFound(col.name.clone()))?;
        let wire_type = table.column_type(col_idx);
        let new = adapter.decode(Wal2JsonColumn {
            column_name: col.name.as_str(),
            wire_type,
            value: column_value(col),
        })?;

        // Pair the new value with its old-row value from the identity image
        // when present, so a primary-key change keeps the old key for the
        // WHERE clause. Non-key columns absent from the identity fall back to
        // set_new.
        if let Some(old_col) = identity.and_then(|id| id.iter().find(|c| c.name == col.name)) {
            let old = adapter.decode(Wal2JsonColumn {
                column_name: col.name.as_str(),
                wire_type,
                value: column_value(old_col),
            })?;
            update = update
                .set(col_idx, old, new)
                .map_err(|_| ConversionError::ColumnNotFound(col.name.clone()))?;
        } else {
            update = update
                .set_new(col_idx, new)
                .map_err(|_| ConversionError::ColumnNotFound(col.name.clone()))?;
        }
    }
    Ok(update)
}

fn build_patchset_update_from_v2<T, S, B, A>(
    columns: &[Column],
    table: &T,
    adapter: &A,
) -> Result<Update<T, PatchsetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    require_values(columns)?;
    build_patchset_update(
        columns.iter().map(|c| Wal2JsonItem {
            name: c.name.as_str(),
            value: column_value(c),
        }),
        table,
        adapter,
    )
}

fn build_changeset_delete_from_columns<T, S, B, A>(
    identity: &[Column],
    table: &T,
    adapter: &A,
) -> Result<ChangeDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Default + AsRef<str>,
    B: Clone + Default + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    require_values(identity)?;
    build_changeset_delete(
        identity.iter().map(|c| Wal2JsonItem {
            name: c.name.as_str(),
            value: column_value(c),
        }),
        table,
        adapter,
    )
}

fn build_patch_delete_from_columns<T, S, B, A>(
    identity: &[Column],
    table: &T,
    adapter: &A,
) -> Result<PatchDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    require_values(identity)?;
    build_patch_delete(
        identity.iter().map(|c| Wal2JsonItem {
            name: c.name.as_str(),
            value: column_value(c),
        }),
        table,
        adapter,
    )
}

// -- v1 helpers ---------------------------------------------------------------

fn iter_v1_columns(
    columns: &ColumnArrays,
) -> impl Iterator<Item = (&str, &serde_json::Value)> + '_ {
    columns
        .columnnames
        .iter()
        .zip(columns.columnvalues.iter())
        .map(|(n, v)| (n.as_str(), v))
}

fn iter_v1_oldkeys(oldkeys: &OldKeys) -> impl Iterator<Item = (&str, &serde_json::Value)> + '_ {
    oldkeys
        .keynames
        .iter()
        .zip(oldkeys.keyvalues.iter())
        .map(|(n, v)| (n.as_str(), v))
}

fn build_insert_from_v1<T, S, B, A>(
    columns: &ColumnArrays,
    table: &T,
    adapter: &A,
) -> Result<Insert<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    build_insert(
        iter_v1_columns(columns).map(|(name, value)| Wal2JsonItem { name, value }),
        table,
        adapter,
    )
}

fn build_changeset_update_from_v1<T, S, B, A>(
    columns: &ColumnArrays,
    oldkeys: &OldKeys,
    table: &T,
    adapter: &A,
) -> Result<Update<T, ChangesetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + AsRef<str>,
    B: Clone + Debug + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    let mut update: Update<T, ChangesetFormat, S, B> = Update::from(table.clone());
    for (name, value) in iter_v1_columns(columns) {
        let col_idx = table
            .column_index(name)
            .ok_or_else(|| ConversionError::ColumnNotFound(name.into()))?;
        let wire_type = table.column_type(col_idx);
        let new = adapter.decode(Wal2JsonColumn {
            column_name: name,
            wire_type,
            value,
        })?;

        // Pair with the old value when the column appears in oldkeys (always at least the primary
        // key), else fall back to set_new.
        if let Some((_, old_value)) = iter_v1_oldkeys(oldkeys).find(|(n, _)| *n == name) {
            let old = adapter.decode(Wal2JsonColumn {
                column_name: name,
                wire_type,
                value: old_value,
            })?;
            update = update
                .set(col_idx, old, new)
                .map_err(|_| ConversionError::ColumnNotFound(name.into()))?;
        } else {
            update = update
                .set_new(col_idx, new)
                .map_err(|_| ConversionError::ColumnNotFound(name.into()))?;
        }
    }
    Ok(update)
}

fn build_patchset_update_from_v1<T, S, B, A>(
    columns: &ColumnArrays,
    table: &T,
    adapter: &A,
) -> Result<Update<T, PatchsetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    build_patchset_update(
        iter_v1_columns(columns).map(|(name, value)| Wal2JsonItem { name, value }),
        table,
        adapter,
    )
}

fn build_changeset_delete_from_v1<T, S, B, A>(
    oldkeys: &OldKeys,
    table: &T,
    adapter: &A,
) -> Result<ChangeDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Default + AsRef<str>,
    B: Clone + Default + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    build_changeset_delete(
        iter_v1_oldkeys(oldkeys).map(|(name, value)| Wal2JsonItem { name, value }),
        table,
        adapter,
    )
}

fn build_patch_delete_from_v1<T, S, B, A>(
    oldkeys: &OldKeys,
    table: &T,
    adapter: &A,
) -> Result<PatchDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Wal2Json, S, B>,
{
    build_patch_delete(
        iter_v1_oldkeys(oldkeys).map(|(name, value)| Wal2JsonItem { name, value }),
        table,
        adapter,
    )
}
