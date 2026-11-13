use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::borrow::Cow;

use crate::types::Bool;
use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor};

use crate::expr::{Expr, IntoCondition, IntoOrder};
use crate::query::{
    Cte, CteColumns, DerivedSource, DynamicSelect, From, IntoQueryBody, JoinClause, JoinKind,
    QueryBody, SelectQuery, SetOp, SetQuery, validate_cte,
};
use crate::selection::Selection;
use crate::table::{Table, TableRef};

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
    pub(crate) fn new(exec: E, source: crate::query::From) -> Self {
        Self {
            exec,
            query: SelectQuery::from_source(source),
            phantom: PhantomData,
        }
    }

    /// Define `cte` before this statement, so the statement may refer to it.
    ///
    /// A statement that reads from the expression, or joins it, already carries its
    /// definition; calling this is harmless then, because an entry is kept only once per
    /// name. It is the way to define an expression the statement only mentions in a
    /// subquery.
    ///
    /// This is `with_cte` rather than `with` because [`Select::with`] already means eager
    /// loading a relation, and Rust has no overloading.
    ///
    /// ```ignore
    /// let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    /// let rows: Vec<User> = db
    ///     .select(posts)
    ///     .with_cte(&adults)
    ///     .filter(exists(db.from(&adults).correlate(posts).filter(..)))
    ///     .all()
    ///     .await?;
    /// ```
    pub fn with_cte<C: 'static>(mut self, cte: &Cte<C>) -> Self {
        self.query.push_with(cte.entries());
        self
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

    /// Label the statement with a comment, rendered as a trailing `/* ... */`.
    ///
    /// The label carries no meaning to the database, so it is only there for a human: a
    /// slow-query log, a `pg_stat_statements` row or a `Logged` event names the code that
    /// ran the statement instead of showing an anonymous `SELECT`.
    ///
    /// ```ignore
    /// db.select(users).comment("feed page").all().await?;
    /// ```
    ///
    /// A `*/` inside the label is written as `* /`, because otherwise the label would end the
    /// comment early and the rest of it would be parsed as SQL.
    pub fn comment(mut self, comment: impl Into<alloc::string::String>) -> Self {
        self.query.comment = Some(comment.into());
        self
    }

    /// `FOR UPDATE`: lock the selected rows until the transaction ends.
    pub fn for_update(mut self) -> Self {
        self.query.lock = Some(crate::query::Lock {
            strength: crate::query::LockStrength::Update,
            skip_locked: false,
            no_wait: false,
        });
        self
    }

    /// `FOR SHARE`: stop others writing to the selected rows until the transaction ends.
    pub fn for_share(mut self) -> Self {
        self.query.lock = Some(crate::query::Lock {
            strength: crate::query::LockStrength::Share,
            skip_locked: false,
            no_wait: false,
        });
        self
    }

    /// Add `SKIP LOCKED` to the row lock, for queue-style workloads.
    pub fn skip_locked(mut self) -> Self {
        if let Some(lock) = &mut self.query.lock {
            lock.skip_locked = true;
        }
        self
    }

    /// Add `NOWAIT` to the row lock, failing instead of waiting.
    pub fn no_wait(mut self) -> Self {
        if let Some(lock) = &mut self.query.lock {
            lock.no_wait = true;
        }
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

    fn into_parts(self) -> (E, SelectQuery) {
        (self.exec, self.query)
    }
}

