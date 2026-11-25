//! `diesel` feature: makes changeset and patchset operations executable
//! Diesel queries.
//!
//! Identifiers are quoted per backend via [`AstPass::push_identifier`] and
//! values bind via [`AstPass::push_bind_param`] with the target column's
//! diesel `SqlType`, so no `CAST` wrappers appear. `Value::Null` renders as
//! the literal `NULL`.
//!
//! `PatchSet` and `ChangeSet` share one path: iterate, optionally attach an
//! [`Adapter`], then execute. They differ only in how each op normalizes into
//! a [`RenderPlan`]. A changeset carries old and new values, so it alone can
//! render a primary-key change that moves a row to a new key.
//!
//! # Example: native BOOLEAN bind through an [`Adapter`]
//!
//! The source stores `Value::Integer(1)` (SQLite has no boolean type) and the
//! target Postgres column is `BOOLEAN`. The adapter binds it as a native
//! `bool` through a `BoolBinder`, so no `CAST` appears in the emitted SQL.
//!
//! ```
//! use diesel::debug_query;
//! use diesel::pg::Pg;
//! use diesel::query_builder::AstPass;
//! use diesel::result::QueryResult;
//! use diesel::sql_types::Bool;
//! use sqlite_diff_rs::{
//!     Adapter, Binder, DefaultBinder, DiffOps, Insert, PatchSet, SimpleTable, Value,
//! };
//!
//! struct BoolBinder(bool);
//! impl Binder<Pg> for BoolBinder {
//!     fn walk<'b>(&'b self, out: &mut AstPass<'_, 'b, Pg>) -> QueryResult<()> {
//!         out.push_bind_param::<Bool, bool>(&self.0)
//!     }
//! }
//!
//! struct UsersAdapter;
//! impl<S, B> Adapter<Pg, S, B> for UsersAdapter
//! where
//!     S: AsRef<str> + Sync,
//!     B: AsRef<[u8]> + Sync,
//! {
//!     fn column_name(&self, _table: &str, index: usize) -> &str {
//!         ["id", "active"][index]
//!     }
//!     fn bind<'a>(
//!         &self,
//!         table: &str,
//!         column_index: usize,
//!         value: &'a Value<S, B>,
//!     ) -> diesel::result::QueryResult<Box<dyn Binder<Pg> + Send + 'a>> {
//!         match (table, column_index, value) {
//!             ("users", 1, Value::Integer(i)) => Ok(Box::new(BoolBinder(*i != 0))),
//!             _ => Ok(Box::new(DefaultBinder::from(value))),
//!         }
//!     }
//! }
//!
//! let table = SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("active", "INTEGER")], 0);
//! let patchset = PatchSet::<SimpleTable, String, Vec<u8>>::new().insert(
//!     Insert::from(table.clone())
//!         .set(0, 1_i64)
//!         .unwrap()
//!         .set(1, 1_i64)
//!         .unwrap(),
//! );
//!
//! let op = patchset
//!     .iter()
//!     .next()
//!     .unwrap()
//!     .with_adapter::<Pg, _>(&UsersAdapter);
//! let sql = debug_query::<Pg, _>(&op).to_string();
//! assert!(sql.starts_with(r#"INSERT INTO "users" ("id", "active") VALUES ($1, $2)"#));
//! assert!(!sql.contains("CAST")); // native bind, no cast wrapper
//! assert!(sql.contains("true")); // bind rendered via bool's Debug
//! ```

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use diesel::backend::Backend;
use diesel::query_builder::{AstPass, QueryFragment, QueryId};
use diesel::query_dsl::RunQueryDsl;
use diesel::result::{Error as DieselError, QueryResult};
use diesel::serialize::ToSql;
use diesel::sql_types::{BigInt, Binary, Double, HasSqlType, Text};

use super::sql_output::ColumnNames;
use super::view::{ChangesetOp, ChangesetUpdatePairExt, PatchsetOp};
use crate::encoding::Value;
use crate::{DynTable, SchemaWithPK};

