//! Running several write statements as one unit.
//!
//! [`Database::batch`] takes a tuple of [`Insert`], [`Update`] and [`Delete`] statements,
//! runs them on one connection inside one transaction, and hands back a tuple of what each
//! one reported. It is the shorthand for the common "these writes must all land, or none of
//! them may" case: one `await`, one `BEGIN`/`COMMIT`, one place to handle the error.
//!
//! # The honest limits
//!
//! A batch is **not** one round trip. sqlx has no pipeline API, so the statements are sent
//! one after another over the transaction's connection, exactly as if you had awaited them
//! in sequence yourself. What a batch buys is the transaction and the typing, not fewer
//! messages. If a round trip count matters, a multi-row `INSERT` or a `VALUES` list is the
//! tool; see [`Insert::values`].
//!
//! A batch holds **writes only** for now. A batched `SELECT` needs the statement's row type
//! to survive the erasure that a heterogeneous list implies, which is a separate design;
//! see [`BatchWrite`] for what that would take.
//!
//! # Atomicity
//!
//! Every statement runs inside a single transaction opened by [`Database::batch`]. If one
//! fails, the transaction is rolled back and the error is returned; nothing the batch wrote
//! stays. If they all succeed, the transaction commits. The statements run in the order the
//! tuple lists them, which matters when a later one reads a row an earlier one wrote.
//!
//! Statements are rendered before the transaction is opened, so a statement that cannot
//! produce valid SQL — an empty `INSERT`, a `RETURNING` clause on MySQL — fails without a
//! transaction ever being started or left open.

use core::future::Future;

use besu_core::{Driver, Error, Executor, Sql, TransactionDriver};

use crate::Database;
use crate::query::{Delete, Insert, Update};
use crate::table::Insertable;

/// A single write statement that can take part in a [`Database::batch`].
///
/// Implemented for [`Insert`], [`Update`] and [`Delete`]. The `Update` and `Delete` impls
/// only cover the safe form — the one that has a `WHERE` clause, or that explicitly opted
/// into every row with `all_rows` — so a batch cannot be used to sneak past the guard that
/// makes an accidental mass write a compile error.
///
/// A statement returning rows is deliberately not part of this trait. `returning(..)` turns
/// an `INSERT` into a different builder, and a batch of selects would need each element to
/// name its own decoded row type; the tuple parameter of [`Batch`] can only do that once a
/// unified statement trait exists. The write half does not need it, and is useful on its
/// own.
pub trait BatchWrite<Dr: TransactionDriver>: Send {
    /// What running the statement reports, which is the driver's own output type.
    type Output: Send + 'static;

    /// The statement's rendered form.
    ///
    /// A write renders to a [`Sql`], but the associated type keeps [`Batch`] independent of
    /// that: a statement whose rendered form needs more than the SQL text can say so without
    /// changing either trait.
    type Rendered: Send + 'static;

    /// Render the statement without running it.
    ///
    /// This is the same rendering the builder does when it is awaited on its own, so the
    /// text a batch sends is the text `to_sql` shows.
    fn render(&self) -> Result<Self::Rendered, Error>;

    /// Run a rendered statement on `tx`.
    ///
    /// This is an associated function rather than a method because running the statement
    /// needs only the rendered SQL: the builder has already written everything it knew into
    /// the text and its bound parameters, so the builder is no longer needed once
    /// [`BatchWrite::render`] has run.
    fn run<'tx, 'a>(
        tx: &'tx mut Dr::Transaction<'a>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx;
}

/// A tuple of write statements that [`Database::batch`] can run as one unit.
///
/// Implemented for tuples of one to eight elements. One element is legal and is simply an
/// atomic single statement.
pub trait Batch<Dr: TransactionDriver>: Send {
    /// The tuple of per-statement outputs, in the order the statements were listed.
    type Output: Send + 'static;

    /// The rendered statements, one per element.
    ///
    /// Rendering is a separate step so that a build error can be reported before a
    /// transaction exists. It is an associated type rather than a `Vec` so a statement's
    /// rendered form can change later without changing this trait.
    type Rendered: Send;

    /// Render every statement in the tuple.
    fn render(&self) -> Result<Self::Rendered, Error>;

    /// Run the rendered statements on `tx`, in tuple order.
    fn run<'tx, 'a>(
        self,
        tx: &'tx mut Dr::Transaction<'a>,
        rendered: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx;
}

/// Run one rendered write statement on a transaction.
///
/// Shared by the three statement impls so they cannot drift apart on how a statement is
/// sent; the only thing that differs between them is how the SQL was rendered.
///
/// Written with an explicit `impl Future` rather than `async fn` so the returned future is
/// bounded by `'tx`, the borrow of the transaction. Clippy's `async fn` rewrite loses that
/// bound: the hidden future would also capture the transaction's own `'a`, which the caller
/// does not promise to keep that short.
#[allow(clippy::manual_async_fn)]
fn run_write<'tx, 'a, Dr: TransactionDriver>(
    tx: &'tx mut Dr::Transaction<'a>,
    sql: Sql<Dr::Dialect>,
) -> impl Future<Output = Result<Dr::Output, Dr::Error>> + Send + 'tx {
    async move { Dr::txn_execute(tx, &sql).await }
}

impl<'a, E, M, I> BatchWrite<E::Driver> for Insert<'a, E, M, I>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    M: 'a,
    I: Insertable + 'a,
{
    type Output = <E::Driver as Driver>::Output;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_write::<E::Driver>(tx, sql)
    }
}

