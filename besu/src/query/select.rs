use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::types::Bool;
use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor};

use crate::expr::{Expr, IntoCondition, IntoOrder};
use crate::query::{JoinClause, JoinKind, SelectQuery};
use crate::selection::Selection;
use crate::table::Table;

/// A `SELECT` under construction.
///
/// The type parameters are all inferred: `M` is the set of tables in the query (used to
/// reject references to tables you did not join) and `R` is the row type the query returns.
pub struct Select<'a, E: Executor, M, R> {
    pub(crate) exec: E,
    pub(crate) query: SelectQuery,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> (M, R))>,
}

impl<'a, E: Executor, M, R> Select<'a, E, M, R> {
    pub(crate) fn new(exec: E, table: &'static besu_core::TableDef) -> Self {
        Self {
            exec,
            query: SelectQuery::new(table),
            phantom: PhantomData,
        }
    }

    /// Add a `WHERE` condition. Repeated calls are combined with `AND`.
    pub fn filter<F, P>(mut self, condition: F) -> Self
    where
        F: IntoCondition<M, P>,
    {
        let _ = PhantomData::<P>;
        self.query.filter.push(condition.into_condition());
        self
    }

    /// Add an `ORDER BY` term.
    pub fn order_by<O: IntoOrder>(mut self, order: O) -> Self {
        self.query.order_by.push(order.into_order());
        self
    }

    /// Add a descending `ORDER BY` term.
    pub fn order_by_desc<O: IntoOrder>(mut self, order: O) -> Self {
        self.query.order_by.push(order.into_order().reverse());
        self
    }

    /// Add a `GROUP BY` expression.
    pub fn group_by<O: IntoOrder>(mut self, expr: O) -> Self {
        self.query.group_by.push(expr.into_order().expr);
        self
    }

    /// Add a `HAVING` condition.
    pub fn having<F, P>(mut self, condition: F) -> Self
    where
        F: IntoCondition<M, P>,
    {
        let _ = PhantomData::<P>;
        self.query.having.push(condition.into_condition());
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

    /// `SELECT DISTINCT`.
    pub fn distinct(mut self) -> Self {
        self.query.distinct = true;
        self
    }

    /// Choose exactly what to return.
    ///
    /// The selection's type becomes the row type, so `pick((user.id, user.name))` yields
    /// `Vec<(i64, String)>` and `pick(count_all())` yields `Vec<i64>`.
    pub fn pick<S, P>(mut self, selection: S) -> Select<'a, E, M, S::Out>
    where
        S: Selection<M, P>,
    {
        let _ = PhantomData::<P>;
        let (exprs, _) = selection.into_projection();
        self.query.projection = Some(exprs);
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// Render the exact SQL this query will run, without running it.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.build())
    }

    /// Render the SQL with its parameters inlined, for logging.
    pub fn inspect(&self) -> Result<alloc::string::String, Error> {
        Ok(alloc::string::ToString::to_string(
            &self.query.build::<DialectOf<E>>().inspect(),
        ))
    }

    fn into_parts(self) -> (E, SelectQuery) {
        (self.exec, self.query)
    }
}

impl<'a, E: Executor, M, R> Select<'a, E, M, R> {
    /// An `INNER JOIN`. Rows must match on both sides.
    pub fn inner_join<T: Table>(
        mut self,
        _table: T,
        on: Expr<(M, T), Bool>,
    ) -> Select<'a, E, (M, T), (R, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Inner,
            table: T::DEF,
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `LEFT JOIN`. The joined row becomes `Option<T>`, `None` when nothing matched.
    pub fn left_join<T: Table>(
        mut self,
        _table: T,
        on: Expr<(M, Option<T>), Bool>,
    ) -> Select<'a, E, (M, Option<T>), (R, Option<T::Model>)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Left,
            table: T::DEF,
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `RIGHT JOIN`.
    pub fn right_join<T: Table>(
        mut self,
        _table: T,
        on: Expr<(M, T), Bool>,
    ) -> Select<'a, E, (M, T), (Option<R>, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Right,
            table: T::DEF,
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `FULL OUTER JOIN`. Not every dialect supports it; the query fails at build time if
    /// yours does not.
    pub fn full_join<T: Table>(
        mut self,
        _table: T,
        on: Expr<(M, Option<T>), Bool>,
    ) -> Select<'a, E, (M, Option<T>), (Option<R>, Option<T::Model>)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Full,
            table: T::DEF,
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `CROSS JOIN`, which pairs every row with every row.
    pub fn cross_join<T: Table>(mut self, _table: T) -> Select<'a, E, (M, T), (R, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Cross,
            table: T::DEF,
            on: None,
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }
}

impl<'a, E: Executor, M, R> Select<'a, E, M, R>
where
    R: Decode<E::Driver>,
{
    /// Run the query and return every row.
    pub async fn all(self) -> Result<alloc::vec::Vec<R>, ErrorOf<E>> {
        let (mut exec, query) = self.into_parts();
        let sql = query
            .build::<DialectOf<E>>()
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        decode_rows::<E, R>(&rows)
    }

    /// Run the query and return the first row, if any.
    pub async fn first(mut self) -> Result<Option<R>, ErrorOf<E>> {
        self.query.limit = Some(1);
        let (mut exec, query) = self.into_parts();
        let sql = query
            .build::<DialectOf<E>>()
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        let mut decoded = decode_rows::<E, R>(&rows)?;
        Ok(decoded.pop())
    }
}

fn decode_rows<'a, E: Executor, R: Decode<E::Driver>>(
    rows: &'a [<E::Driver as Driver>::Row],
) -> Result<alloc::vec::Vec<R>, ErrorOf<E>> {
    let mut out = alloc::vec::Vec::with_capacity(rows.len());
    for row in rows {
        let mut offset = 0;
        out.push(R::decode(row, &mut offset)?);
    }
    Ok(out)
}

impl<'a, E, M, R> IntoFuture for Select<'a, E, M, R>
where
    E: Executor + Send + 'a,
    M: 'a,
    R: Decode<E::Driver> + 'a,
{
    type Output = Result<alloc::vec::Vec<R>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.all())
    }
}