/// Reasons an op cannot render into valid SQL. Wrapped in
/// [`DieselError::QueryBuilderError`].
#[derive(Debug, Clone, thiserror::Error)]
enum RenderError {
    /// `ColumnNames::column_name(index)` returned `None`.
    #[error("missing column name for index {column_index}")]
    MissingColumnName { column_index: usize },
    /// An UPDATE with no columns to write, so its `SET` clause is empty.
    #[error("UPDATE has an empty SET clause")]
    EmptyUpdateSet,
    /// An INSERT whose every column the [`Adapter`] leaves out.
    #[error("INSERT leaves out every column")]
    EmptyInsert,
    /// UPDATE/DELETE against a table with no primary-key columns.
    #[error("UPDATE/DELETE targets a table with no primary key")]
    EmptyRowPredicate,
    /// A changeset UPDATE lacks the old value of a primary-key column, so
    /// the row cannot be located by its `WHERE` predicate.
    #[error("changeset UPDATE is missing the old value of primary-key column {column_index}")]
    MissingPkValue { column_index: usize },
    /// [`Adapter::bind`] returned `Err`, carrying that error's display text.
    #[error("adapter rejected column {column_index} of table {table_name:?}: {message}")]
    AdapterBindFailure {
        table_name: alloc::string::String,
        column_index: usize,
        message: alloc::string::String,
    },
}

#[inline]
fn render_err(err: RenderError) -> DieselError {
    DieselError::QueryBuilderError(Box::new(err))
}

#[inline]
fn column_name_at<T: ColumnNames>(table: &T, index: usize) -> QueryResult<&str> {
    table.column_name(index).ok_or_else(|| {
        render_err(RenderError::MissingColumnName {
            column_index: index,
        })
    })
}

/// Backends that can bind every [`Value`] variant natively. Blanket
/// implemented, so a single `DB: ValueBackend` bound replaces the four
/// `HasSqlType` and four `ToSql` facts the binding path needs.
pub trait ValueBackend: Backend {
    /// Bind `value` with the diesel `SqlType` matching its variant.
    /// `Value::Null` renders as the literal `NULL`.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`AstPass::push_bind_param`].
    fn bind_value<'b, S, B>(
        out: &mut AstPass<'_, 'b, Self>,
        value: &'b Value<S, B>,
    ) -> QueryResult<()>
    where
        S: AsRef<str>,
        B: AsRef<[u8]>;
}

impl<DB> ValueBackend for DB
where
    DB: Backend + HasSqlType<BigInt> + HasSqlType<Double> + HasSqlType<Text> + HasSqlType<Binary>,
    i64: ToSql<BigInt, DB>,
    f64: ToSql<Double, DB>,
    str: ToSql<Text, DB>,
    [u8]: ToSql<Binary, DB>,
{
    fn bind_value<'b, S, B>(
        out: &mut AstPass<'_, 'b, Self>,
        value: &'b Value<S, B>,
    ) -> QueryResult<()>
    where
        S: AsRef<str>,
        B: AsRef<[u8]>,
    {
        match value {
            Value::Null => {
                out.push_sql("NULL");
                Ok(())
            }
            Value::Integer(i) => out.push_bind_param::<BigInt, i64>(i),
            Value::Real(f) => out.push_bind_param::<Double, f64>(f),
            Value::Text(s) => out.push_bind_param::<Text, str>(s.as_ref()),
            Value::Blob(b) => out.push_bind_param::<Binary, [u8]>(b.as_ref()),
        }
    }
}

/// One adapter-resolved column binder, with the index of its column.
type ResolvedBinder<'a, DB> = (usize, Box<dyn Binder<DB> + Send + 'a>);

/// One `(column index, value)` pair for a SET or WHERE clause.
type Assignment<'a, S, B> = (usize, &'a Value<S, B>);

// ============================================================================
// RenderPlan: format-agnostic normalized shape of one op.
// ============================================================================

/// Normalized shape of one op, every per-format decision resolved. Both
/// formats collapse to this, and all rendering runs over it.
enum RenderPlan<'a, S, B> {
    /// `INSERT` with the full row, one value per column in column order.
    Insert {
        /// Full row values.
        values: &'a [Value<S, B>],
    },
    /// `UPDATE`: `set` = `(column, new value)` to assign, `predicate` =
    /// `(column, key value)` locating the row, in PK-ordinal order.
    Update {
        /// Columns to write, with their new values.
        set: Vec<Assignment<'a, S, B>>,
        /// Primary-key predicate, in PK-ordinal order.
        predicate: Vec<Assignment<'a, S, B>>,
    },
    /// `DELETE`: `predicate` locates the row, in PK-ordinal order.
    Delete {
        /// Primary-key predicate, in PK-ordinal order.
        predicate: Vec<Assignment<'a, S, B>>,
    },
}

