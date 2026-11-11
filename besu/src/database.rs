use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    ops::AsyncFnOnce,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, Dialect, DialectOf, Driver, DryDriver, ErrorOf, Executor};

use crate::macros::Fragment;
use crate::query::{Delete, Insert, Select, Update};
use crate::table::Table;

/// A database you can run queries against.
///
/// `Database` only ever holds the driver: the SQL is built by the query builders, which are
/// generic over an executor. That split is why the same builder works against a pool, a
/// transaction and a database with no connection at all.
pub struct Database<Dr: Driver> {
    driver: Dr,
}

impl<Dr: Driver> Database<Dr> {
    /// Wrap a driver.
    pub fn new(driver: Dr) -> Self {
        Self { driver }
    }

    /// A reference to the underlying driver, for anything Besu does not cover.
    pub fn driver(&self) -> &Dr {
        &self.driver
    }

    /// Start a `SELECT` over `table`.
    pub fn select<T: Table>(&self, _table: T) -> Select<'_, &Self, (T,), T::Model> {
        Select::new(&*self, T::DEF)
    }

    /// Start an `INSERT` into `table`.
    pub fn insert<T: Table>(&self, _table: T) -> Insert<'_, &Self, (T,), T::Insert> {
        Insert::new(&*self, T::DEF)
    }

    /// Start an `UPDATE` on `table`.
    ///
    /// The returned builder cannot be awaited until a `WHERE` clause is added, or until you
    /// explicitly call `all_rows`.
    pub fn update<T: Table>(&self, _table: T) -> Update<'_, &Self, T, false> {
        Update::new(&*self, T::DEF)
    }

    /// Start a `DELETE` on `table`.
    ///
    /// The returned builder cannot be awaited until a `WHERE` clause is added, or until you
    /// explicitly call `all_rows`.
    pub fn delete<T: Table>(&self, _table: T) -> Delete<'_, &Self, T, false> {
        Delete::new(&*self, T::DEF)
    }

    /// Run a raw, parameterised statement.
    ///
    /// The fragment's `?` placeholders are rewritten to the dialect's own spelling, so the
    /// same call works against every backend.
    pub fn execute(&self, fragment: Fragment) -> RawQuery<'_, &Self> {
        RawQuery {
            exec: &*self,
            fragment,
            phantom: PhantomData,
        }
    }
}

impl<Dr: Driver> core::fmt::Debug for Database<Dr> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Database")
            .field("driver", &core::any::type_name::<Dr>())
            .field("dialect", &Dr::Dialect::NAME)
            .finish()
    }
}

impl<Dr: Driver> Executor for &Database<Dr> {
    type Driver = Dr;
    type Output = Dr::Output;

    async fn query(
        &mut self,
        sql: &besu_core::Sql<Dr::Dialect>,
    ) -> Result<Vec<Dr::Row>, Dr::Error> {
        self.driver.query(sql).await
    }

    async fn execute(
        &mut self,
        sql: &besu_core::Sql<Dr::Dialect>,
    ) -> Result<Dr::Output, Dr::Error> {
        self.driver.execute(sql).await
    }
}

use besu_core::TransactionDriver;

/// A database that builds SQL but refuses to run it.
///
/// This is how driverless SQL generation works: you get the full typed builder and an exact
/// `to_sql()`, and attempting to execute returns an error instead of panicking.
pub fn dry<D: Dialect>() -> Database<DryDriver<D>> {
    Database::new(DryDriver::new())
}

/// A transaction.
///
/// Borrowed from a [`Database`](Database), it runs every query on the same connection and
/// is committed once you ask it to be. Dropping it without committing rolls back.
pub struct Transaction<'a, Dr: TransactionDriver> {
    inner: Dr::Transaction<'a>,
}