impl<'a, E: Executor, M, R> Select<'a, E, M, R> {
    /// Add a table to the row set without selecting from it.
    ///
    /// This is how a correlated subquery reaches the enclosing query's columns. The table
    /// joins the row set, so `users.id` compiles inside the subquery, but no `FROM` entry is
    /// added, so the subquery still only reads its own tables:
    ///
    /// ```ignore
    /// db.select(posts)
    ///     .correlate(users)
    ///     .filter(posts.author_id.eq(users.id))
    /// ```
    ///
    /// Use it only inside a subquery. On its own it names a table the statement does not
    /// select from, which the database will reject as an unknown column. The reverse check —
    /// that the enclosing query really does contain `users` — cannot be made here, because
    /// the enclosing query's type is not in scope; see [`IntoSubquery`](crate::IntoSubquery).
    pub fn correlate<T: TableRef>(self, _table: T) -> Select<'a, E, (M, T::Marker), R> {
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// `UNION`: every row from both sides, with duplicates removed.
    ///
    /// The other side must return the same row type `R`, so two projections that decode
    /// differently cannot be combined. It may come from a different executor as long as the
    /// driver — and therefore the dialect — is the same.
    ///
    /// ```compile_fail
    /// use besu::prelude::*;
    ///
    /// #[derive(Table)]
    /// #[besu(table = "users")]
    /// struct User {
    ///     id: i64,
    /// }
    ///
    /// #[derive(Table)]
    /// #[besu(table = "posts")]
    /// struct Post {
    ///     id: i64,
    /// }
    ///
    /// # #[derive(Debug, Clone, Copy)]
    /// # struct Test;
    /// # impl besu::Dialect for Test {
    /// #     type Value = besu::Value;
    /// #     const NAME: &'static str = "test";
    /// #     const PLACEHOLDER: besu::besu_core::Placeholder =
    /// #         besu::besu_core::Placeholder::Question;
    /// #     const SUPPORTS_RETURNING: bool = true;
    /// #     const SUPPORTS_ILIKE: bool = false;
    /// #     const SUPPORTS_FULL_JOIN: bool = true;
    /// #     const UPSERT: besu::besu_core::UpsertStyle =
    /// #         besu::besu_core::UpsertStyle::OnConflict;
    /// #     fn bind(value: besu::Value) -> Result<Self::Value, besu::Error> {
    /// #         Ok(value)
    /// #     }
    /// #     fn write_placeholder(out: &mut String, _: usize) {
    /// #         out.push('?');
    /// #     }
    /// #     fn type_name(_: besu::TypeDef) -> std::borrow::Cow<'static, str> {
    /// #         "TEXT".into()
    /// #     }
    /// # }
    /// let db = besu::dry::<Test>();
    /// // `User` and `Post` are different row types, so the two sides cannot be combined.
    /// let _ = db.select(users).union(db.select(posts));
    /// ```
    pub fn union<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::Union, other.query)
    }

    /// `UNION ALL`: every row from both sides, keeping duplicates.
    pub fn union_all<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::UnionAll, other.query)
    }

    /// `INTERSECT`: rows that both sides contain, once.
    pub fn intersect<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::Intersect, other.query)
    }

    /// `INTERSECT ALL`: rows that both sides contain, keeping the smaller count.
    ///
    /// SQLite has no such spelling, so the query fails while building.
    pub fn intersect_all<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::IntersectAll, other.query)
    }

    /// `EXCEPT`: rows from the left that the right does not contain.
    pub fn except<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::Except, other.query)
    }

    /// `EXCEPT ALL`: rows from the left minus the right, by count.
    ///
    /// SQLite has no such spelling, so the query fails while building.
    pub fn except_all<'a2, E2, M2>(self, other: Select<'a2, E2, M2, R>) -> SetQuery<'a, E, M, R>
    where
        E2: Executor<Driver = E::Driver>,
    {
        SetQuery::new(self.exec, self.query, SetOp::ExceptAll, other.query)
    }
}

impl<'a, E: Executor, T: Table> Select<'a, E, (T,), T::Model> {
    /// Name this query as a common table expression.
    ///
    /// The returned handle can be selected from with
    /// [`Database::from`](crate::Database::from), joined with
    /// [`inner_join_cte`](Self::inner_join_cte) and [`left_join_cte`](Self::left_join_cte),
    /// and extended into a recursive definition with [`Cte::union_all`](crate::Cte::union_all).
    /// The statement that uses it renders `WITH "name" AS (...)` itself, so the expression is
    /// defined exactly where it is used and never runs on its own.
    ///
    /// ```ignore
    /// let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    /// let rows: Vec<User> = db.from(&adults).filter(adults.column(users.age).lt(65)).all().await?;
    /// ```
    ///
    /// ```sql
    /// WITH "adults" AS (SELECT ... FROM "users" WHERE ("users"."age" >= $1))
    /// SELECT "adults"."id", ... FROM "adults" WHERE ("adults"."age" < $2)
    /// ```
    ///
    /// This form is for a whole-row `SELECT` over one table. That is what lets the handle know
    /// its columns — they are the source table's — and therefore what lets
    /// [`Cte::column`](crate::Cte::column) type-check against the schema. A query that
    /// projects a subset, an aggregate or a join uses [`Select::cte_named`] instead, which
    /// takes the output names and a type witness for each.
    ///
    /// The name is used verbatim inside double quotes, so it may be any identifier, but it
    /// is written into the SQL rather than bound; passing a value from another source would
    /// be an injection.
    pub fn cte(self, name: &'static str) -> Cte<T::Model> {
        let mut query = self.query;
        let prerequisites = query.with.take();
        Cte::new(
            name,
            QueryBody::Select(query),
            Cow::Borrowed(T::DEF.columns),
            false,
            prerequisites,
        )
    }
}