/// Build a PK `WHERE` predicate. `value_for(pk_ordinal, col_idx)` supplies
/// the key value for each PK column (from a compact `pk` slice for patchsets,
/// or from old-row values for changesets).
fn build_predicate<'a, T, S, B, F>(
    table: &T,
    mut value_for: F,
) -> Result<Vec<Assignment<'a, S, B>>, RenderError>
where
    T: SchemaWithPK,
    F: FnMut(usize, usize) -> Result<&'a Value<S, B>, RenderError>,
{
    let key_width = table.number_of_primary_keys();
    if key_width == 0 {
        return Err(RenderError::EmptyRowPredicate);
    }
    let mut predicate = Vec::with_capacity(key_width);
    for (pk_ordinal, col_idx) in table.primary_key_columns().enumerate() {
        predicate.push((col_idx, value_for(pk_ordinal, col_idx)?));
    }
    Ok(predicate)
}

/// One op view normalized for Diesel rendering. [`Self::render_plan`] is the
/// sole place [`PatchsetOp`] and [`ChangesetOp`] differ.
trait DieselRenderable<'a, S, B> {
    /// Schema type of the target table.
    type Table: SchemaWithPK;
    /// The target table schema.
    fn table(&self) -> &'a Self::Table;
    /// Normalize this op into a [`RenderPlan`], or report why it cannot
    /// render into valid SQL.
    fn render_plan(&self) -> Result<RenderPlan<'a, S, B>, RenderError>;
}

impl<'a, T, S, B> DieselRenderable<'a, S, B> for PatchsetOp<'a, T, S, B>
where
    T: SchemaWithPK,
{
    type Table = T;

    fn table(&self) -> &'a T {
        PatchsetOp::table(self)
    }

    fn render_plan(&self) -> Result<RenderPlan<'a, S, B>, RenderError> {
        match *self {
            PatchsetOp::Insert { values, .. } => Ok(RenderPlan::Insert { values }),
            PatchsetOp::Update {
                table, pk, entries, ..
            } => {
                // SET: non-PK columns carrying a new value. A patchset stores
                // no new PK value, so PK columns never appear here.
                let mut set = Vec::new();
                for (col_idx, ((), new)) in entries.iter().enumerate() {
                    if let Some(value) = new.as_ref()
                        && table.primary_key_index(col_idx).is_none()
                    {
                        set.push((col_idx, value));
                    }
                }
                if set.is_empty() {
                    return Err(RenderError::EmptyUpdateSet);
                }
                let predicate = build_predicate(table, |pk_ordinal, _col_idx| Ok(&pk[pk_ordinal]))?;
                Ok(RenderPlan::Update { set, predicate })
            }
            PatchsetOp::Delete { table, pk, .. } => {
                let predicate = build_predicate(table, |pk_ordinal, _col_idx| Ok(&pk[pk_ordinal]))?;
                Ok(RenderPlan::Delete { predicate })
            }
        }
    }
}

impl<'a, T, S, B> DieselRenderable<'a, S, B> for ChangesetOp<'a, T, S, B>
where
    T: SchemaWithPK,
    S: AsRef<str> + PartialEq,
    B: AsRef<[u8]> + PartialEq,
{
    type Table = T;

    fn table(&self) -> &'a T {
        ChangesetOp::table(self)
    }

    fn render_plan(&self) -> Result<RenderPlan<'a, S, B>, RenderError> {
        match *self {
            ChangesetOp::Insert { values, .. } => Ok(RenderPlan::Insert { values }),
            ChangesetOp::Update { table, values, .. } => {
                // SET each column whose new value differs from its old value.
                // A changed PK column lands here, an unchanged one does not,
                // so the UPDATE never rewrites a column that did not move.
                let mut set = Vec::new();
                for (col_idx, pair) in values.iter().enumerate() {
                    if let Some(new_value) = pair.1.as_ref()
                        && pair.is_changed()
                    {
                        set.push((col_idx, new_value));
                    }
                }
                if set.is_empty() {
                    return Err(RenderError::EmptyUpdateSet);
                }
                // WHERE: old PK values, so a PK change locates the old row.
                let predicate = build_predicate(table, |_pk_ordinal, col_idx| {
                    values[col_idx]
                        .0
                        .as_ref()
                        .ok_or(RenderError::MissingPkValue {
                            column_index: col_idx,
                        })
                })?;
                Ok(RenderPlan::Update { set, predicate })
            }
            ChangesetOp::Delete {
                table, old_values, ..
            } => {
                let predicate =
                    build_predicate(table, |_pk_ordinal, col_idx| Ok(&old_values[col_idx]))?;
                Ok(RenderPlan::Delete { predicate })
            }
        }
    }
}

