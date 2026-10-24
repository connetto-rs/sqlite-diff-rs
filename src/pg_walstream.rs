//! `pg_walstream` message conversion to `SQLite` changeset operations.
//!
//! [pg_walstream](https://github.com/isdaniel/pg-walstream) parses PostgreSQL
//! logical replication into `EventType` values. This module implements
//! [`Digestable`] on those events so callers fold them
//! into a builder via `DiffSetBuilder::digest(&event, &schema, &adapter)`.

// Re-export key types from pg_walstream for convenience
pub use pg_walstream::Oid;
pub use pg_walstream::{ChangeEvent, ColumnValue, EventType, Lsn, ReplicaIdentity, RowData};

use crate::ChangesetFormat;
use crate::builders::{
    ChangeDelete, DeleteArg, DiffOps, DiffSetBuilder, Format, Insert, PatchDelete, PatchsetFormat,
    Update,
};
use crate::schema::NamedColumns;
use crate::wire::{
    Sealed, WireAdapter, WireColumnItem, WireSource, WireType, build_changeset_delete,
    build_insert, build_patch_delete, build_patchset_update, resolve_table,
};
use core::fmt::Debug;
use core::hash::Hash;

pub use crate::wire::ConversionError;

/// Marker type for the `pg_walstream` source. Passed as the `Src`
/// generic parameter to `TypeMap`, `WireAdapter`, and `Decoder`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgWalstream;

impl Sealed for PgWalstream {}

impl WireSource for PgWalstream {
    type Payload<'a> = PgWalstreamColumn<'a>;

    fn wire_type(payload: &Self::Payload<'_>) -> WireType {
        payload.wire_type
    }

    fn column_name<'a>(payload: &'a Self::Payload<'_>) -> &'a str {
        payload.column_name
    }
}

/// Per-column payload for the `pg_walstream` source.
///
/// The format wrapper populates this once per column before invoking
/// [`WireAdapter::decode`].
#[derive(Debug, Clone, Copy)]
pub struct PgWalstreamColumn<'a> {
    /// Column name resolved from the relation cache.
    pub column_name: &'a str,
    /// Semantic column type driving decoder dispatch.
    pub wire_type: WireType,
    /// Raw wire payload.
    pub data: &'a ColumnValue,
}

impl PgWalstreamColumn<'_> {
    /// Ergonomic helper for calling a specific [`Decoder`](crate::wire::Decoder) on this
    /// payload without fully-qualified syntax. Fixes the `Src` generic
    /// to [`PgWalstream`] so the compiler can pick the impl.
    ///
    /// # Errors
    ///
    /// Propagates the decoder's [`DecodeError`](crate::wire::DecodeError).
    pub fn decoded_by<D, S, B>(
        self,
        decoder: &D,
    ) -> Result<crate::encoding::Value<S, B>, crate::wire::DecodeError>
    where
        D: crate::wire::Decoder<PgWalstream, S, B>,
    {
        decoder.decode(self)
    }
}

/// One `pg_walstream` column adapted to the shared [`WireColumnItem`] contract.
struct PgItem<'a> {
    name: &'a str,
    data: &'a ColumnValue,
}

impl WireColumnItem<PgWalstream> for PgItem<'_> {
    fn name(&self) -> &str {
        self.name
    }

    fn payload(&self, wire_type: WireType) -> PgWalstreamColumn<'_> {
        PgWalstreamColumn {
            column_name: self.name,
            wire_type,
            data: self.data,
        }
    }
}

// Schema-aware digest impls.

use crate::wire::{Digestable, WireColumnTypes, WireSchema};

impl<T, S, B> Digestable<ChangesetFormat, T, S, B> for EventType
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = PgWalstream;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<ChangesetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<ChangesetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<PgWalstream, S, B>,
    {
        digest(
            self,
            builder,
            schema,
            adapter,
            build_changeset_update_from_pg,
            build_changeset_delete_from_pg,
        )
    }
}

impl<T, S, B> Digestable<PatchsetFormat, T, S, B> for EventType
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = PgWalstream;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<PatchsetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<PatchsetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<PgWalstream, S, B>,
    {
        digest(
            self,
            builder,
            schema,
            adapter,
            |_old, new_data, table, adapter| {
                build_patchset_update_from_pg(new_data, table, adapter)
            },
            build_patch_delete_from_pg,
        )
    }
}

