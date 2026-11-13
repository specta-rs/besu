use core::{future::Future, pin::Pin};

use futures_core::Stream;

use crate::{Dialect, Error, Sql};

/// A stream of rows, borrowed from the statement and the connection it was started on.
///
/// Streaming has to borrow rather than own: sqlx needs the SQL text to outlive the cursor, and
/// a transaction needs to stay borrowed for as long as the cursor is open. The lifetime is why
/// [`Sql`] is passed by reference.
pub type RowStream<'a, D> =
    Pin<Box<dyn Stream<Item = Result<<D as Driver>::Row, <D as Driver>::Error>> + Send + 'a>>;

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

    /// The names of a row's columns, in the order this driver decodes them.
    ///
    /// A result set's names are a property of the response rather than the statement: only
    /// the server knows what `SELECT *` expanded to, and only it can name an expression the
    /// query did not alias. A driver that can read them off a row should; the default is
    /// `None` so a hand-written driver that cannot report names is not forced to invent
    /// them. Callers should treat `None` as "there is nothing to display" rather than as an
    /// error.
    fn column_names(_row: &Self::Row) -> Option<Vec<String>> {
        None
    }

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

    /// Run a query and hand back its rows as they arrive.
    ///
    /// The default collects through [`Driver::query`], which keeps a hand-written driver
    /// simple; a driver that can genuinely stream should override it. Every shipped driver
    /// does.
    fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<RowStream<'a, Self>, Self::Error>> + Send {
        async move {
            let rows = self
                .query(sql)
                .await?
                .into_iter()
                .map(Ok)
                .collect::<Vec<_>>();
            Ok(Box::pin(futures_stream::iter(rows)) as RowStream<'a, Self>)
        }
    }
}

/// The smallest possible `Stream` over a `Vec`, so the default [`Driver::query_stream`] does
/// not need a dependency on `futures-util`.
///
/// This is public because a driver that cannot genuinely stream — and a test fake — needs
/// exactly this.
pub mod futures_stream {
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use futures_core::Stream;

    /// A `Stream` over the items of a `Vec`.
    pub struct Iter<T> {
        items: std::vec::IntoIter<T>,
    }

    /// Build the stream.
    pub fn iter<T>(items: Vec<T>) -> Iter<T> {
        Iter {
            items: items.into_iter(),
        }
    }

    impl<T> Unpin for Iter<T> {}

    impl<T> Stream for Iter<T> {
        type Item = T;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
            Poll::Ready(self.get_mut().items.next())
        }
    }
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

    /// Run a query inside a transaction and stream its rows.
    fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Self::Dialect>,
    ) -> impl Future<Output = Result<RowStream<'c, Self>, Self::Error>> + Send;
}