// ============================================================================
// ClauseSink: bridges the naive and adapter paths for one clause.
// ============================================================================

/// Emits a column identifier and its bound value for one clause entry.
///
/// [`NaiveSink`] takes names from [`ColumnNames`] and values from
/// [`ValueBackend::bind_value`]. The adapter path takes names from an
/// [`Adapter`] and values from pre-resolved [`Binder`]s.
trait ClauseSink<'b, S, B, DB: Backend> {
    /// Identifier for `col_idx`.
    fn column_name(&self, col_idx: usize) -> QueryResult<&str>;
    /// Whether an `INSERT` names `col_idx`, carrying `value`.
    fn keeps_insert_column(&self, col_idx: usize, value: &Value<S, B>) -> bool;
    /// Emit the value for `col_idx`. `value` is the plan's value; the
    /// adapter path may ignore it in favor of a pre-resolved binder.
    fn emit_value(
        &mut self,
        col_idx: usize,
        value: &'b Value<S, B>,
        out: &mut AstPass<'_, 'b, DB>,
    ) -> QueryResult<()>;
}

/// Naive sink: names from the schema, values bound by [`ValueBackend::bind_value`].
struct NaiveSink<'t, T> {
    table: &'t T,
}

impl<'b, T, S, B, DB> ClauseSink<'b, S, B, DB> for NaiveSink<'_, T>
where
    T: ColumnNames,
    S: AsRef<str>,
    B: AsRef<[u8]>,
    DB: ValueBackend,
{
    fn column_name(&self, col_idx: usize) -> QueryResult<&str> {
        column_name_at(self.table, col_idx)
    }

    fn keeps_insert_column(&self, _col_idx: usize, _value: &Value<S, B>) -> bool {
        true
    }

    fn emit_value(
        &mut self,
        _col_idx: usize,
        value: &'b Value<S, B>,
        out: &mut AstPass<'_, 'b, DB>,
    ) -> QueryResult<()> {
        DB::bind_value(out, value)
    }
}

// ============================================================================
// Shared rendering: one walk for both formats and both paths.
// ============================================================================

