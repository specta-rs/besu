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
    prefix: Option<alloc::sync::Arc<str>>,
}

impl<Dr: Driver> Database<Dr> {
    /// Wrap a driver.
    pub fn new(driver: Dr) -> Self {
        Self {
            driver,
            prefix: None,
        }
    }

    /// Wrap a driver and prefix every table name it mentions.
    ///
    /// This is for deployments that share one physical database between tenants and name
    /// their tables `tenant_42_users`. The prefix is applied when a statement is rendered —
    /// `FROM`, `JOIN`, and every qualified column reference — and it is applied to the
    /// declared and introspected schemas by `push`, so migrations see the same names the
    /// queries do.
    ///
    /// Hand-written migration SQL is not rewritten, because it is not built by Besu; write it
    /// with the prefix already in place.
    pub fn prefixed(driver: Dr, prefix: impl Into<alloc::sync::Arc<str>>) -> Self {
        Self {
            driver,
            prefix: Some(prefix.into()),
        }
    }

    /// The prefix applied to table names, if any.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// A reference to the underlying driver, for anything Besu does not cover.
    pub fn driver(&self) -> &Dr {
        &self.driver
    }

    /// Start a `SELECT` over `table`.
    pub fn select<T: Table>(&self, _table: T) -> Select<'_, &Self, (T,), T::Model> {
        Select::new(self, T::DEF)
    }

    /// Start an `INSERT` into `table`.
    pub fn insert<T: Table>(&self, _table: T) -> Insert<'_, &Self, (T,), T::Insert> {
        Insert::new(self, T::DEF)
    }

    /// Start an `UPDATE` on `table`.
    ///
    /// The returned builder cannot be awaited until a `WHERE` clause is added, or until you
    /// explicitly call `all_rows`.
    pub fn update<T: Table>(&self, _table: T) -> Update<'_, &Self, T, false> {
        Update::new(self, T::DEF)
    }

    /// Start a `DELETE` on `table`.
    ///
    /// The returned builder cannot be awaited until a `WHERE` clause is added, or until you
    /// explicitly call `all_rows`.
    pub fn delete<T: Table>(&self, _table: T) -> Delete<'_, &Self, T, false> {
        Delete::new(self, T::DEF)
    }

    /// Run a query and hand back its rows as they arrive.
    ///
    /// The statement is passed in rather than built here because a cursor borrows the SQL
    /// text until it closes, and a value cannot borrow its own temporary:
    ///
    /// ```ignore
    /// let sql = db.select(users).to_sql()?;
    /// let mut rows = db.stream::<User>(&sql).await?;
    /// while let Some(user) = rows.next().await { .. }
    /// ```
    pub async fn stream<'a, R: Decode<Dr>>(
        &'a self,
        sql: &'a besu_core::Sql<Dr::Dialect>,
    ) -> Result<crate::Rows<'a, Dr, R>, Dr::Error> {
        let inner = self.driver.query_stream(sql).await?;
        Ok(crate::Rows {
            inner,
            phantom: PhantomData,
        })
    }

    /// Run a raw, parameterised statement.
    ///
    /// The fragment's `?` placeholders are rewritten to the dialect's own spelling, so the
    /// same call works against every backend.
    pub fn execute(&self, fragment: Fragment) -> RawQuery<'_, &Self> {
        RawQuery {
            exec: self,
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

    fn query_stream<'c>(
        &'c mut self,
        sql: &'c besu_core::Sql<Dr::Dialect>,
    ) -> impl core::future::Future<Output = Result<besu_core::RowStream<'c, Dr>, Dr::Error>> + Send + 'c
    {
        let driver: &'c Dr = &self.driver;
        driver.query_stream(sql)
    }

    fn table_prefix(&self) -> Option<alloc::sync::Arc<str>> {
        self.prefix.clone()
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
    /// The number of savepoints opened through [`Transaction::transaction`], so each gets a
    /// name of its own.
    savepoints: u32,
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

    /// Run a query inside the transaction and hand back its rows as they arrive.
    pub async fn stream<'c, R: Decode<Dr>>(
        &'c mut self,
        sql: &'c besu_core::Sql<Dr::Dialect>,
    ) -> Result<crate::Rows<'c, Dr, R>, Dr::Error> {
        let inner = Dr::txn_query_stream(&mut self.inner, sql).await?;
        Ok(crate::Rows {
            inner,
            phantom: PhantomData,
        })
    }

    /// Run a raw, parameterised statement inside the transaction.
    pub fn execute(&mut self, fragment: Fragment) -> RawQuery<'_, &mut Self> {
        RawQuery {
            exec: self,
            fragment,
            phantom: PhantomData,
        }
    }

    /// Run a closure inside a savepoint, releasing it on success and rolling back to it on
    /// error.
    ///
    /// The closure receives the same transaction, so nothing about the type changes: a
    /// savepoint is a named point in the transaction, not a different handle.
    ///
    /// ```ignore
    /// db.transaction(async |tx| {
    ///     tx.insert(users).value(..).await?;
    ///     // A failure here leaves the insert above in place.
    ///     let _ = tx.transaction(async |tx| {
    ///         tx.insert(audit).value(..).await?;
    ///         Err::<(), _>(error)
    ///     }).await;
    ///     Ok(())
    /// })
    /// .await?;
    /// ```
    pub async fn transaction<T, F>(&mut self, f: F) -> Result<T, Dr::Error>
    where
        F: AsyncFnOnce(&mut Self) -> Result<T, Dr::Error>,
    {
        let name = alloc::format!("besu_savepoint_{}", self.savepoints);
        self.savepoints = self.savepoints.wrapping_add(1);
        self.savepoint(&name, f).await
    }

    /// Run a closure inside a savepoint with a name you choose.
    ///
    /// The name is used verbatim in `SAVEPOINT`, `RELEASE SAVEPOINT` and `ROLLBACK TO
    /// SAVEPOINT`, so it is restricted to ASCII letters, digits and underscores. Anything else
    /// is an [`Error::InvalidQuery`](besu_core::Error::InvalidQuery) before a statement runs.
    ///
    /// A savepoint that is neither released nor rolled back is released when the enclosing
    /// transaction commits, which is what you want, so there is no guard to forget to drop.
    pub async fn savepoint<T, F>(&mut self, name: &str, f: F) -> Result<T, Dr::Error>
    where
        F: AsyncFnOnce(&mut Self) -> Result<T, Dr::Error>,
    {
        if !is_valid_savepoint_name(name) {
            return Err(<Dr as Driver>::error(besu_core::Error::invalid_query(
                alloc::format!(
                    "`{name}` is not a valid savepoint name; use ASCII letters, digits and \
                     underscores"
                ),
            )));
        }

        self.run_control(alloc::format!("SAVEPOINT {name}")).await?;
        match f(self).await {
            Ok(value) => {
                self.run_control(alloc::format!("RELEASE SAVEPOINT {name}"))
                    .await?;
                Ok(value)
            }
            Err(err) => {
                // Best effort: the original error is what the caller cares about.
                let _ = self
                    .run_control(alloc::format!("ROLLBACK TO SAVEPOINT {name}"))
                    .await;
                Err(err)
            }
        }
    }

    async fn run_control(&mut self, statement: alloc::string::String) -> Result<(), Dr::Error> {
        let sql = besu_core::Sql::<Dr::Dialect>::raw(statement)
            .finish()
            .map_err(<Dr as Driver>::error)?;
        Dr::txn_execute(&mut self.inner, &sql).await.map(|_| ())
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

    fn query_stream<'c>(
        &'c mut self,
        sql: &'c besu_core::Sql<Dr::Dialect>,
    ) -> impl core::future::Future<Output = Result<besu_core::RowStream<'c, Dr>, Dr::Error>> + Send + 'c
    {
        Dr::txn_query_stream(&mut self.inner, sql)
    }
}

impl<Dr: TransactionDriver> Database<Dr> {
    /// Begin a transaction.
    pub async fn begin(&self) -> Result<Transaction<'_, Dr>, Dr::Error> {
        Ok(Transaction {
            inner: Dr::begin(&self.driver).await?,
            savepoints: 0,
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
        // `swap_remove(0)` is O(1) and returns the first row, unlike `pop`.
        Ok(if rows.is_empty() {
            None
        } else {
            Some(rows.swap_remove(0))
        })
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

/// Savepoint names are written into SQL, so they are checked rather than escaped.
///
/// Quoting an identifier is not enough here: `SAVEPOINT` takes a name, not a string, and not
/// every dialect accepts a quoted one. Restricting the character set is the honest fix.
fn is_valid_savepoint_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
