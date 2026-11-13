//! Running several statements as one unit.
//!
//! [`Database::batch`] takes a tuple of statements, runs them on one connection inside one
//! transaction, and hands back a tuple of what each one reported. It is the shorthand for the
//! common "these statements must all land, or none of them may" case: one `await`, one
//! `BEGIN`/`COMMIT`, one place to handle the error.
//!
//! # What can be in a batch
//!
//! A batch element is anything implementing [`BatchItem`]:
//!
//! - a write — [`Insert`], [`Update`] and [`Delete`] — reporting the driver's own output,
//!   usually the number of affected rows,
//! - a query — [`Select`], [`SetQuery`] and [`DynamicSelect`] — reporting its decoded rows as
//!   `Vec<R>`,
//! - a `returning` write — [`InsertReturning`], [`UpdateReturning`] and [`DeleteReturning`] —
//!   reporting the returned rows as `Vec<R>`.
//!
//! Because each element names its own output, a tuple can mix them:
//!
//! ```ignore
//! let (inserted, rows) = db
//!     .batch((
//!         db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
//!         // Runs after the insert, in the same transaction, so it sees the new row.
//!         db.select(users).filter(users.age.gte(18)),
//!     ))
//!     .await?;
//! ```
//!
//! # The honest limits
//!
//! A batch is **not** one round trip. No shipped driver here exposes PostgreSQL's pipeline
//! protocol, so the statements are sent one after another over the transaction's connection,
//! exactly as if you had awaited them in sequence yourself. What a batch buys is the
//! transaction and the typing, not fewer messages. If a round trip count matters, a multi-row
//! `INSERT` or a `VALUES` list is the tool; see [`Insert::values`].
//!
//! A batch is a **fixed-arity tuple**, not a list. That is a deliberate choice: the tuple is
//! what makes each element keep its own output type, so `db.batch((insert, select))` can
//! return `(Dr::Output, Vec<User>)` with nothing boxed or erased. A list of statements would
//! need one uniform output type for every element, which is exactly the information a mixed
//! read/write batch exists to preserve.
//!
//! # Atomicity
//!
//! Every statement runs inside a single transaction opened by [`Database::batch`]. If one
//! fails, the transaction is rolled back and the error is returned; nothing the batch wrote
//! stays, including the rows a select earlier in the tuple could have observed. If they all
//! succeed, the transaction commits. The statements run in the order the tuple lists them,
//! which matters when a later one reads a row an earlier one wrote.
//!
//! Statements are rendered before the transaction is opened, so a statement that cannot
//! produce valid SQL — an empty `INSERT`, a `RETURNING` clause on MySQL — fails without a
//! transaction ever being started or left open.

use core::future::Future;

use alloc::vec::Vec;

use besu_core::{Decode, Driver, Error, Executor, Sql, TransactionDriver};

use crate::Database;
use crate::query::select::decode_rows;
use crate::query::{
    Delete, DeleteReturning, DynamicSelect, Insert, InsertReturning, Select, SetQuery, Update,
    UpdateReturning,
};
use crate::table::Insertable;

/// A single statement that can take part in a [`Database::batch`].
///
/// This is the umbrella over every statement shape a batch accepts. A write implements
/// [`BatchWrite`] and is a `BatchItem` through the blanket impl below, so an `INSERT`,
/// `UPDATE` or `DELETE` needs no extra impl to be a batch element. A query and the
/// `RETURNING` builders implement this trait directly, because their output is decoded rows
/// rather than the driver's own output type.
///
/// The associated types are what make a heterogeneous tuple work: each element names its own
/// [`Output`](BatchItem::Output), so a tuple of a write and a select returns
/// `(Dr::Output, Vec<R>)` without either one being erased or boxed.
pub trait BatchItem<Dr: TransactionDriver>: Send {
    /// What running the statement reports.
    ///
    /// A write reports the driver's own output type; a query or a `RETURNING` write reports
    /// its decoded rows. The tuple parameter of [`Batch`] keeps them apart, element by
    /// element.
    type Output: Send;

