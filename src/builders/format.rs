//! Format trait defining changeset vs patchset behavior.

use crate::builders::Operation;
use crate::builders::change::{encode_changeset_op, encode_patchset_op, patchset_pk_mapping};
use crate::encoding::markers;
use crate::encoding::{MaybeValue, Value};
use crate::schema::SchemaWithPK;
use alloc::vec::Vec;
use core::fmt::Debug;

/// Trait defining the differences between changeset and patchset formats.
///
/// A changeset DELETE stores all column values and a changeset UPDATE stores
/// both old and new values. A patchset DELETE stores only the PK (data lives
/// externally) and a patchset UPDATE stores only the PK plus new values.
pub(crate) trait Format<S, B>: Default + Clone + Copy + PartialEq + Eq + 'static {
    /// The type representing old values in this format.
    ///
    /// - Changeset: `MaybeValue<S, B>` (Option<Value<S, B>>, None = undefined/unchanged)
    /// - Patchset: `()` (old values not stored)
    type Old: Clone + Debug + Default;

    /// The data stored for a DELETE operation (beyond the PK which is always
    /// stored as the `IndexMap` key in `DiffSetBuilder`).
    ///
    /// - Changeset: `Vec<Value<S, B>>` (full old-row values)
    /// - Patchset: `()` (only the PK matters, stored externally)
    type DeleteData: Clone + Debug + Default;

    /// One-byte table-section marker written before each table's rows:
    /// `b'T'` for a changeset, `b'P'` for a patchset.
    const TABLE_MARKER: u8;

    /// Per-table state precomputed once and reused for every row during
    /// [`build`](crate::DiffSetBuilder::build).
    ///
    /// Changeset needs none (`()`); patchset needs the primary-key column
    /// mapping used to project each record.
    type BuildState;

    /// Precompute the per-table [`BuildState`](Self::BuildState).
    fn build_state<T: SchemaWithPK>(table: &T) -> Self::BuildState;

    /// Encode one operation's record into `out`, using `pk` (the row's
    /// primary-key values) and the precomputed `state`.
    fn encode_op(
        out: &mut Vec<u8>,
        op: &Operation<Self, S, B>,
        pk: &[Value<S, B>],
        state: &Self::BuildState,
    ) where
        S: AsRef<str>,
        B: AsRef<[u8]>;

    /// `existing` followed by `new` on the same row of `table`, as `sqlite3changeset_concat`
    /// merges them, or `None` when they cancel out.
    fn merge<T: SchemaWithPK>(
        existing: Operation<Self, S, B>,
        new: Operation<Self, S, B>,
        table: &T,
    ) -> Option<Operation<Self, S, B>>
    where
        S: AsRef<str> + PartialEq,
        B: AsRef<[u8]> + PartialEq;
}

/// Public, nameable bound for a diff format, either changeset or patchset.
///
/// This is the downstream-visible counterpart to the crate-private
/// `Format` trait. It carries no items of its own; the associated types
/// live on the sealed supertrait and stay private. Because `Format` is
/// crate-private, no external type can satisfy this bound, so it is sealed
/// in the same sense as a sealed trait: only [`ChangesetFormat`] and
/// [`PatchsetFormat`] implement it.
///
/// A consumer that only calls `new`, `digest`, and `build` can therefore
/// write one function generic over `F: DiffFormat<String, Vec<u8>>` and
/// fold a batch of wire events into either a changeset or a patchset,
/// instead of duplicating the body once per format.
pub trait DiffFormat<S, B>: Format<S, B> {}

impl<S, B, F: Format<S, B>> DiffFormat<S, B> for F {}

/// Changeset format marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChangesetFormat;

impl<S: Clone + Debug + AsRef<str>, B: Clone + Debug + AsRef<[u8]>> Format<S, B>
    for ChangesetFormat
{
    type Old = MaybeValue<S, B>;
    type DeleteData = Vec<Value<S, B>>;
    const TABLE_MARKER: u8 = markers::CHANGESET;
    type BuildState = ();

    fn build_state<T: SchemaWithPK>(_table: &T) -> Self::BuildState {}

    fn encode_op(
        out: &mut Vec<u8>,
        op: &Operation<Self, S, B>,
        _pk: &[Value<S, B>],
        _state: &Self::BuildState,
    ) where
        S: AsRef<str>,
        B: AsRef<[u8]>,
    {
        encode_changeset_op(out, op);
    }

    fn merge<T: SchemaWithPK>(
        existing: Operation<Self, S, B>,
        new: Operation<Self, S, B>,
        table: &T,
    ) -> Option<Operation<Self, S, B>>
    where
        S: AsRef<str> + PartialEq,
        B: AsRef<[u8]> + PartialEq,
    {
        existing.merge(new, table)
    }
}

/// Patchset format marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PatchsetFormat;

impl<S, B> Format<S, B> for PatchsetFormat {
    type Old = ();
    type DeleteData = ();
    const TABLE_MARKER: u8 = markers::PATCHSET;
    type BuildState = (Vec<u8>, Vec<Option<usize>>);

    fn build_state<T: SchemaWithPK>(table: &T) -> Self::BuildState {
        patchset_pk_mapping(table)
    }

    fn encode_op(
        out: &mut Vec<u8>,
        op: &Operation<Self, S, B>,
        pk: &[Value<S, B>],
        state: &Self::BuildState,
    ) where
        S: AsRef<str>,
        B: AsRef<[u8]>,
    {
        encode_patchset_op(out, op, pk, &state.0, &state.1);
    }

    fn merge<T: SchemaWithPK>(
        existing: Operation<Self, S, B>,
        new: Operation<Self, S, B>,
        _table: &T,
    ) -> Option<Operation<Self, S, B>>
    where
        S: AsRef<str> + PartialEq,
        B: AsRef<[u8]> + PartialEq,
    {
        existing.merge(new)
    }
}
