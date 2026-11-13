use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    ops::AsyncFnOnce,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, Dialect, DialectOf, Driver, DryDriver, Error, ErrorOf, Executor, Value};

use crate::expr::ExprKind;
use crate::macros::Fragment;
use crate::query::insert::InsertQuery;
use crate::query::{Delete, Insert, Select, Update};
use crate::relation::InsertChildren;
use crate::table::{Insertable, Table, TableRef};

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
    ///
    /// The source is anything implementing [`TableRef`](crate::TableRef): a declared table
    /// from `#[derive(Table)]`, a derived table from [`derived`](crate::derived), or the same
    /// table under a second name from [`alias`](crate::alias). Pass a derived table or an
    /// alias by reference when its columns are also needed:
    ///
    /// ```ignore
    /// let totals = derived(db.select(posts).pick(posts.author_id), "totals");
    /// let author = totals.column(posts.author_id);
    /// db.select(&totals).pick((author.clone(), count_all())).group_by(author)
    /// ```
    ///
    /// ```ignore
    /// let manager = alias(employees, "manager");
    /// db.select(&manager).filter(manager.column(employees.name).eq("Alice"))
    /// ```
    pub fn select<T: TableRef>(&self, table: T) -> Select<'_, &Self, (T::Marker,), T::Model> {
        Select::new(self, table.into_source())
    }

    /// Start a `SELECT` from a common table expression.
    ///
    /// The statement renders the expression's definition as a `WITH` clause and then reads
    /// the name, so one expression is defined exactly where it is used:
    ///
    /// ```ignore
    /// let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    /// let rows: Vec<User> = db.from(&adults).all().await?;
    /// ```
    ///
    /// ```sql
    /// WITH "adults" AS (SELECT ... FROM "users" WHERE ("users"."age" >= $1))
    /// SELECT "adults"."id", ... FROM "adults"
    /// ```
    ///
    /// See [`Select::cte`] for how the expression is built, and [`Cte`](crate::Cte) for how
    /// its columns are reached.
    pub fn from<R: 'static>(
        &self,
        cte: &crate::query::Cte<R>,
    ) -> Select<'_, &Self, (crate::query::Cte<R>,), R> {
        crate::query::cte::select_from(self, cte)
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
    /// The driver's own transaction handle.
    ///
    /// `pub(crate)` so [`Database::batch`](crate::Database::batch) can run its statements
    /// on the transaction it opened without duplicating the `Transaction` type.
    pub(crate) inner: Dr::Transaction<'a>,
    /// The number of savepoints opened through [`Transaction::transaction`], so each gets a
    /// name of its own.
    savepoints: u32,
    /// The table prefix the database carries, so a query built inside the transaction names
    /// the same physical tables a query on the database itself would.
    prefix: Option<alloc::sync::Arc<str>>,
}

impl<'a, Dr: TransactionDriver> Transaction<'a, Dr> {
    /// Start a `SELECT` inside the transaction.
    pub fn select<T: TableRef>(
        &mut self,
        table: T,
    ) -> Select<'_, &mut Self, (T::Marker,), T::Model> {
        Select::new(self, table.into_source())
    }

    /// Start a `SELECT` from a common table expression inside the transaction.
    ///
    /// See [`Database::from`] for what the expression does.
    pub fn from<R: 'static>(
        &mut self,
        cte: &crate::query::Cte<R>,
    ) -> Select<'_, &mut Self, (crate::query::Cte<R>,), R> {
        crate::query::cte::select_from(self, cte)
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

    fn table_prefix(&self) -> Option<alloc::sync::Arc<str>> {
        self.prefix.clone()
    }
}