    /// The statement's rendered form.
    ///
    /// A statement renders to a [`Sql`], but the associated type keeps [`Batch`] independent
    /// of that: a statement whose rendered form needs more than the SQL text can say so
    /// without changing either trait.
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
    /// [`BatchItem::render`] has run.
    fn run<'tx, 'a>(
        tx: &'tx mut Dr::Transaction<'a>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx;
}

/// A single write statement that can take part in a [`Database::batch`].
///
/// Implemented for [`Insert`], [`Update`] and [`Delete`]. The `Update` and `Delete` impls
/// only cover the safe form — the one that has a `WHERE` clause, or that explicitly opted
/// into every row with `all_rows` — so a batch cannot be used to sneak past the guard that
/// makes an accidental mass write a compile error.
///
/// A `returning` write is not part of this trait, because it reports rows rather than the
/// driver's output; it is a [`BatchItem`] directly, as are the query builders.
///
/// Every `BatchWrite` is a [`BatchItem`] through a blanket impl, which is why this trait keeps
/// its meaning from before queries joined a batch: a type that implements it can be a batch
/// element without knowing about [`BatchItem`].
///
/// The trait is sealed. That is what lets the blanket impl above and the direct [`BatchItem`]
/// impls for queries coexist: the compiler can only tell the two apart if no other crate can
/// add a `BatchWrite` impl for a query builder. A statement of a new kind — one whose output
/// is not the driver's own — implements [`BatchItem`] directly, which is what queries and the
/// `RETURNING` builders do.
pub trait BatchWrite<Dr: TransactionDriver>: sealed::Sealed + Send {
    /// What running the statement reports, which is the driver's own output type.
    type Output: Send;

    /// The statement's rendered form.
    type Rendered: Send + 'static;

    /// Render the statement without running it.
    fn render(&self) -> Result<Self::Rendered, Error>;