/// Render an assignment list (`col = value`) joined by `separator`.
fn walk_assignments<'p, 'b, S, B, DB, K>(
    items: &[Assignment<'p, S, B>],
    separator: &str,
    sink: &mut K,
    out: &mut AstPass<'_, 'b, DB>,
) -> QueryResult<()>
where
    'p: 'b,
    DB: Backend,
    K: ClauseSink<'b, S, B, DB>,
{
    for (position, (col_idx, value)) in items.iter().copied().enumerate() {
        if position > 0 {
            out.push_sql(separator);
        }
        out.push_identifier(sink.column_name(col_idx)?)?;
        out.push_sql(" = ");
        sink.emit_value(col_idx, value, out)?;
    }
    Ok(())
}

/// Render a whole [`RenderPlan`] through `sink`.
fn walk_plan<'p, 'b, S, B, DB, K>(
    plan: RenderPlan<'p, S, B>,
    table_name: &str,
    sink: &mut K,
    out: &mut AstPass<'_, 'b, DB>,
) -> QueryResult<()>
where
    'p: 'b,
    DB: Backend,
    K: ClauseSink<'b, S, B, DB>,
{
    match plan {
        RenderPlan::Insert { values } => {
            out.push_sql("INSERT INTO ");
            out.push_identifier(table_name)?;
            let mut named = 0_usize;
            for (col_idx, value) in values.iter().enumerate() {
                if !sink.keeps_insert_column(col_idx, value) {
                    continue;
                }
                out.push_sql(if named == 0 { " (" } else { ", " });
                out.push_identifier(sink.column_name(col_idx)?)?;
                named += 1;
            }
            if named == 0 {
                return Err(render_err(RenderError::EmptyInsert));
            }
            out.push_sql(") VALUES (");
            let mut emitted = 0_usize;
            for (col_idx, value) in values.iter().enumerate() {
                if !sink.keeps_insert_column(col_idx, value) {
                    continue;
                }
                if emitted > 0 {
                    out.push_sql(", ");
                }
                sink.emit_value(col_idx, value, out)?;
                emitted += 1;
            }
            out.push_sql(")");
            Ok(())
        }
        RenderPlan::Update { set, predicate } => {
            out.push_sql("UPDATE ");
            out.push_identifier(table_name)?;
            out.push_sql(" SET ");
            walk_assignments(&set, ", ", sink, out)?;
            out.push_sql(" WHERE ");
            walk_assignments(&predicate, " AND ", sink, out)
        }
        RenderPlan::Delete { predicate } => {
            out.push_sql("DELETE FROM ");
            out.push_identifier(table_name)?;
            out.push_sql(" WHERE ");
            walk_assignments(&predicate, " AND ", sink, out)
        }
    }
}

/// Render `op` through the naive path (schema names, native value binds).
fn walk_naive<'a, 'b, V, T, S, B, DB>(op: &'b V, mut out: AstPass<'_, 'b, DB>) -> QueryResult<()>
where
    'a: 'b,
    V: DieselRenderable<'a, S, B, Table = T>,
    T: ColumnNames + 'a,
    S: AsRef<str> + 'a,
    B: AsRef<[u8]> + 'a,
    DB: ValueBackend,
{
    // Identifiers are chosen at runtime, so the prepared-statement cache must
    // not retain this query.
    out.unsafe_to_cache_prepared();
    let plan = op.render_plan().map_err(render_err)?;
    let table = op.table();
    let mut sink = NaiveSink { table };
    walk_plan(plan, table.name(), &mut sink, &mut out)
}

impl<T, S, B, DB> QueryFragment<DB> for PatchsetOp<'_, T, S, B>
where
    T: ColumnNames,
    S: AsRef<str>,
    B: AsRef<[u8]>,
    DB: ValueBackend,
{
    fn walk_ast<'b>(&'b self, out: AstPass<'_, 'b, DB>) -> QueryResult<()> {
        walk_naive(self, out)
    }
}

impl<T, S, B, DB> QueryFragment<DB> for ChangesetOp<'_, T, S, B>
where
    T: ColumnNames,
    S: AsRef<str> + PartialEq,
    B: AsRef<[u8]> + PartialEq,
    DB: ValueBackend,
{
    fn walk_ast<'b>(&'b self, out: AstPass<'_, 'b, DB>) -> QueryResult<()> {
        walk_naive(self, out)
    }
}

impl<T, S, B> QueryId for PatchsetOp<'_, T, S, B> {
    type QueryId = ();
    const HAS_STATIC_QUERY_ID: bool = false;
}

impl<T, S, B> QueryId for ChangesetOp<'_, T, S, B> {
    type QueryId = ();
    const HAS_STATIC_QUERY_ID: bool = false;
}

// `RunQueryDsl<Conn>` has a blanket impl only for `T: Table`, and the op views
// are not tables. We add unconditional impls the same way `SqlQuery` and
// `BoxedSqlQuery` do; the trait's own methods carry their bounds.
impl<T, S, B, Conn> RunQueryDsl<Conn> for PatchsetOp<'_, T, S, B> {}
impl<T, S, B, Conn> RunQueryDsl<Conn> for ChangesetOp<'_, T, S, B> {}

// ============================================================================
// Adapter / Binder: user-driven per-column native binding, no CAST wrappers.
// ============================================================================

/// Writes one column value onto the query AST.
///
/// Owns any parsed representation the target column needs (`bool`,
/// `uuid::Uuid`, ...) so the value survives [`AstPass::push_bind_param`]'s
/// bind-lifetime requirement.
pub trait Binder<DB: Backend> {
    /// Push the column's value onto the AST. Typically calls
    /// [`AstPass::push_bind_param`] with the diesel `SqlType` matching the
    /// target column, or emits a literal via [`AstPass::push_sql`].
    ///
    /// # Errors
    ///
    /// Propagates any [`DieselError`](diesel::result::Error) from
    /// [`AstPass`].
    fn walk<'b>(&'b self, out: &mut AstPass<'_, 'b, DB>) -> QueryResult<()>;
}

