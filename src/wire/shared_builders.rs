//! Source-independent builders that fold a CDC event's columns into a
//! diff operation.
//!
//! The three forward sources (`pg_walstream`, `wal2json`, `maxwell`)
//! previously each carried a near-identical copy of these `build_*`
//! helpers. They differ only in how a column's name and value are read
//! and how the per-source payload is constructed, which is exactly what
//! [`WireColumnItem`] abstracts. The four helpers here are byte-identical
//! across sources; the changeset UPDATE helper stays per-source because
//! its old-value fallback is genuinely source-specific (for example
//! `pg_walstream`'s replica-identity handling).

use alloc::string::String;
use alloc::vec::Vec;

use super::adapter::WireAdapter;
use super::conversion_error::ConversionError;
use super::source::{WireColumnTypes, WireSchema, WireSource};
use super::wire_type::WireType;
use crate::builders::{ChangeDelete, Insert, PatchDelete, PatchsetFormat, Update};
use crate::encoding::Value;
use crate::schema::NamedColumns;

/// Resolve a table name against `schema`, or return
/// [`ConversionError::TableNotFound`].
///
/// # Errors
///
/// [`ConversionError::TableNotFound`] when the schema has no such table.
pub(crate) fn resolve_table<'a, Sch>(
    schema: &'a Sch,
    source_schema: Option<&str>,
    table_name: &str,
) -> Result<&'a Sch::Table, ConversionError>
where
    Sch: WireSchema,
{
    schema
        .get(source_schema, table_name)
        .ok_or_else(|| ConversionError::TableNotFound(String::from(table_name)))
}
/// One column of a CDC event, abstracting the per-source payload shape.
///
/// A source yields an iterator of these; the shared `build_*` helpers
/// resolve the column index, look up its semantic type, and construct the
/// source-specific [`WireSource::Payload`] via [`payload`](Self::payload)
/// for the adapter to decode.
pub(crate) trait WireColumnItem<Src: WireSource> {
    /// The event's column name.
    fn name(&self) -> &str;

    /// Build the per-source decoder payload for this column.
    fn payload(&self, wire_type: WireType) -> Src::Payload<'_>;
}

/// Resolve a column, decode it, and return its `(column_index, value)`.
fn decode_item<Src, T, S, B, A, C>(
    item: &C,
    table: &T,
    adapter: &A,
) -> Result<(usize, Value<S, B>), ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    A: WireAdapter<Src, S, B>,
    C: WireColumnItem<Src>,
{
    let name = item.name();
    let col_idx = table
        .column_index(name)
        .ok_or_else(|| ConversionError::ColumnNotFound(String::from(name)))?;
    let wire_type = table.column_type(col_idx);
    let value = adapter.decode(item.payload(wire_type))?;
    Ok((col_idx, value))
}

/// Decode every column of `items` and set it on `row` with `set`.
fn set_items<Src, T, S, B, A, I, C, R, E>(
    items: I,
    table: &T,
    adapter: &A,
    row: R,
    set: impl Fn(R, usize, Value<S, B>) -> Result<R, E>,
) -> Result<R, ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    A: WireAdapter<Src, S, B>,
    I: IntoIterator<Item = C>,
    C: WireColumnItem<Src>,
{
    items.into_iter().try_fold(row, |row, item| {
        let (col_idx, value) = decode_item(&item, table, adapter)?;
        set(row, col_idx, value)
            .map_err(|_| ConversionError::ColumnNotFound(String::from(item.name())))
    })
}

/// Build an INSERT from every column in the event.
pub(crate) fn build_insert<Src, T, S, B, A, I, C>(
    items: I,
    table: &T,
    adapter: &A,
) -> Result<Insert<T, S, B>, ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Src, S, B>,
    I: IntoIterator<Item = C>,
    C: WireColumnItem<Src>,
{
    set_items(
        items,
        table,
        adapter,
        Insert::from(table.clone()),
        Insert::set,
    )
}

/// Build a patchset UPDATE from the event's new-image columns.
pub(crate) fn build_patchset_update<Src, T, S, B, A, I, C>(
    items: I,
    table: &T,
    adapter: &A,
) -> Result<Update<T, PatchsetFormat, S, B>, ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Src, S, B>,
    I: IntoIterator<Item = C>,
    C: WireColumnItem<Src>,
{
    let update: Update<T, PatchsetFormat, S, B> = Update::from(table.clone());
    set_items(items, table, adapter, update, |row, col_idx, value| {
        row.set(col_idx, value)
    })
}

/// Build a changeset DELETE from the event's old-image (identity) columns.
pub(crate) fn build_changeset_delete<Src, T, S, B, A, I, C>(
    items: I,
    table: &T,
    adapter: &A,
) -> Result<ChangeDelete<T, S, B>, ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    S: Clone + Default + AsRef<str>,
    B: Clone + Default + AsRef<[u8]>,
    A: WireAdapter<Src, S, B>,
    I: IntoIterator<Item = C>,
    C: WireColumnItem<Src>,
{
    set_items(
        items,
        table,
        adapter,
        ChangeDelete::from(table.clone()),
        ChangeDelete::set,
    )
}

/// Build a patchset DELETE, extracting only the primary-key columns.
///
/// Returns [`ConversionError::MissingColumns`] if any primary-key column
/// is absent from the event.
pub(crate) fn build_patch_delete<Src, T, S, B, A, I, C>(
    items: I,
    table: &T,
    adapter: &A,
) -> Result<PatchDelete<T, S, B>, ConversionError>
where
    Src: WireSource,
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Src, S, B>,
    I: IntoIterator<Item = C>,
    C: WireColumnItem<Src>,
{
    let num_pks = table.number_of_primary_keys();
    let mut pk_slots: Vec<Option<Value<S, B>>> = alloc::vec![None; num_pks];

    for item in items {
        let name = item.name();
        let col_idx = table
            .column_index(name)
            .ok_or_else(|| ConversionError::ColumnNotFound(String::from(name)))?;
        if let Some(pk_idx) = table.primary_key_index(col_idx) {
            let wire_type = table.column_type(col_idx);
            pk_slots[pk_idx] = Some(adapter.decode(item.payload(wire_type))?);
        }
    }

    let pk = pk_slots
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(ConversionError::MissingColumns)?;
    Ok(PatchDelete::new(table.clone(), pk))
}
