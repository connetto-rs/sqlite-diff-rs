//! `diesel-async` feature: batch-apply changeset and patchset ops against an
//! async Diesel connection.
//!
//! This adds only an execution driver on top of the `diesel` feature. Each op
//! already renders as a [`QueryFragment`], and `diesel-async`'s
//! [`ExecuteDsl`](diesel_async::methods::ExecuteDsl) blanket-implements async
//! execution for any `QueryFragment + QueryId + Send`. So the async path reuses
//! the exact same rendering as the synchronous
//! [`ApplyOps`](super::diesel_query::ApplyOps) trait, differing only in awaiting
//! each op and using [`AsyncConnection::transaction`] for the atomic variant.

use diesel::query_builder::{QueryFragment, QueryId};
use diesel::result::{Error as DieselError, QueryResult};
use diesel_async::AsyncConnection;
use diesel_async::methods::ExecuteDsl;

/// Async counterpart of [`ApplyOps`](super::diesel_query::ApplyOps): run every
/// op in an iterator against a [`diesel_async::AsyncConnection`].
///
/// Works uniformly on the naive path (`set.iter()`) and the adapter path
/// (`set.iter().map(|op| op.with_adapter::<DB, _>(&adapter))`), for both
/// patchsets and changesets, exactly like the synchronous trait.
///
/// The generic wrappers below compile only if both methods exist with the
/// intended bounds and return `Send` futures, so they double as a
/// backend-agnostic compile check for the async apply surface:
///
/// ```
/// use diesel::query_builder::{QueryFragment, QueryId};
/// use diesel::result::QueryResult;
/// use diesel_async::AsyncConnection;
/// use sqlite_diff_rs::ApplyOpsAsync;
///
/// fn apply<Conn, I>(ops: I, conn: &mut Conn) -> impl Future<Output = QueryResult<usize>> + Send
/// where
///     Conn: AsyncConnection,
///     I: Iterator + Send,
///     I::Item: QueryFragment<Conn::Backend> + QueryId + Send,
/// {
///     ops.apply_async(conn)
/// }
///
/// fn apply_tx<'a, Conn, I>(
///     ops: I,
///     conn: &'a mut Conn,
/// ) -> impl Future<Output = QueryResult<usize>> + Send + 'a
/// where
///     Conn: AsyncConnection,
///     I: Iterator + Send + 'a,
///     I::Item: QueryFragment<Conn::Backend> + QueryId + Send,
/// {
///     ops.apply_transactional_async(conn)
/// }
/// ```
pub trait ApplyOpsAsync: Iterator + Sized {
    /// Execute every op in sequence, returning the summed affected-row count.
    /// Without a transaction, a mid-op failure leaves prior ops committed.
    ///
    /// # Errors
    ///
    /// Returns the first [`DieselError`](diesel::result::Error) any op
    /// produces, and does not run the rest.
    fn apply_async<Conn>(self, conn: &mut Conn) -> impl Future<Output = QueryResult<usize>> + Send
    where
        Conn: AsyncConnection,
        Self: Send,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + Send;

    /// Same as [`Self::apply_async`] wrapped in an
    /// [`AsyncConnection::transaction`], so a mid-op failure rolls the whole
    /// batch back.
    ///
    /// # Errors
    ///
    /// Returns the first [`DieselError`](diesel::result::Error) any op
    /// produces, after rollback.
    fn apply_transactional_async<'a, Conn>(
        self,
        conn: &'a mut Conn,
    ) -> impl Future<Output = QueryResult<usize>> + Send + 'a
    where
        Conn: AsyncConnection,
        Self: Send + 'a,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + Send;
}

impl<I> ApplyOpsAsync for I
where
    I: Iterator + Sized,
{
    async fn apply_async<Conn>(self, conn: &mut Conn) -> QueryResult<usize>
    where
        Conn: AsyncConnection,
        Self: Send,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + Send,
    {
        let mut total = 0_usize;
        for op in self {
            total = total.saturating_add(ExecuteDsl::execute(op, conn).await?);
        }
        Ok(total)
    }

    fn apply_transactional_async<'a, Conn>(
        self,
        conn: &'a mut Conn,
    ) -> impl Future<Output = QueryResult<usize>> + Send + 'a
    where
        Conn: AsyncConnection,
        Self: Send + 'a,
        Self::Item: QueryFragment<Conn::Backend> + QueryId + Send,
    {
        conn.transaction::<usize, DieselError, _>(async move |conn| self.apply_async(conn).await)
    }
}