impl<'a, E: Executor, M, R: 'static> Select<'a, E, M, R> {
    /// Name this query as a common table expression with a chosen column list.
    ///
    /// Unlike [`Select::cte`], this works for any projection: an aggregate, a join, a subset
    /// of columns. The compiler cannot see the projection's names when they are built from
    /// values, so the caller supplies them, together with a type witness for each:
    ///
    /// ```ignore
    /// let totals = db
    ///     .select(posts)
    ///     .pick((posts.author_id, count_all()))
    ///     .cte_named(
    ///         "totals",
    ///         (("author_id", posts.author_id), ("total", Int8)),
    ///     );
    ///
    /// let author = totals.column_named("author_id", posts.author_id);
    /// let total = totals.column_named("total", Int8);
    /// ```
    ///
    /// A witness is either the type marker itself (`Int8`) or a column of the intended type
    /// (`posts.author_id`), exactly as [`scalar`](crate::expr::scalar) accepts. The pairs can
    /// be a tuple — which is how differently typed columns fit in one list — or a homogeneous
    /// array or `Vec`.
    ///
    /// The list must describe the projection exactly; a mismatch is recorded on the statement
    /// that uses the expression, the same way a dialect capability is. The names must be
    /// plain identifiers, because they are the names a `SELECT` from the expression reads.
    ///
    /// A compound query gets the same constructor:
    /// [`SetQuery::cte_named`](crate::SetQuery::cte_named).
    pub fn cte_named<C: CteColumns>(self, name: &'static str, columns: C) -> Cte<R> {
        let mut body = QueryBody::Select(self.query);
        let prerequisites = body.take_with();
        let columns = columns.into_columns();
        let error = validate_cte(name, &columns, body.projection_len());
        Cte::new(name, body, columns.into(), true, prerequisites).with_error(error)
    }

    /// Escape the row-set type-state and finish the query with erased clauses.
    ///
    /// See [`DynamicSelect`] for when this is the right tool. The row type `R` is kept, so
    /// the result still decodes into the shape `pick` chose.
    ///
    /// ```ignore
    /// // The expression is built while its row set is known, then erased for the handle.
    /// let adults: Expr<(UserTable,), Bool> = users.age.gte(18);
    ///
    /// let query = db
    ///     .select(users)
    ///     .dynamic()
    ///     .filter(adults.into_kind())
    ///     .order_by(users.name.asc());
    /// ```
    pub fn dynamic(self) -> DynamicSelect<'a, E, R> {
        DynamicSelect::new(self.exec, self.query)
    }

    /// Count the rows this query produces.
    ///
    /// The query is wrapped in a derived table so its filters, joins, grouping, ordering and
    /// pagination all still apply:
    ///
    /// ```sql
    /// SELECT COUNT(*) FROM (
    ///     SELECT ... FROM "users" WHERE ("users"."age" >= $1) LIMIT $2
    /// ) AS "besu_count"
    /// ```
    ///
    /// It returns one `i64`, the same type [`count_all`](crate::count_all) produces. A
    /// compound gets the same helper: [`SetQuery::count`](crate::SetQuery::count).
    pub fn count(self) -> Select<'a, E, (), i64> {
        let Select {
            exec,
            query,
            phantom: _,
        } = self;
        count_query(exec, QueryBody::Select(query))
    }
}

/// Build the `SELECT COUNT(*) FROM (<body>) AS "besu_count"` statement.
pub(crate) fn count_query<'a, E: Executor>(exec: E, body: QueryBody) -> Select<'a, E, (), i64> {
    let mut query = SelectQuery::from_source(From::Derived(DerivedSource {
        alias: "besu_count",
        // The projection is `COUNT(*)`, so the outer statement never reads the derived
        // table's columns and their names are not needed.
        columns: alloc::vec::Vec::new(),
        body: alloc::boxed::Box::new(body),
    }));
    query.projection = Some(alloc::vec![crate::expr::count_all::<()>().into_kind()]);
    Select {
        exec,
        query,
        phantom: PhantomData,
    }
}