impl<'a, Dr: TransactionDriver> Transaction<'a, Dr> {
    /// Start a `SELECT` inside the transaction.
    pub fn select<T: Table>(&mut self, _table: T) -> Select<'_, &mut Self, (T,), T::Model> {
        Select::new(self, T::DEF)
    }

    /// Start an `INSERT` inside the transaction.
    pub fn insert<T: Table>(&mut self, _table: T) -> Insert<'_, &mut Self, (T,), T::Insert> {
        Insert::new(self, T::DEF)
    }

    /// Start an `UPDATE` inside the transaction.
    pub fn update<T: Table>(&mut self, _table: T) -> Update<'_, &mut Self, T, false> {
        Update::new(self, T::DEF)
    }

    /// Start a `DELETE` inside the transaction.
    pub fn delete<T: Table>(&mut self, _table: T) -> Delete<'_, &mut Self, T, false> {
        Delete::new(self, T::DEF)
    }

    /// Run a raw, parameterised statement inside the transaction.
    pub fn execute(&mut self, fragment: Fragment) -> RawQuery<'_, &mut Self> {
        RawQuery {
            exec: self,
            fragment,
            phantom: PhantomData,
        }
    }

    /// Commit the transaction.
    pub async fn commit(self) -> Result<(), Dr::Error> {
        Dr::commit(self.inner).await
    }

    /// Roll the transaction back.
    pub async fn rollback(self) -> Result<(), Dr::Error> {
        Dr::rollback(self.inner).await
    }
}

impl<'a, Dr: TransactionDriver> Executor for &mut Transaction<'a, Dr> {
    type Driver = Dr;
    type Output = Dr::Output;

    async fn query(
        &mut self,
        sql: &besu_core::Sql<Dr::Dialect>,
    ) -> Result<Vec<Dr::Row>, Dr::Error> {
        Dr::txn_query(&mut self.inner, sql).await
    }

    async fn execute(
        &mut self,
        sql: &besu_core::Sql<Dr::Dialect>,
    ) -> Result<Dr::Output, Dr::Error> {
        Dr::txn_execute(&mut self.inner, sql).await
    }
}

impl<Dr: TransactionDriver> Database<Dr> {
    /// Begin a transaction.
    pub async fn begin(&self) -> Result<Transaction<'_, Dr>, Dr::Error> {
        Ok(Transaction {
            inner: Dr::begin(&self.driver).await?,
        })
    }

    /// Run a closure inside a transaction, committing on success and rolling back on error.
    ///
    /// ```ignore
    /// db.transaction(|tx| async move {
    ///     tx.insert(user).value(user.insert(..)).await?;
    ///     tx.update(user).set(user.age, 22).filter(user.id.eq(1)).await?;
    ///     Ok(())
    /// })
    /// .await?;
    /// ```
    pub async fn transaction<T, F>(&self, f: F) -> Result<T, Dr::Error>
    where
        F: AsyncFnOnce(&mut Transaction<'_, Dr>) -> Result<T, Dr::Error>,
    {
        let mut tx = self.begin().await?;
        match f(&mut tx).await {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(err)
            }
        }
    }
}

/// A raw SQL statement under construction.
pub struct RawQuery<'a, E: Executor> {
    exec: E,
    fragment: Fragment,
    phantom: PhantomData<&'a ()>,
}

impl<'a, E: Executor> RawQuery<'a, E> {
    /// Render the exact SQL this query will run.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, besu_core::Error> {
        Ok(self.fragment.render())
    }

    /// Run the statement and decode every row into `R`.
    pub async fn all<R: Decode<E::Driver>>(self) -> Result<Vec<R>, ErrorOf<E>> {
        let mut exec = self.exec;
        let sql = self
            .fragment
            .render::<DialectOf<E>>()
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut offset = 0;
            out.push(R::decode(row, &mut offset)?);
        }
        Ok(out)
    }

    /// Run the statement and return the first row, if any.
    pub async fn first<R: Decode<E::Driver>>(self) -> Result<Option<R>, ErrorOf<E>> {
        let mut rows = self.all::<R>().await?;
        Ok(rows.pop())
    }
}

impl<'a, E: Executor + Send + 'a> IntoFuture for RawQuery<'a, E> {
    type Output = Result<E::Output, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let mut exec = self.exec;
            let sql = self
                .fragment
                .render::<DialectOf<E>>()
                .finish()
                .map_err(<E::Driver as Driver>::error)?;
            exec.execute(&sql).await
        })
    }
}
