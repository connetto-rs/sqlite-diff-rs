//! Maxwell message parsing and conversion to `SQLite` changeset operations.
//!
//! [Maxwell](https://maxwells-daemon.io/) is a `MySQL` CDC application that
//! reads binlogs and emits row-level changes as JSON. This module re-exports
//! the model and parser from [`maxwell_cdc`] and provides digest conversion
//! to changeset and patchset builders.
//!
//! Row events ([`Message::Insert`], [`Message::Update`], [`Message::Delete`],
//! [`Message::BootstrapInsert`]) are converted to changeset or patchset
//! operations. Control events ([`Message::BootstrapStart`],
//! [`Message::BootstrapComplete`], table events, and database events) pass
//! through without modifying the builder.
//!
//! Maxwell events carry no trigger-origin marker, so converted ops default
//! to `indirect = false`. Override via the [`Indirect`](crate::Indirect) trait
//! if you know out-of-band that the event was trigger-induced.
//!
//! # Example
//!
//! ```
//! use sqlite_diff_rs::maxwell::{parse, Message, OpType};
//!
//! let json = r#"{"database":"mydb","table":"users","type":"insert","ts":1477053217,"data":{"id":1,"name":"Alice"}}"#;
//! let message = parse(json).unwrap();
//!
//! assert_eq!(message.op_type(), Some(OpType::Insert));
//! if let Message::Insert(row) = &message {
//!     assert_eq!(row.table, "users");
//! }
//! ```

use alloc::string::String;

pub use maxwell_cdc::{
    ColumnDefinition, ControlMessage, DatabaseChange, DatabaseDefinition, DatabaseDropChange,
    DdlMetadata, Message, OpType, RowChange, TableAlterChange, TableCreateChange, TableDefinition,
    TableDropChange, parse,
};

pub use crate::wire::ConversionError;

use crate::ChangesetFormat;
use crate::builders::{ChangeDelete, Insert, PatchDelete, Update};
use crate::schema::NamedColumns;

use crate::wire::{
    Sealed, WireColumnItem, WireSource, WireType, build_changeset_delete, build_insert,
    build_patch_delete, build_patchset_update, resolve_table,
};

use crate::builders::{DeleteArg, DiffOps, DiffSetBuilder, Format, PatchsetFormat};
use crate::wire::WireAdapter;
use core::fmt::Debug;
use core::hash::Hash;

/// Marker type for the `maxwell` source.
#[derive(Debug, Clone, Copy, Default)]
pub struct Maxwell;

impl Sealed for Maxwell {}

impl WireSource for Maxwell {
    type Payload<'a> = MaxwellColumn<'a>;

    fn wire_type(payload: &Self::Payload<'_>) -> WireType {
        payload.wire_type
    }

    fn column_name<'a>(payload: &'a Self::Payload<'_>) -> &'a str {
        payload.column_name
    }
}

/// Per-column payload for the `maxwell` source.
///
/// The schema supplies the semantic [`WireType`] per column, independent
/// of the MySQL type name Maxwell may emit.
#[derive(Debug, Clone, Copy)]
pub struct MaxwellColumn<'a> {
    /// Column name.
    pub column_name: &'a str,
    /// Semantic column type driving decoder dispatch.
    pub wire_type: WireType,
    /// Column value as a JSON value.
    pub value: &'a serde_json::Value,
}

/// One Maxwell column adapted to the shared [`WireColumnItem`] contract.
struct MaxwellItem<'a> {
    name: &'a str,
    value: &'a serde_json::Value,
}

impl WireColumnItem<Maxwell> for MaxwellItem<'_> {
    fn name(&self) -> &str {
        self.name
    }

    fn payload(&self, wire_type: WireType) -> MaxwellColumn<'_> {
        MaxwellColumn {
            column_name: self.name,
            wire_type,
            value: self.value,
        }
    }
}

impl MaxwellColumn<'_> {
    /// Ergonomic helper for calling a specific [`Decoder`](crate::wire::Decoder) on this
    /// payload without fully-qualified syntax. Fixes the `Src` generic
    /// to [`Maxwell`] so the compiler can pick the impl.
    ///
    /// # Errors
    ///
    /// Propagates the decoder's [`DecodeError`](crate::wire::DecodeError).
    pub fn decoded_by<D, S, B>(
        self,
        decoder: &D,
    ) -> Result<crate::encoding::Value<S, B>, crate::wire::DecodeError>
    where
        D: crate::wire::Decoder<Maxwell, S, B>,
    {
        decoder.decode(self)
    }
}

use crate::wire::{Digestable, WireColumnTypes, WireSchema};

impl<T, S, B> Digestable<ChangesetFormat, T, S, B> for Message
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Maxwell;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<ChangesetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<ChangesetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Maxwell, S, B>,
    {
        digest(
            self,
            builder,
            schema,
            adapter,
            build_changeset_update_from_maxwell,
            build_changeset_delete_from_maxwell,
        )
    }
}

impl<T, S, B> Digestable<PatchsetFormat, T, S, B> for Message
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
{
    type Src = Maxwell;
    type Error = ConversionError;

    fn digest_into<Sch, A>(
        &self,
        builder: DiffSetBuilder<PatchsetFormat, T, S, B>,
        schema: &Sch,
        adapter: &A,
    ) -> Result<DiffSetBuilder<PatchsetFormat, T, S, B>, ConversionError>
    where
        Sch: WireSchema<Table = T>,
        A: WireAdapter<Maxwell, S, B>,
    {
        digest(
            self,
            builder,
            schema,
            adapter,
            |data, _old, table, adapter| build_patchset_update_from_maxwell(data, table, adapter),
            build_patch_delete_from_maxwell,
        )
    }
}