    /// Run a rendered statement on `tx`.
    fn run<'tx, 'a>(
        tx: &'tx mut Dr::Transaction<'a>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx;
}

/// Sealing for [`BatchWrite`].
///
/// A private supertrait is what makes the blanket [`BatchItem`] impl and the direct impls for
/// queries provably disjoint: without it, another crate could implement `BatchWrite` for
/// `Select<..>` and the two impls would overlap. The trait is `pub` inside a private module so
/// it is nameable in this file and nowhere else.
mod sealed {
    pub trait Sealed {}
}

/// Every write is a batch item.
///
/// A batch is defined over [`BatchItem`] so that a query can sit next to a write in the same
/// tuple. Writes predate that trait and keep their own, narrower one, so this impl is what
/// keeps every existing `INSERT`, `UPDATE` and `DELETE` — and the way each one is written
/// today — usable as a batch element unchanged.
impl<Dr, T> BatchItem<Dr> for T
where
    Dr: TransactionDriver,
    T: BatchWrite<Dr>,
{
    type Output = <T as BatchWrite<Dr>>::Output;
    type Rendered = <T as BatchWrite<Dr>>::Rendered;

    fn render(&self) -> Result<Self::Rendered, Error> {
        <T as BatchWrite<Dr>>::render(self)
    }

    fn run<'tx, 'a>(
        tx: &'tx mut Dr::Transaction<'a>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, Dr::Error>> + Send + 'tx {
        <T as BatchWrite<Dr>>::run(tx, sql)
    }
}

/// A tuple of statements that [`Database::batch`] can run as one unit.
///
/// Implemented for tuples of one to eight elements. One element is legal and is simply an
/// atomic single statement. The tuple is fixed-arity on purpose: each element keeps its own
/// output type, which a list of statements cannot do without erasing it.
pub trait Batch<Dr: TransactionDriver>: Send {
    /// The tuple of per-statement outputs, in the order the statements were listed.
    type Output: Send;

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

/// Run one rendered query statement on a transaction and decode every row into `R`.
///
/// Shared by [`Select`], [`SetQuery`] and the three `RETURNING` builders, so a batch decodes a
/// query exactly the way awaiting it on its own does. One `Vec<R>` holds the result; the
/// driver's rows are borrowed for the decode and dropped, never copied.
#[allow(clippy::manual_async_fn)]
fn run_query<'tx, 'a, Dr, R>(
    tx: &'tx mut Dr::Transaction<'a>,
    sql: Sql<Dr::Dialect>,
) -> impl Future<Output = Result<Vec<R>, Dr::Error>> + Send + 'tx
where
    Dr: TransactionDriver,
    R: Decode<Dr> + Send + 'static,
{
    async move {
        let rows = Dr::txn_query(tx, &sql).await?;
        decode_rows::<Dr, R>(&rows)
    }
}

impl<'a, E: Executor, M, I> sealed::Sealed for Insert<'a, E, M, I> {}

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

impl<'a, E: Executor, T> sealed::Sealed for Update<'a, E, T, true> {}

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

impl<'a, E: Executor, T> sealed::Sealed for Delete<'a, E, T, true> {}

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

// The query impls below decode into `Vec<R>`, and require `R: 'static` rather than the
// builder's `'a`. `run`'s future is bounded by `'tx`, the borrow of the transaction, which the
// caller chooses and which may outlive the builder, so a row type that borrowed from the
// batch could not be returned from it. `Decode` only produces owned values — `R::decode` has
// no lifetime tying it to the row it reads — so the bound costs nothing in practice.
impl<'a, E, M, R> BatchItem<E::Driver> for Select<'a, E, M, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    M: 'a,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

impl<'a, E, M, R> BatchItem<E::Driver> for SetQuery<'a, E, M, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    M: 'a,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

impl<'a, E, R> BatchItem<E::Driver> for DynamicSelect<'a, E, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

impl<'a, E, M, I, R> BatchItem<E::Driver> for InsertReturning<'a, E, M, I, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    M: 'a,
    I: 'a,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

impl<'a, E, T, R> BatchItem<E::Driver> for UpdateReturning<'a, E, T, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    T: 'a,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

impl<'a, E, T, R> BatchItem<E::Driver> for DeleteReturning<'a, E, T, R>
where
    E: Executor + Send,
    E::Driver: TransactionDriver,
    T: 'a,
    R: Decode<E::Driver> + Send + 'static,
{
    type Output = Vec<R>;
    type Rendered = Sql<<E::Driver as Driver>::Dialect>;

    fn render(&self) -> Result<Self::Rendered, Error> {
        self.to_sql()?.finish()
    }

    fn run<'tx, 'a2>(
        tx: &'tx mut <E::Driver as TransactionDriver>::Transaction<'a2>,
        sql: Self::Rendered,
    ) -> impl Future<Output = Result<Self::Output, <E::Driver as Driver>::Error>> + Send + 'tx {
        run_query::<E::Driver, R>(tx, sql)
    }
}

macro_rules! impl_batch {
    ($(($ty:ident, $value:ident, $index:tt)),+ $(,)?) => {
        impl<Dr, $($ty),+> Batch<Dr> for ($($ty,)+)
        where
            Dr: TransactionDriver,
            $($ty: BatchItem<Dr>),+
        {
            type Output = ($($ty::Output,)+);
            type Rendered = ($(<$ty as BatchItem<Dr>>::Rendered,)+);

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
    /// Run several statements in one transaction and return their outputs.
    ///
    /// ```ignore
    /// let (first, second, rows) = db
    ///     .batch((
    ///         db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
    ///         db.insert(users).value(users.insert("b@example.com", "Bob", 31)),
    ///         db.select(users).filter(users.age.gte(30)),
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
    /// # Reading and writing in one batch
    ///
    /// A [`Select`], [`SetQuery`] or [`DynamicSelect`] may be an element. Its output is the
    /// same `Vec<R>` that awaiting it on its own produces, decoded when its turn comes, so a
    /// select after a write sees what that write wrote. A `returning` write works the same
    /// way. Because each element keeps its own output type, the returned tuple can mix them:
    /// `(Dr::Output, Vec<User>)` above.
    ///
    /// # One `await`, not one round trip
    ///
    /// The statements are sent one after another: no shipped driver exposes PostgreSQL's
    /// pipeline protocol, so this is not fewer round trips than awaiting each statement. It is
    /// one `await`, one transaction and a typed tuple, and that is all it claims.
    ///
    /// # Fixed arity
    ///
    /// The batch is a tuple of one to eight statements, which is what lets each element name
    /// its own output type. A dynamically built list of statements would need every element
    /// to report the same thing, so it is not this API; build the tuple you need, or use
    /// [`Database::transaction`](crate::Database::transaction).
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
