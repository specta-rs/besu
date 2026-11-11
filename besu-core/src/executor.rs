use core::future::Future;

use crate::{Dialect, Driver, RowStream, Sql};

/// The dialect an [`Executor`] speaks.
pub type DialectOf<E> = <<E as Executor>::Driver as Driver>::Dialect;

/// The error type an [`Executor`] produces.
pub type ErrorOf<E> = <<E as Executor>::Driver as Driver>::Error;

/// The row type an [`Executor`] produces.
pub type RowOf<E> = <<E as Executor>::Driver as Driver>::Row;

/// Something a query can be run on.
///
/// This is the seam that makes transactions free: a query builder is generic over an
/// `Executor`, and both `&Database` and `&mut Transaction` implement it. Binding a builder
/// to an executor costs nothing because it is usually just a borrowed reference.
///
/// The dialect is not a separate parameter: it is a projection of the executor's driver, so
/// the two can never disagree.
pub trait Executor {
    /// The driver behind this executor.
    type Driver: Driver;

    /// What a statement that returns no rows reports.
    type Output: Send + 'static;

    /// Run a query and collect every row.
    fn query(
        &mut self,
        sql: &Sql<DialectOf<Self>>,
    ) -> impl Future<Output = Result<Vec<RowOf<Self>>, ErrorOf<Self>>> + Send;

    /// Run a statement that returns no rows.
    fn execute(
        &mut self,
        sql: &Sql<DialectOf<Self>>,
    ) -> impl Future<Output = Result<Self::Output, ErrorOf<Self>>> + Send;

    /// The prefix every table name in a statement carries, if any.
    ///
    /// A shared physical database with one set of tables per tenant needs it; everything
    /// else returns `None` and pays nothing for the capability.
    fn table_prefix(&self) -> Option<alloc::sync::Arc<str>> {
        None
    }

    /// Run a query and hand back its rows as they arrive.
    ///
    /// The borrow is `'a` on both `self` and the statement: a cursor keeps the connection and
    /// the SQL text alive for as long as it is open.
    fn query_stream<'a>(
        &'a mut self,
        sql: &'a Sql<DialectOf<Self>>,
    ) -> impl Future<Output = Result<RowStream<'a, Self::Driver>, ErrorOf<Self>>> + Send + 'a;
}

impl<Dr: Driver> Executor for &Dr {
    type Driver = Dr;
    type Output = Dr::Output;

    async fn query(&mut self, sql: &Sql<Dr::Dialect>) -> Result<Vec<Dr::Row>, Dr::Error> {
        let driver: &Dr = self;
        driver.query(sql).await
    }

    async fn execute(&mut self, sql: &Sql<Dr::Dialect>) -> Result<Dr::Output, Dr::Error> {
        let driver: &Dr = self;
        driver.execute(sql).await
    }

    fn query_stream<'a>(
        &'a mut self,
        sql: &'a Sql<Dr::Dialect>,
    ) -> impl Future<Output = Result<RowStream<'a, Dr>, Dr::Error>> + Send + 'a {
        let driver: &Dr = self;
        driver.query_stream(sql)
    }
}

/// An executor that builds SQL but refuses to run it.
///
/// This is how Besu supports driverless SQL generation: you get the full typed query
/// builder and an exact `to_sql()` without a database connection, and attempting to
/// actually execute returns an error rather than panicking.
pub struct Dry<D: Dialect>(core::marker::PhantomData<fn() -> D>);

impl<D: Dialect> Dry<D> {
    /// Construct a dry executor.
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

impl<D: Dialect> Default for Dry<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect> Clone for Dry<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: Dialect> Copy for Dry<D> {}

impl<D: Dialect> core::fmt::Debug for Dry<D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Dry").field("dialect", &D::NAME).finish()
    }
}

/// A row produced by a [`Dry`] executor. It is never populated.
#[derive(Debug, Clone, Copy)]
pub struct DryRow;

/// The driver behind a [`Dry`] executor.
pub struct DryDriver<D: Dialect>(core::marker::PhantomData<fn() -> D>);

impl<D: Dialect> Clone for DryDriver<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: Dialect> Copy for DryDriver<D> {}

impl<D: Dialect> core::fmt::Debug for DryDriver<D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DryDriver")
            .field("dialect", &D::NAME)
            .finish()
    }
}

impl<D: Dialect> DryDriver<D> {
    /// Construct a dry driver.
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

impl<D: Dialect> Default for DryDriver<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect> Driver for DryDriver<D> {
    type Dialect = D;
    type Error = crate::Error;
    type Row = DryRow;
    type Output = ();

    fn error(err: crate::Error) -> Self::Error {
        err
    }

    fn row_len(_: &Self::Row) -> usize {
        0
    }

    fn is_null(_: &Self::Row, _: usize) -> bool {
        true
    }

    async fn query(&self, _: &Sql<D>) -> Result<Vec<Self::Row>, Self::Error> {
        Err(crate::Error::unsupported(
            "this query was built with a dry executor and has no database attached; \
             call `to_sql` instead of executing it",
        ))
    }

    async fn execute(&self, _: &Sql<D>) -> Result<Self::Output, Self::Error> {
        Err(crate::Error::unsupported(
            "this query was built with a dry executor and has no database attached; \
             call `to_sql` instead of executing it",
        ))
    }
}

impl<D: Dialect> Executor for &Dry<D> {
    type Driver = DryDriver<D>;
    type Output = ();

    async fn query(
        &mut self,
        sql: &Sql<D>,
    ) -> Result<Vec<<DryDriver<D> as Driver>::Row>, crate::Error> {
        DryDriver::<D>(core::marker::PhantomData).query(sql).await
    }

    async fn execute(&mut self, sql: &Sql<D>) -> Result<(), crate::Error> {
        DryDriver::<D>(core::marker::PhantomData).execute(sql).await
    }

    async fn query_stream<'a>(
        &'a mut self,
        sql: &'a Sql<D>,
    ) -> Result<RowStream<'a, DryDriver<D>>, crate::Error> {
        DryDriver::<D>(core::marker::PhantomData)
            .query_stream(sql)
            .await
    }
}