/// One Maxwell row image, column name to JSON value.
type RowImage = serde_json::Map<String, serde_json::Value>;

/// Digests one Maxwell message into `builder`, building its UPDATE and DELETE rows with
/// the format's `update` and `delete`.
fn digest<F, T, S, B, Sch, A>(
    message: &Message,
    builder: DiffSetBuilder<F, T, S, B>,
    schema: &Sch,
    adapter: &A,
    update: impl FnOnce(
        &RowImage,
        Option<&RowImage>,
        &T,
        &A,
    ) -> Result<Update<T, F, S, B>, ConversionError>,
    delete: impl FnOnce(&RowImage, &T, &A) -> Result<DeleteArg<F, T, S, B>, ConversionError>,
) -> Result<DiffSetBuilder<F, T, S, B>, ConversionError>
where
    F: Format<S, B>,
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + Hash + Eq + AsRef<str> + Default,
    B: Clone + Debug + Hash + Eq + AsRef<[u8]> + Default,
    Sch: WireSchema<Table = T>,
    A: WireAdapter<Maxwell, S, B>,
    DiffSetBuilder<F, T, S, B>: DiffOps<T, S, B, Format = F>,
{
    match message {
        Message::Insert(row) | Message::BootstrapInsert(row) => {
            let table = resolve_table(schema, Some(row.database.as_str()), row.table.as_str())?;
            let insert = build_insert_from_maxwell(&row.data, table, adapter)?;
            Ok(DiffOps::insert(builder, insert))
        }
        Message::Update(row) => {
            let table = resolve_table(schema, Some(row.database.as_str()), row.table.as_str())?;
            let update = update(&row.data, row.old.as_ref(), table, adapter)?;
            Ok(DiffOps::update(builder, update))
        }
        Message::Delete(row) => {
            let table = resolve_table(schema, Some(row.database.as_str()), row.table.as_str())?;
            let delete = delete(&row.data, table, adapter)?;
            Ok(DiffOps::delete(builder, delete))
        }
        // Bootstrap control events, DDL, and anything a newer Maxwell adds carry no
        // row data to digest. `Message` is non-exhaustive, so this cannot enumerate.
        _ => Ok(builder),
    }
}

fn build_insert_from_maxwell<T, S, B, A>(
    data: &serde_json::Map<String, serde_json::Value>,
    table: &T,
    adapter: &A,
) -> Result<Insert<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Maxwell, S, B>,
{
    build_insert(
        data.iter().map(|(name, value)| MaxwellItem {
            name: name.as_str(),
            value,
        }),
        table,
        adapter,
    )
}

fn build_changeset_update_from_maxwell<T, S, B, A>(
    data: &serde_json::Map<String, serde_json::Value>,
    old: Option<&serde_json::Map<String, serde_json::Value>>,
    table: &T,
    adapter: &A,
) -> Result<Update<T, ChangesetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Debug + AsRef<str>,
    B: Clone + Debug + AsRef<[u8]>,
    A: WireAdapter<Maxwell, S, B>,
{
    let mut update: Update<T, ChangesetFormat, S, B> = Update::from(table.clone());
    for (name, new_value) in data {
        let col_idx = table
            .column_index(name)
            .ok_or_else(|| ConversionError::ColumnNotFound(name.clone()))?;
        let wire_type = table.column_type(col_idx);

        let new_payload = MaxwellColumn {
            column_name: name.as_str(),
            wire_type,
            value: new_value,
        };
        let new = adapter.decode(new_payload)?;

        // A column present in `old` changed, so pair its old and new values. A
        // column absent from `old` did not change, so its old value equals the
        // new value. That keeps an unchanged primary key available for the
        // WHERE clause and unchanged columns out of the SET.
        let old = if let Some(old_value) = old.and_then(|old_map| old_map.get(name)) {
            let old_payload = MaxwellColumn {
                column_name: name.as_str(),
                wire_type,
                value: old_value,
            };
            adapter.decode(old_payload)?
        } else {
            new.clone()
        };
        update = update
            .set(col_idx, old, new)
            .map_err(|_| ConversionError::ColumnNotFound(name.clone()))?;
    }
    Ok(update)
}

fn build_patchset_update_from_maxwell<T, S, B, A>(
    data: &serde_json::Map<String, serde_json::Value>,
    table: &T,
    adapter: &A,
) -> Result<Update<T, PatchsetFormat, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Maxwell, S, B>,
{
    build_patchset_update(
        data.iter().map(|(name, value)| MaxwellItem {
            name: name.as_str(),
            value,
        }),
        table,
        adapter,
    )
}

fn build_changeset_delete_from_maxwell<T, S, B, A>(
    data: &serde_json::Map<String, serde_json::Value>,
    table: &T,
    adapter: &A,
) -> Result<ChangeDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + Default + AsRef<str>,
    B: Clone + Default + AsRef<[u8]>,
    A: WireAdapter<Maxwell, S, B>,
{
    build_changeset_delete(
        data.iter().map(|(name, value)| MaxwellItem {
            name: name.as_str(),
            value,
        }),
        table,
        adapter,
    )
}

fn build_patch_delete_from_maxwell<T, S, B, A>(
    data: &serde_json::Map<String, serde_json::Value>,
    table: &T,
    adapter: &A,
) -> Result<PatchDelete<T, S, B>, ConversionError>
where
    T: NamedColumns + WireColumnTypes,
    S: Clone + AsRef<str>,
    B: Clone + AsRef<[u8]>,
    A: WireAdapter<Maxwell, S, B>,
{
    build_patch_delete(
        data.iter().map(|(name, value)| MaxwellItem {
            name: name.as_str(),
            value,
        }),
        table,
        adapter,
    )
}