/// The naive binder used when nothing custom is required for a column.
///
/// Binds `Integer -> BigInt`, `Real -> Double`, `Text -> Text`,
/// `Blob -> Binary`; pushes literal `NULL` for `Value::Null`.
pub struct DefaultBinder<'a, S, B> {
    value: &'a Value<S, B>,
}

impl<'a, S, B> DefaultBinder<'a, S, B> {
    /// Construct a default binder that reads through to `value`.
    #[must_use]
    pub fn new(value: &'a Value<S, B>) -> Self {
        Self { value }
    }
}

impl<'a, S, B> From<&'a Value<S, B>> for DefaultBinder<'a, S, B> {
    fn from(value: &'a Value<S, B>) -> Self {
        Self { value }
    }
}

impl<S, B, DB> Binder<DB> for DefaultBinder<'_, S, B>
where
    S: AsRef<str>,
    B: AsRef<[u8]>,
    DB: ValueBackend,
{
    fn walk<'b>(&'b self, out: &mut AstPass<'_, 'b, DB>) -> QueryResult<()> {
        DB::bind_value(out, self.value)
    }
}

/// Downstream source of truth for column names and native bindings. One
/// adapter per schema, dispatching on `table_name`. The wire format carries
/// table names and column positions but not names or types, so the adapter
/// supplies them. Object-safe.
pub trait Adapter<DB, S, B> {
    /// Column identifier emitted via `push_identifier` for
    /// `(table_name, column_index)`.
    fn column_name(&self, table_name: &str, column_index: usize) -> &str;

    /// Binder for `(table_name, column_index, value)`. Parse `value` into the
    /// owned representation the target column needs and box it, or fall
    /// through to [`DefaultBinder`].
    ///
    /// # Errors
    ///
    /// Return an error when the value cannot represent the target column's SQL
    /// type, for example a `Value::Integer` for a `UUID` column. It surfaces
    /// as [`DieselError::QueryBuilderError`] when the [`BoundOp`] is executed.
    fn bind<'a>(
        &self,
        table_name: &str,
        column_index: usize,
        value: &'a Value<S, B>,
    ) -> QueryResult<Box<dyn Binder<DB> + Send + 'a>>;

    /// Whether an `INSERT` leaves column `column_index`, carrying `value`, out
    /// of both its column list and `VALUES`, so the row takes the column's
    /// default and the role needs no `INSERT` privilege on it. The default
    /// keeps every column, and an `INSERT` left with none fails to render.
    fn omits_from_insert(
        &self,
        _table_name: &str,
        _column_index: usize,
        _value: &Value<S, B>,
    ) -> bool {
        false
    }
}

// ============================================================================
// Adapter path: resolve binders up front, walk in lockstep.
// ============================================================================

/// Resolve every column of `plan` through `adapter`, in the exact order
/// [`walk_plan`] emits placeholders, so the two iterate in lockstep.
fn resolve_binders<'a, S, B, DB, A>(
    plan: RenderPlan<'a, S, B>,
    table_name: &str,
    adapter: &A,
) -> Result<Vec<ResolvedBinder<'a, DB>>, RenderError>
where
    DB: Backend,
    A: Adapter<DB, S, B>,
{
    let mut binders: Vec<ResolvedBinder<'a, DB>> = Vec::new();
    {
        let mut bind = |col_idx: usize, value: &'a Value<S, B>| -> Result<(), RenderError> {
            let binder = adapter.bind(table_name, col_idx, value).map_err(|err| {
                RenderError::AdapterBindFailure {
                    table_name: table_name.into(),
                    column_index: col_idx,
                    message: err.to_string(),
                }
            })?;
            binders.push((col_idx, binder));
            Ok(())
        };
        match plan {
            RenderPlan::Insert { values } => {
                for (col_idx, value) in values.iter().enumerate() {
                    if !adapter.omits_from_insert(table_name, col_idx, value) {
                        bind(col_idx, value)?;
                    }
                }
            }
            RenderPlan::Update { set, predicate } => {
                for (col_idx, value) in set {
                    bind(col_idx, value)?;
                }
                for (col_idx, value) in predicate {
                    bind(col_idx, value)?;
                }
            }
            RenderPlan::Delete { predicate } => {
                for (col_idx, value) in predicate {
                    bind(col_idx, value)?;
                }
            }
        }
    }
    Ok(binders)
}