impl<Dr: TransactionDriver> Database<Dr> {
    /// Begin a transaction.
    pub async fn begin(&self) -> Result<Transaction<'_, Dr>, Dr::Error> {
        Ok(Transaction {
            inner: Dr::begin(&self.driver).await?,
            savepoints: 0,
            prefix: self.prefix.clone(),
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

    /// Insert one parent row and its has-many children in one transaction.
    ///
    /// The parent is inserted first, with `RETURNING` every declared column, and decoded into
    /// `P::Model`. The parent key is read out of that model, and every child's foreign key is
    /// overwritten with it, so the value the caller put there is a placeholder and may be
    /// anything the column accepts. The children then go into one multi-row
    /// `INSERT ... RETURNING`, and both statements commit together:
    ///
    /// ```ignore
    /// let (user, posts): (User, Vec<Post>) = db
    ///     .insert_tree(
    ///         db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
    ///         has_many(users.id, posts.author_id),
    ///         [posts.insert("Hello", 0), posts.insert("World", 0)],
    ///     )
    ///     .await?;
    /// ```
    ///
    /// ```sql
    /// INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3)
    ///   RETURNING "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio";
    /// INSERT INTO "posts" ("title", "author_id") VALUES ($1, $2), ($3, $4)
    ///   RETURNING "posts"."id", "posts"."title", "posts"."author_id";
    /// ```
    ///
    /// The parent insert must carry exactly one row — `.value(..)` rather than
    /// `.values(..)` — because its result is the single parent model. A parent with no
    /// children returns `(parent, Vec::new())` after committing the parent insert.
    ///
    /// Each statement numbers its own placeholders from one, exactly as it would on its
    /// own: the child insert binds the parent key again for every row.
    ///
    /// `insert_tree` supplies the parent's `RETURNING` projection itself, projecting every
    /// declared column so the model decodes. An insert that already has a projection cannot
    /// be passed: [`Insert::returning`] returns a different type, so it is a compile error
    /// rather than a runtime surprise. Any `ON CONFLICT` clause is preserved; if it skips the
    /// row, there is no parent to return and the call fails.
    ///
    /// The relation must be a plain [`has_many`](crate::has_many): a nested relation, a
    /// relation with an order or limit, and a many-to-many have no
    /// [`InsertChildren`](crate::relation::InsertChildren) implementation. Use
    /// [`Database::transaction`](Self::transaction) for those.
    ///
    /// MySQL has no `RETURNING`, so this reports an
    /// [`Error::Unsupported`](besu_core::Error::Unsupported) before opening a transaction
    /// rather than guessing the parent key. A database without `RETURNING` cannot implement
    /// the contract without a second, driver-specific mechanism such as MySQL's
    /// `LAST_INSERT_ID()`.
    pub async fn insert_tree<'a, Rel, P, C, I>(
        &'a self,
        parent: Insert<'a, &'a Self, (P,), I>,
        relation: Rel,
        children: impl IntoIterator<Item = C::Insert>,
    ) -> Result<(P::Model, Vec<C::Model>), Dr::Error>
    where
        Rel: InsertChildren<Parent = P, Child = C>,
        P: Table,
        C: Table,
        I: Insertable,
        P::Model: Decode<Dr>,
        C::Model: Decode<Dr>,
    {
        if !<Dr::Dialect as Dialect>::SUPPORTS_RETURNING {
            return Err(<Dr as Driver>::error(Error::unsupported(alloc::format!(
                "`insert_tree` needs `RETURNING` to read the generated parent key, and `{}` \
                 does not support it; use `Database::transaction` and read the key from the \
                 driver instead",
                <Dr::Dialect as Dialect>::NAME
            ))));
        }

        let parent_table = relation.parent_table();
        let child_table = relation.child_table();
        let parent_key_column = relation.parent_key_column();
        let child_foreign_key = relation.child_foreign_key();

        let Insert {
            query: mut parent_query,
            ..
        } = parent;

        let stride = parent_query.stride();
        if stride == 0 {
            return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                "`insert_tree` needs a parent table with at least one insertable column, but \
                 `{}` has none",
                parent_table.name
            ))));
        }
        if parent_query.values.len() != stride {
            return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                "`insert_tree` inserts exactly one parent row, but this insert provides {}; \
                 call `.value(..)` once, or use `Database::transaction` for several parents",
                parent_query.values.len() / stride
            ))));
        }
        if parent_query.returning.is_some() {
            // Unreachable through the public API — `Insert::returning` consumes the insert and
            // returns a different type — but kept so a future builder change cannot silently
            // replace a caller's projection.
            return Err(<Dr as Driver>::error(Error::invalid_query(
                "`insert_tree` supplies the parent's `RETURNING` projection; pass the insert \
                 before `.returning(..)`, or use `Database::transaction`",
            )));
        }

        let parent_key_index = match parent_table
            .columns
            .iter()
            .position(|column| column.name == parent_key_column)
        {
            Some(index) => index,
            None => {
                return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                    "`insert_tree` cannot find the parent key column `{}.{parent_key_column}`",
                    parent_table.name
                ))));
            }
        };
        let child_columns = <C::Insert as Insertable>::COLUMNS;
        let child_foreign_key_index = match child_columns
            .iter()
            .position(|column| *column == child_foreign_key)
        {
            Some(index) => index,
            None => {
                return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                    "`insert_tree` cannot find the child foreign key column `{}.{child_foreign_key}`",
                    child_table.name
                ))));
            }
        };

        // The caller's values cannot be decoded without every column, and the model's decode
        // order is the declared column order. Replacing any projection with that order is
        // what makes the return type `P::Model` rather than a caller-chosen subset.
        parent_query.returning = Some(
            parent_table
                .columns
                .iter()
                .map(|column| ExprKind::Column {
                    table: parent_table.name,
                    column: column.name,
                })
                .collect(),
        );

        let prefix = self.prefix.clone();
        let parent_sql = parent_query
            .build::<Dr::Dialect>(prefix.clone())
            .finish()
            .map_err(<Dr as Driver>::error)?;

        // Every build error above happens before the transaction opens. From here on, any
        // failure rolls the transaction back rather than leaving a parent without its
        // children.
        let mut tx = self.begin().await?;
        let result = async {
            let rows = Dr::txn_query(&mut tx.inner, &parent_sql).await?;
            let Some(row) = rows.first() else {
                return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                    "`insert_tree` did not receive the inserted `{}` row; an `ON CONFLICT` \
                     clause that skips the row leaves nothing to return",
                    parent_table.name
                ))));
            };
            let mut offset = 0;
            let parent_model = P::Model::decode(row, &mut offset)?;
            drop(rows);

            let mut model_values = P::model_values(&parent_model);
            if parent_key_index >= model_values.len() {
                return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                    "`insert_tree` decoded a `{}` model with {} value(s) for {} column(s)",
                    parent_table.name,
                    model_values.len(),
                    parent_table.columns.len()
                ))));
            }
            let parent_key = model_values.swap_remove(parent_key_index);
            if parent_key == Value::Null {
                return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                    "`insert_tree` read a `NULL` from the parent key `{}.{parent_key_column}`",
                    parent_table.name
                ))));
            }

            // One flat buffer for the whole child statement, written a row at a time. The
            // foreign key is overwritten in place immediately after each row lands, so there
            // is no second pass and no per-row allocation.
            let child_stride = child_columns.len();
            let children = children.into_iter();
            let (child_rows, _) = children.size_hint();
            let mut child_values = Vec::with_capacity(child_rows.saturating_mul(child_stride));
            for child in children {
                let start = child_values.len();
                child.write_values(&mut child_values);
                if child_values.len() - start != child_stride {
                    return Err(<Dr as Driver>::error(Error::invalid_query(alloc::format!(
                        "an `Insertable` for `{}` wrote {} value(s) for {} column(s)",
                        child_table.name,
                        child_values.len() - start,
                        child_stride
                    ))));
                }
                child_values[start + child_foreign_key_index] = Some(parent_key.clone());
            }

            if child_values.is_empty() {
                return Ok((parent_model, Vec::new()));
            }

            let mut child_query = InsertQuery::new(child_table, child_columns);
            child_query.values = child_values;
            child_query.returning = Some(
                child_table
                    .columns
                    .iter()
                    .map(|column| ExprKind::Column {
                        table: child_table.name,
                        column: column.name,
                    })
                    .collect(),
            );
            let child_sql = child_query
                .build::<Dr::Dialect>(prefix)
                .finish()
                .map_err(<Dr as Driver>::error)?;

            let rows = Dr::txn_query(&mut tx.inner, &child_sql).await?;
            let mut decoded = Vec::with_capacity(rows.len());
            for row in &rows {
                let mut offset = 0;
                decoded.push(C::Model::decode(row, &mut offset)?);
            }

            Ok((parent_model, decoded))
        }
        .await;

        match result {
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
    let bytes = name.as_bytes();
    !name.is_empty()
        && name.len() <= 64
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::is_valid_savepoint_name;

    #[test]
    fn a_savepoint_name_may_start_with_an_underscore() {
        // The error message and the documentation both say letters, digits and underscores,
        // so an underscore is legal anywhere an identifier can start.
        assert!(is_valid_savepoint_name("_before_commit"));
        assert!(is_valid_savepoint_name("besu_savepoint_0"));
        assert!(!is_valid_savepoint_name("0_before"));
        assert!(!is_valid_savepoint_name("has space"));
        assert!(!is_valid_savepoint_name(""));
    }
}