/// Digests one `pg_walstream` event into `builder`, building its UPDATE and DELETE rows
/// with the format's `update` and `delete`.
fn digest<F, T, S, B, Sch, A>(
    event: &EventType,
    builder: DiffSetBuilder<F, T, S, B>,
    schema: &Sch,
    adapter: &A,
    update: impl FnOnce(
        Option<&RowData>,
        &RowData,
        &T,
        &A,
    ) -> Result<Update<T, F, S, B>, ConversionError>,
    delete: impl FnOnce(&RowData, &T, &A) -> Result<DeleteArg<F, T, S, B>, ConversionError>,
) -> Result<DiffSetBuilder<F, T, S, B>, ConversionError>
where
    F: Format<S, B>,
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
    Sch: WireSchema<Table = T>,
    A: WireAdapter<PgWalstream, S, B>,
    DiffSetBuilder<F, T, S, B>: DiffOps<T, S, B, Format = F>,
{
    match event {
        EventType::Insert {
            schema: source_schema,
            table: name,
            data,
            ..
        } => {
            let table = resolve_table(schema, Some(source_schema.as_ref()), name.as_ref())?;
            let insert = build_insert_from_pg(data, table, adapter)?;
            Ok(DiffOps::insert(builder, insert))
        }
        EventType::Update {
            schema: source_schema,
            table: name,
            old_data,
            new_data,
            ..
        } => {
            let table = resolve_table(schema, Some(source_schema.as_ref()), name.as_ref())?;
            let update = update(old_data.as_ref(), new_data, table, adapter)?;
            Ok(DiffOps::update(builder, update))
        }
        EventType::Delete {
            schema: source_schema,
            table: name,
            old_data,
            ..
        } => {
            let table = resolve_table(schema, Some(source_schema.as_ref()), name.as_ref())?;
            let delete = delete(old_data, table, adapter)?;
            Ok(DiffOps::delete(builder, delete))
        }
        _ => Ok(builder),
    }
}

fn build_insert_from_pg<T, S, B, A>(
    data: &RowData,
    table: &T,
    adapter: &A,
) -> Result<Insert<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<PgWalstream, S, B>,
{
    build_insert(
        data.iter().map(|(name, value)| PgItem {
            name: name.as_ref(),
            data: value,
        }),
        table,
        adapter,
    )
}

fn build_changeset_update_from_pg<T, S, B, A>(
    old_data: Option<&RowData>,
    new_data: &RowData,
    table: &T,
    adapter: &A,
) -> Result<Update<T, ChangesetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + AsRef<str>,
    B: Clone + Debug + AsRef<[u8]>,
    A: WireAdapter<PgWalstream, S, B>,
{
    let mut update: Update<T, ChangesetFormat, S, B> = Update::from(table.clone());
    for (name, new_value) in new_data.iter() {
        let col_idx = table
            .column_index(name.as_ref())
            .ok_or_else(|| ConversionError::ColumnNotFound(name.as_ref().into()))?;
        let wire_type = table.column_type(col_idx);
        let new_payload = PgWalstreamColumn {
            column_name: name.as_ref(),
            wire_type,
            data: new_value,
        };
        let new_decoded = adapter.decode(new_payload)?;

        if let Some(old) = old_data
            && let Some(old_value) = old.get(name.as_ref())
        {
            let old_payload = PgWalstreamColumn {
                column_name: name.as_ref(),
                wire_type,
                data: old_value,
            };
            let old_decoded = adapter.decode(old_payload)?;
            update = update
                .set(col_idx, old_decoded, new_decoded)
                .map_err(|_| ConversionError::ColumnNotFound(name.as_ref().into()))?;
            continue;
        }

        // No old value on the wire. Under REPLICA IDENTITY DEFAULT a non-key
        // update sends no old tuple, so the key did not change and its old
        // value equals the new value. Keep it for primary-key columns so the
        // WHERE predicate can be built; other columns stay set_new.
        update = if table.primary_key_index(col_idx).is_some() {
            update
                .set(col_idx, new_decoded.clone(), new_decoded)
                .map_err(|_| ConversionError::ColumnNotFound(name.as_ref().into()))?
        } else {
            update
                .set_new(col_idx, new_decoded)
                .map_err(|_| ConversionError::ColumnNotFound(name.as_ref().into()))?
        };
    }
    Ok(update)
}

fn build_patchset_update_from_pg<T, S, B, A>(
    new_data: &RowData,
    table: &T,
    adapter: &A,
) -> Result<Update<T, PatchsetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<PgWalstream, S, B>,
{
    build_patchset_update(
        new_data.iter().map(|(name, value)| PgItem {
            name: name.as_ref(),
            data: value,
        }),
        table,
        adapter,
    )
}

fn build_changeset_delete_from_pg<T, S, B, A>(
    old_data: &RowData,
    table: &T,
    adapter: &A,
) -> Result<ChangeDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Default + AsRef<str>,
    B: Clone + Default + AsRef<[u8]>,
    A: WireAdapter<PgWalstream, S, B>,
{
    build_changeset_delete(
        old_data.iter().map(|(name, value)| PgItem {
            name: name.as_ref(),
            data: value,
        }),
        table,
        adapter,
    )
}

fn build_patch_delete_from_pg<T, S, B, A>(
    old_data: &RowData,
    table: &T,
    adapter: &A,
) -> Result<PatchDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<PgWalstream, S, B>,
{
    build_patch_delete(
        old_data.iter().map(|(name, value)| PgItem {
            name: name.as_ref(),
            data: value,
        }),
        table,
        adapter,
    )
}