/// Adapter sink: names from the adapter, values and kept `INSERT` columns
/// from the binders [`resolve_binders`] resolved.
struct BoundSink<'r, 'a, S, B, DB, A>
where
    DB: Backend,
{
    binders: &'r [ResolvedBinder<'a, DB>],
    next: usize,
    adapter: &'a A,
    table_name: &'a str,
    _marker: core::marker::PhantomData<(S, B)>,
}

impl<'b, 'a, S, B, DB, A> ClauseSink<'b, S, B, DB> for BoundSink<'b, 'a, S, B, DB, A>
where
    'a: 'b,
    DB: Backend,
    A: Adapter<DB, S, B>,
{
    fn column_name(&self, col_idx: usize) -> QueryResult<&str> {
        Ok(self.adapter.column_name(self.table_name, col_idx))
    }

    fn keeps_insert_column(&self, col_idx: usize, _value: &Value<S, B>) -> bool {
        // An INSERT resolves its kept columns in column order.
        self.binders
            .binary_search_by_key(&col_idx, |&(resolved_idx, _)| resolved_idx)
            .is_ok()
    }

    fn emit_value(
        &mut self,
        col_idx: usize,
        _value: &'b Value<S, B>,
        out: &mut AstPass<'_, 'b, DB>,
    ) -> QueryResult<()> {
        let binder = match self.binders.get(self.next) {
            Some((resolved_idx, binder)) if *resolved_idx == col_idx => binder,
            _ => {
                return Err(DieselError::QueryBuilderError(Box::new(
                    BinderResolutionError,
                )));
            }
        };
        self.next += 1;
        binder.walk(out)
    }
}

/// Executable Diesel query from one op view plus an [`Adapter`], built via
/// [`PatchsetOp::with_adapter`] or [`ChangesetOp::with_adapter`] and usually
/// named through the [`BoundPatchsetOp`] / [`BoundChangesetOp`] aliases.
pub struct BoundOp<'a, V, S, B, DB, A>
where
    DB: Backend,
{
    op: V,
    resolved: Result<Vec<ResolvedBinder<'a, DB>>, RenderError>,
    adapter: &'a A,
    _marker: core::marker::PhantomData<(S, B)>,
}

impl<'a, V, S, B, DB, A> BoundOp<'a, V, S, B, DB, A>
where
    V: DieselRenderable<'a, S, B> + 'a,
    S: 'a,
    B: 'a,
    DB: Backend,
    A: Adapter<DB, S, B> + Send + Sync,
{
    /// Resolve `op` against `adapter` up front. Any bind or render error is
    /// captured here and re-surfaced when the query is walked.
    fn resolve(op: V, adapter: &'a A) -> Self {
        let table_name = op.table().name();
        let resolved = op
            .render_plan()
            .and_then(|plan| resolve_binders(plan, table_name, adapter));
        Self {
            op,
            resolved,
            adapter,
            _marker: core::marker::PhantomData,
        }
    }
}

impl<'a, V, T, S, B, DB, A> QueryFragment<DB> for BoundOp<'a, V, S, B, DB, A>
where
    V: DieselRenderable<'a, S, B, Table = T>,
    T: SchemaWithPK + 'a,
    S: AsRef<str> + 'a,
    B: AsRef<[u8]> + 'a,
    DB: Backend,
    A: Adapter<DB, S, B> + Send + Sync,
{
    fn walk_ast<'b>(&'b self, mut out: AstPass<'_, 'b, DB>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();
        let binders = self
            .resolved
            .as_ref()
            .map_err(|err| render_err(err.clone()))?;
        let plan = self.op.render_plan().map_err(render_err)?;
        let table_name = self.op.table().name();
        let mut sink = BoundSink {
            binders,
            next: 0,
            adapter: self.adapter,
            table_name,
            _marker: core::marker::PhantomData,
        };
        walk_plan(plan, table_name, &mut sink, &mut out)
    }
}

impl<V, S, B, DB, A> QueryId for BoundOp<'_, V, S, B, DB, A>
where
    DB: Backend,
{
    type QueryId = ();
    const HAS_STATIC_QUERY_ID: bool = false;
}

impl<V, S, B, DB, A, Conn> RunQueryDsl<Conn> for BoundOp<'_, V, S, B, DB, A> where DB: Backend {}