impl<'a, E, T> BatchWrite<E::Driver> for Update<'a, E, T, true>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    T: 'a,
{
    type Output = <E::Driver as Driver>::Output;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_write::<E::Driver>(tx, sql)
    }
}

impl<'a, E, T> BatchWrite<E::Driver> for Delete<'a, E, T, true>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    T: 'a,
{
    type Output = <E::Driver as Driver>::Output;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_write::<E::Driver>(tx, sql)
    }
}

macro_rules! impl_batch {
    ($(($ty:ident, $value:ident, $index:tt)),+ $(,)?) => {
        impl<Dr, $($ty),+> Batch<Dr> for ($($ty,)+)
        where
            Dr: TransactionDriver,
            $($ty: BatchWrite<Dr>),+
        {
            type Output = ($($ty::Output,)+);
            type Rendered = ($(<$ty as BatchWrite<Dr>>::Rendered,)+);

            fn render(&self) -> Result<Self::Rendered, Error> {
                Ok(($(self.$index.render()?,)+))
            }

            fn run<'tx, 'a>(
                self,
                tx: &'tx mut Dr::Transaction<'a>,
                rendered: Self::Rendered,
            ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx {
                let ($($value,)+) = rendered;
                async move {
                    // Sequential, not concurrent: the transaction holds one connection, and
                    // a later statement may read what an earlier one wrote. Reborrowing
                    // `tx` for each call keeps the borrow checker content without a second
                    // handle to the connection.
                    $(let $value = $ty::run(&mut *tx, $value).await?;)+
                    Ok(($($value,)+))
                }
            }
        }
    };
}

impl_batch!((A, a, 0));
impl_batch!((A, a, 0), (B, b, 1));
impl_batch!((A, a, 0), (B, b, 1), (C, c, 2));
impl_batch!((A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3));
impl_batch!((A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4));
impl_batch!(
    (A, a, 0),
    (B, b, 1),
    (C, c, 2),
    (D, d, 3),
    (E, e, 4),
    (F, f, 5)
);
impl_batch!(
    (A, a, 0),
    (B, b, 1),
    (C, c, 2),
    (D, d, 3),
    (E, e, 4),
    (F, f, 5),
    (G, g, 6)
);
impl_batch!(
    (A, a, 0),
    (B, b, 1),
    (C, c, 2),
    (D, d, 3),
    (E, e, 4),
    (F, f, 5),
    (G, g, 6),
    (H, h, 7)
);

impl<Dr: TransactionDriver> Database<Dr> {
    /// Run several write statements in one transaction and return their outputs.
    ///
    /// ```ignore
    /// let (first, second, third) = db
    ///     .batch((
    ///         db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
    ///         db.insert(users).value(users.insert("b@example.com", "Bob", 31)),
    ///         db.update(users).set(users.age, 40).filter(users.email.eq("a@example.com")),
    ///     ))
    ///     .await?;
    /// ```
    ///
    /// Every statement runs inside one transaction on one connection, in the order the
    /// tuple lists them, and the transaction commits only once all of them have succeeded.
    /// A failure rolls the whole batch back, so the rows the earlier statements wrote are
    /// gone too; the error is returned unchanged, as `Dr::Error`, so the caller handles it
    /// with the same type it already handles for every other statement.
    ///
    /// The statements are rendered before the transaction is opened. A builder that cannot
    /// produce valid SQL therefore reports its [`Error`] without a transaction ever being
    /// started, and with nothing to leave open.
    ///
    /// # One `await`, not one round trip
    ///
    /// The statements are sent one after another: sqlx exposes no pipeline, so this is not
    /// fewer round trips than awaiting each statement. It is one `await`, one transaction
    /// and a typed tuple, and that is all it claims.
    ///
    /// # Writes only
    ///
    /// A batched `SELECT` is not supported yet. See [`BatchWrite`] for what it would need.
    ///
    /// # Errors
    ///
    /// Returns the first statement's error, after rolling back. A commit failure is
    /// returned too, though at that point the database decides whether the work landed; the
    /// same is true of [`Transaction::commit`](crate::Transaction::commit).
    pub async fn batch<B: Batch<Dr>>(&self, batch: B) -> Result<B::Output, Dr::Error> {
        // Render first, so a build error cannot leave a transaction behind. `render` is
        // cheap relative to a round trip and only borrows the builders, so the same
        // builders can still be run by `batch.run` below.
        let rendered = batch.render().map_err(<Dr as Driver>::error)?;

        let mut tx = self.begin().await?;
        match batch.run(&mut tx.inner, rendered).await {
            Ok(output) => {
                tx.commit().await?;
                Ok(output)
            }
            Err(err) => {
                // Best effort: the statement's error is what the caller needs, and a
                // rollback failure has nowhere better to go.
                let _ = tx.rollback().await;
                Err(err)
            }
        }
    }
}
