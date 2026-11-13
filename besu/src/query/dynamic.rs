//! The escape hatch for the builder's row-set type-state.
//!
//! A [`DynamicSelect`] is a [`Select`] with the row-set parameter `M` erased from the
//! clauses. It still carries the row type `R`, so the statement continues to decode into the
//! shape `pick` chose, but `filter`, `group_by`, `having`, `order_by` and `pick` take the
//! erased [`ExprKind`](crate::ExprKind) / [`OrderClause`] instead of asking the compiler to
//! prove the expression belongs to the query.
//!
//! This is not the default way to build a query. The normal builder checks that a column
//! comes from a table the statement really selects from, and that comparisons line up on
//! both types; that check is the reason the query layer exists. Reach for `dynamic` when the
//! shape of the query is chosen at runtime, in a way the type-state cannot follow — a list of
//! conditions built in a loop, a filter handed in by another function, a clause that is
//! present only sometimes.
//!
//! ```ignore
//! let mut query = db.select(users).dynamic();
//! for condition in conditions {
//!     query = query.filter(condition.into_kind());
//! }
//! let rows: Vec<User> = query
//!     .order_by(users.name.asc())
//!     .limit(50)
//!     .all()
//!     .await?;
//! ```
//!
//! Everything the builder would have checked is the database's problem from here: a column
//! from a table that is not in `FROM`, a comparison between incompatible types, and a
//! projection whose columns do not match `R` all reach the server. Use
//! [`DynamicSelect::into_select`] to hand the query back to the typed builder once the
//! dynamic part is done; the row-set parameter starts empty, so a later `filter` needs a
//! `correlate` to name anything.

use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor};

use crate::expr::{ExprKind, OrderClause};
use crate::query::{Select, SelectQuery, select::decode_rows};

/// A [`Select`] with the row-set type-state erased from its clauses.
///
/// Created by [`Select::dynamic`]. The typed builder is the default; this handle is the
/// escape hatch for the cases where its row-set type-state is in the way, and the database
/// becomes the check for the clauses added through it.
///
/// `R` is the row type the query decodes to. It does not change when a clause is added:
/// [`DynamicSelect::pick`] and [`DynamicSelect::pick_many`] replace the projection but leave
/// `R` alone, because the point of the handle is that the caller already knows what the
/// statement returns.
pub struct DynamicSelect<'a, E: Executor, R> {
    pub(crate) exec: E,
    pub(crate) query: SelectQuery,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> R)>,
}

impl<'a, E: Executor, R> DynamicSelect<'a, E, R> {
    pub(crate) fn new(exec: E, query: SelectQuery) -> Self {
        Self {
            exec,
            query,
            phantom: PhantomData,
        }
    }

    /// Add a `WHERE` condition that is already erased. Repeated calls are combined with
    /// `AND`.
    ///
    /// The condition is not checked to be a `Bool`; a non-boolean expression is the
    /// database's error.
    pub fn filter(mut self, condition: ExprKind) -> Self {
        self.query.filter.push(condition);
        self
    }

    /// Add a `HAVING` condition that is already erased.
    pub fn having(mut self, condition: ExprKind) -> Self {
        self.query.having.push(condition);
        self
    }

    /// Add a `GROUP BY` expression that is already erased.
    pub fn group_by(mut self, expr: ExprKind) -> Self {
        self.query.group_by.push(expr);
        self
    }

    /// Add an `ORDER BY` term that is already erased.
    pub fn order_by(mut self, order: OrderClause) -> Self {
        self.query.order_by.push(order);
        self
    }

    /// Add a descending `ORDER BY` term that is already erased.
    pub fn order_by_desc(mut self, order: OrderClause) -> Self {
        self.query.order_by.push(order.reverse());
        self
    }

    /// Replace the projection with one erased expression.
    ///
    /// The row type `R` is left alone: it is the caller's statement that one column decodes
    /// as `R`. Use [`DynamicSelect::into_select`] and `pick` if the result needs the
    /// compiler's check instead.
    pub fn pick(mut self, selection: ExprKind) -> Self {
        self.query.projection = Some(alloc::vec![selection]);
        self
    }

    /// Replace the projection with a list of erased expressions, in order.
    ///
    /// The row type `R` is left alone, for the same reason as [`DynamicSelect::pick`].
    pub fn pick_many(mut self, projection: alloc::vec::Vec<ExprKind>) -> Self {
        self.query.projection = Some(projection);
        self
    }

    /// `SELECT DISTINCT`.
    pub fn distinct(mut self) -> Self {
        self.query.distinct = true;
        self
    }

    /// `LIMIT n`.
    pub fn limit(mut self, limit: i64) -> Self {
        self.query.limit = Some(limit);
        self
    }

    /// `OFFSET n`.
    pub fn offset(mut self, offset: i64) -> Self {
        self.query.offset = Some(offset);
        self
    }

    /// Label the statement with a comment, rendered as a trailing `/* ... */`.
    ///
    /// See [`Select::comment`] for what the label is for and how an embedded `*/` is written.
    pub fn comment(mut self, comment: impl Into<alloc::string::String>) -> Self {
        self.query.comment = Some(comment.into());
        self
    }

    /// Hand the query back to the typed builder.
    ///
    /// The row-set parameter starts empty, so filters and joins added afterwards cannot name
    /// any table without [`correlate`](Select::correlate); the clauses that were added while
    /// dynamic stay as they are.
    pub fn into_select(self) -> Select<'a, E, (), R> {
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// Render the exact SQL this query will run, without running it.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.clone().build(self.exec.table_prefix()))
    }

    /// Render the SQL with its parameters inlined, for logging.
    pub fn inspect(&self) -> Result<alloc::string::String, Error> {
        Ok(alloc::string::ToString::to_string(
            &self
                .query
                .clone()
                .build::<DialectOf<E>>(self.exec.table_prefix())
                .inspect(),
        ))
    }
}

impl<'a, E: Executor, R> DynamicSelect<'a, E, R>
where
    R: Decode<E::Driver>,
{
    /// Run the query and return every row.
    pub async fn all(self) -> Result<alloc::vec::Vec<R>, ErrorOf<E>> {
        let prefix = self.exec.table_prefix();
        let mut exec = self.exec;
        let sql = self
            .query
            .build::<DialectOf<E>>(prefix)
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        decode_rows::<E, R>(&rows)
    }

    /// Run the query and return the first row, if any.
    ///
    /// This sets `limit(1)`, replacing any limit already set, exactly as [`Select::first`]
    /// does.
    pub async fn first(mut self) -> Result<Option<R>, ErrorOf<E>> {
        self.query.limit = Some(1);
        let mut rows = self.all().await?;
        Ok(rows.pop())
    }
}

impl<'a, E, R> IntoFuture for DynamicSelect<'a, E, R>
where
    E: Executor + Send + 'a,
    R: Decode<E::Driver> + 'a,
{
    type Output = Result<alloc::vec::Vec<R>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.all())
    }
}