/// Internal error: the binders resolved up front disagree with the columns
/// `walk_ast` emits. Only reachable through a bookkeeping bug in this crate.
#[derive(Debug, thiserror::Error)]
#[error("internal: BoundOp resolve and walk_ast disagree about the bound columns")]
struct BinderResolutionError;

/// Executable Diesel query built from one [`PatchsetOp`] + [`Adapter`].
pub type BoundPatchsetOp<'a, T, S, B, DB, A> = BoundOp<'a, PatchsetOp<'a, T, S, B>, S, B, DB, A>;

/// Executable Diesel query built from one [`ChangesetOp`] + [`Adapter`].
pub type BoundChangesetOp<'a, T, S, B, DB, A> = BoundOp<'a, ChangesetOp<'a, T, S, B>, S, B, DB, A>;

// ============================================================================
// Entry points on the op views
// ============================================================================

impl<'a, T, S, B> PatchsetOp<'a, T, S, B>
where
    T: SchemaWithPK,
    S: AsRef<str>,
    B: AsRef<[u8]>,
{
    /// Build an executable [`BoundPatchsetOp`] from this op and `adapter`.
    #[must_use]
    pub fn with_adapter<DB, A>(self, adapter: &'a A) -> BoundPatchsetOp<'a, T, S, B, DB, A>
    where
        A: Adapter<DB, S, B> + Send + Sync,
        DB: Backend,
    {
        BoundOp::resolve(self, adapter)
    }
}

impl<'a, T, S, B> ChangesetOp<'a, T, S, B>
where
    T: SchemaWithPK,
    S: AsRef<str> + PartialEq,
    B: AsRef<[u8]> + PartialEq,
{
    /// Build an executable [`BoundChangesetOp`] from this op and `adapter`.
    #[must_use]
    pub fn with_adapter<DB, A>(self, adapter: &'a A) -> BoundChangesetOp<'a, T, S, B, DB, A>
    where
        A: Adapter<DB, S, B> + Send + Sync,
        DB: Backend,
    {
        BoundOp::resolve(self, adapter)
    }
}

// ============================================================================
// ApplyOps: batch execute an op iterator without hand-rolled ceremony.
// ============================================================================

/// Extension trait: run every op in an iterator against a Diesel connection.
///
/// Works uniformly on the naive path (`set.iter()`) and the adapter path
/// (`set.iter().map(|op| op.with_adapter::<DB, _>(&adapter))`), for both
/// patchsets and changesets.
///
/// ```
/// use sqlite_diff_rs::ApplyOps;
/// // With `ApplyOps` in scope, on any iterator of Diesel-executable ops:
/// //
/// //   set.iter().apply_transactional(&mut conn)?;                    // naive path
/// //   set.iter()                                                     // adapter path
/// //       .map(|op| op.with_adapter::<Pg, _>(&adapter))
/// //       .apply_transactional(&mut conn)?;
/// ```
pub trait ApplyOps: Iterator + Sized {
    /// Execute every op in sequence, returning the summed affected-row
    /// count. Without a transaction, a mid-op failure leaves prior ops committed.
    ///
    /// # Errors
    ///
    /// Returns the first [`DieselError`](diesel::result::Error) any op
    /// produces, and does not run the rest.
    fn apply<Conn>(self, conn: &mut Conn) -> QueryResult<usize>
    where
        Conn: diesel::Connection,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>;

    /// Same as [`Self::apply`] wrapped in a diesel
    /// [`Connection::transaction`](diesel::Connection::transaction), so a
    /// mid-op failure rolls the whole batch back.
    ///
    /// # Errors
    ///
    /// Returns the first [`DieselError`](diesel::result::Error) any op
    /// produces, after rollback.
    fn apply_transactional<Conn>(self, conn: &mut Conn) -> QueryResult<usize>
    where
        Conn: diesel::Connection,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>;
}

impl<I> ApplyOps for I
where
    I: Iterator + Sized,
{
    fn apply<Conn>(self, conn: &mut Conn) -> QueryResult<usize>
    where
        Conn: diesel::Connection,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>,
    {
        let mut total = 0_usize;
        for op in self {
            total = total.saturating_add(op.execute(conn)?);
        }
        Ok(total)
    }

    fn apply_transactional<Conn>(self, conn: &mut Conn) -> QueryResult<usize>
    where
        Conn: diesel::Connection,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + RunQueryDsl<Conn>,
    {
        conn.transaction(|conn| self.apply(conn))
    }
}
