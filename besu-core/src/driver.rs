use core::future::Future;

use crate::{Dialect, Error, Sql};

/// A driver implements the database execution logic for a specific database.
///
/// Drivers are intentionally tiny: everything about *what* SQL to run lives in the
/// [`Dialect`], so a driver is only responsible for getting bytes to the server and rows
/// back. That split is what lets Besu build queries with no driver at all.
pub trait Driver: Clone + Send + Sync + 'static {
    /// The dialect this driver speaks.
    type Dialect: Dialect;

    /// The driver's error type. Every failure, including decode and encode failures, shows
    /// up here so callers only ever deal with one error type.
    type Error: core::error::Error + Send + Sync + 'static;

    /// A row in the result set.
    type Row: Send + 'static;

    /// What a statement that returns no rows reports, usually the number of affected rows.
    type Output: Send + 'static;

    /// Convert a core error into the driver's error type.
    fn error(err: Error) -> Self::Error;

    /// The number of values in a row.
    fn row_len(row: &Self::Row) -> usize;

    /// Whether the value at `index` is `NULL`.
    ///
    /// This is used to decode `Option<T>` rows coming out of outer joins, so it must not
    /// fail: an out-of-range index reports `true`.
    fn is_null(row: &Self::Row, index: usize) -> bool;

    /// Run a query and collect every row.
    fn query(
        &self,
        sql: &Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<Vec<Self::Row>, Self::Error>> + Send;

    /// Run a statement that returns no rows.
    fn execute(
        &self,
        sql: &Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}

/// A driver that can run queries inside a transaction.
///
/// Transactions are a separate trait because not every driver can offer them (an HTTP-based
/// database cannot, for instance), and because implementing one is a real amount of work
/// that a read-only driver should not be forced into.
pub trait TransactionDriver: Driver {
    /// The driver's underlying transaction handle.
    type Transaction<'a>: Send + 'a
    where
        Self: 'a;

    /// Start a transaction.
    fn begin<'a>(
        &'a self,
    ) -> impl Future<Output = Result<Self::Transaction<'a>, Self::Error>> + Send;

    /// Commit, consuming the transaction.
    fn commit<'a>(
        tx: Self::Transaction<'a>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Roll back, consuming the transaction.
    fn rollback<'a>(
        tx: Self::Transaction<'a>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Run a query inside a transaction.
    fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<Vec<Self::Row>, Self::Error>> + Send;

    /// Run a statement inside a transaction.
    fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}
