//! Streaming a result set.
//!
//! [`Rows`] is what [`Database::stream`](crate::Database::stream) and
//! [`Transaction::stream`](crate::Transaction::stream) hand back. It decodes one row at a time
//! as the driver delivers it, so a million-row scan is a million rows of *work* rather than a
//! million rows of memory.
//!
//! ```ignore
//! let sql = db.select(users).filter(users.age.gt(18)).to_sql()?;
//! let mut rows = db.stream::<User>(&sql).await?;
//! while let Some(user) = rows.next().await {
//!     let user = user?;
//!     println!("{}", user.name);
//! }
//! ```
//!
//! The statement is passed in rather than built inside `stream` because a cursor borrows the
//! SQL text for as long as it is open, and a value cannot borrow a temporary it owns.

use core::{
    future::poll_fn,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use alloc::vec::Vec;

use besu_core::{Decode, Driver, RowStream, Stream};

/// A result set being delivered one row at a time.
pub struct Rows<'a, Dr: Driver, R> {
    pub(crate) inner: RowStream<'a, Dr>,
    pub(crate) phantom: PhantomData<fn() -> R>,
}

impl<Dr: Driver, R> core::fmt::Debug for Rows<'_, Dr, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rows").finish_non_exhaustive()
    }
}

impl<Dr: Driver, R: Decode<Dr>> Rows<'_, Dr, R> {
    /// The next row, or `None` when the result set is exhausted.
    ///
    /// A decode failure is reported as `Some(Err(..))` and the stream stays usable, so one bad
    /// row does not force you to abandon the rest.
    pub async fn next(&mut self) -> Option<Result<R, Dr::Error>> {
        poll_fn(|cx| match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(row))) => {
                let mut offset = 0;
                Poll::Ready(Some(R::decode(&row, &mut offset)))
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        })
        .await
    }

    /// Collect the rest of the stream into a `Vec`.
    ///
    /// This throws away the memory advantage, so it is for small result sets and tests.
    pub async fn collect(mut self) -> Result<Vec<R>, Dr::Error> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await {
            out.push(row?);
        }
        Ok(out)
    }
}

impl<Dr: Driver, R: Decode<Dr>> Stream for Rows<'_, Dr, R> {
    type Item = Result<R, Dr::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(row))) => {
                let mut offset = 0;
                Poll::Ready(Some(R::decode(&row, &mut offset)))
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