impl<'a, E: Executor, M, R> Select<'a, E, M, R> {
    /// An `INNER JOIN`. Rows must match on both sides.
    pub fn inner_join<T: TableRef>(
        mut self,
        table: T,
        on: Expr<(M, T::Marker), Bool>,
    ) -> Select<'a, E, (M, T::Marker), (R, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Inner,
            source: table.into_source(),
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `LEFT JOIN`. The joined row becomes `Option<T>`, `None` when nothing matched.
    pub fn left_join<T: TableRef>(
        mut self,
        table: T,
        on: Expr<(M, Option<T::Marker>), Bool>,
    ) -> Select<'a, E, (M, Option<T::Marker>), (R, Option<T::Model>)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Left,
            source: table.into_source(),
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `RIGHT JOIN`.
    pub fn right_join<T: TableRef>(
        mut self,
        table: T,
        on: Expr<(M, T::Marker), Bool>,
    ) -> Select<'a, E, (M, T::Marker), (Option<R>, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Right,
            source: table.into_source(),
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
    pub fn full_join<T: TableRef>(
        mut self,
        table: T,
        on: Expr<(M, Option<T::Marker>), Bool>,
    ) -> Select<'a, E, (M, Option<T::Marker>), (Option<R>, Option<T::Model>)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Full,
            source: table.into_source(),
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `CROSS JOIN`, which pairs every row with every row.
    pub fn cross_join<T: TableRef>(
        mut self,
        table: T,
    ) -> Select<'a, E, (M, T::Marker), (R, T::Model)> {
        self.query.joins.push(JoinClause {
            kind: JoinKind::Cross,
            source: table.into_source(),
            on: None,
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// An `INNER JOIN` against a common table expression.
    ///
    /// Reading the expression from a join adds its definition to the statement, so the
    /// `WITH` clause is rendered without a separate call.
    ///
    /// ```ignore
    /// let tree = db.select(nodes).filter(nodes.parent_id.is_null()).cte("tree");
    /// db.select(nodes)
    ///     .inner_join_cte(&tree, nodes.parent_id.eq(tree.column(nodes.id)))
    /// ```
    pub fn inner_join_cte<C: 'static>(
        mut self,
        cte: &Cte<C>,
        on: Expr<(M, Cte<C>), Bool>,
    ) -> Select<'a, E, (M, Cte<C>), (R, C)> {
        self.query.push_with(cte.entries());
        self.query.joins.push(JoinClause {
            kind: JoinKind::Inner,
            source: cte.source(),
            on: Some(on.into_kind()),
        });
        Select {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// A `LEFT JOIN` against a common table expression.
    ///
    /// The joined row becomes `Option<T>`, exactly as a left join against a table does.
    pub fn left_join_cte<C: 'static>(
        mut self,
        cte: &Cte<C>,
        on: Expr<(M, Option<Cte<C>>), Bool>,
    ) -> Select<'a, E, (M, Option<Cte<C>>), (R, Option<C>)> {
        self.query.push_with(cte.entries());
        self.query.joins.push(JoinClause {
            kind: JoinKind::Left,
            source: cte.source(),
            on: Some(on.into_kind()),
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
        let prefix = self.exec.table_prefix();
        let (mut exec, query) = self.into_parts();
        let sql = query
            .build::<DialectOf<E>>(prefix)
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        decode_rows::<E::Driver, R>(&rows)
    }

    /// Run the query and return the first row, if any.
    pub async fn first(mut self) -> Result<Option<R>, ErrorOf<E>> {
        self.query.limit = Some(1);
        let prefix = self.exec.table_prefix();
        let (mut exec, query) = self.into_parts();
        let sql = query
            .build::<DialectOf<E>>(prefix)
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&sql).await?;
        let mut decoded = decode_rows::<E::Driver, R>(&rows)?;
        Ok(decoded.pop())
    }
}

/// Decode every row into `R`, one allocation for the whole result.
///
/// Generic over the driver rather than the executor so a batch, which holds the driver's
/// transaction rather than an executor, decodes a query exactly the way awaiting it on its
/// own does.
pub(crate) fn decode_rows<Dr: Driver, R: Decode<Dr>>(
    rows: &[Dr::Row],
) -> Result<alloc::vec::Vec<R>, Dr::Error> {
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

impl<'a, E: Executor, M, R> IntoQueryBody for Select<'a, E, M, R> {
    type Row = R;

    fn into_query_body(self) -> QueryBody {
        QueryBody::Select(self.query)
    }
}
