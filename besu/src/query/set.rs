//! Set operations: `UNION`, `INTERSECT` and `EXCEPT`.
//!
//! A compound query is a list of `SELECT`s joined by operators, with one `ORDER BY`,
//! `LIMIT` and `OFFSET` of its own. The branches keep their own filters, joins, grouping and
//! projections; the ordering and pagination belong to the compound, because that is where
//! SQL puts them.

use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, Dialect, DialectOf, Driver, Error, ErrorOf, Executor, Sql};

use crate::expr::{IntoOrder, OrderClause};
use crate::query::{Select, SelectQuery, render_limit, render_order, select::decode_rows};

/// Which set operator joins two `SELECT`s.
///
/// The difference between an operator and its `ALL` form is multiset semantics: `UNION`
/// removes duplicates, `UNION ALL` keeps them; `INTERSECT ALL` keeps a row as many times as
/// the shorter side contains it; `EXCEPT ALL` subtracts counts instead of removing every
/// copy. The `ALL` forms are not available everywhere — see
/// [`Dialect::SUPPORTS_INTERSECT_ALL`] and [`Dialect::SUPPORTS_EXCEPT_ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOp {
    /// `UNION`: every row from both sides, with duplicates removed.
    Union,
    /// `UNION ALL`: every row from both sides, keeping duplicates.
    UnionAll,
    /// `INTERSECT`: rows that both sides contain, once.
    Intersect,
    /// `INTERSECT ALL`: rows that both sides contain, keeping the smaller count.
    IntersectAll,
    /// `EXCEPT`: rows from the left that the right does not contain.
    Except,
    /// `EXCEPT ALL`: rows from the left minus the right, by count.
    ExceptAll,
}

impl SetOp {
    /// How the operator is spelled.
    fn as_sql(self) -> &'static str {
        match self {
            Self::Union => "UNION",
            Self::UnionAll => "UNION ALL",
            Self::Intersect => "INTERSECT",
            Self::IntersectAll => "INTERSECT ALL",
            Self::Except => "EXCEPT",
            Self::ExceptAll => "EXCEPT ALL",
        }
    }

    /// Whether this is one of the operators that binds more tightly than `UNION`.
    ///
    /// PostgreSQL and MySQL evaluate `INTERSECT` before `UNION` and `EXCEPT`; SQLite
    /// evaluates every compound operator left to right. A statement that mixes the two
    /// families therefore means different things on different databases, which is why the
    /// query layer refuses it rather than picking one.
    const fn is_intersect(self) -> bool {
        matches!(self, Self::Intersect | Self::IntersectAll)
    }
}

/// Two or more `SELECT`s combined with `UNION`, `INTERSECT` or `EXCEPT`.
///
/// Built by [`Select::union`] and its siblings. The row type `R` is the row type of every
/// branch, which is what stops a `UNION` of two unrelated projections from compiling. The
/// branches are rendered one after another with no parentheses, which is the form all three
/// dialects accept.
///
/// Ordering, limits and offsets belong to the compound: `SetQuery::order_by`, `limit` and
/// `offset` come after the last branch. A branch may not carry its own — MySQL's grammar
/// does not allow it without a derived table, and silently dropping it would mean running a
/// different query from the one you wrote — so using one on a branch records an
/// [`Error::unsupported`](besu_core::Error::Unsupported) naming the branch.
pub struct SetQuery<'a, E: Executor, M, R> {
    pub(crate) exec: E,
    pub(crate) query: SetParts,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> (M, R))>,
}

/// The dialect-independent half of a [`SetQuery`].
///
/// It is separate so that [`SetQuery::to_sql`] can render without consuming the builder:
/// rendering moves the bound values into the statement, so a borrow has to clone this first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SetParts {
    branches: Vec<SelectQuery>,
    /// One operator between each pair of branches, so one fewer than `branches`.
    operators: Vec<SetOp>,
    order_by: Vec<OrderClause>,
    limit: Option<i64>,
    offset: Option<i64>,
}

impl SetParts {
    fn new(first: SelectQuery) -> Self {
        Self {
            branches: alloc::vec![first],
            operators: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }
    }

    fn push(&mut self, op: SetOp, branch: SelectQuery) {
        self.operators.push(op);
        self.branches.push(branch);
    }

    /// Render the compound query.
    fn render_into<D: Dialect>(&mut self, out: &mut Sql<D>) {
        self.check_precedence(out);

        let mut expected = None;
        for index in 0..self.branches.len() {
            let columns = self.branches[index].projection_len();
            match expected {
                None => expected = Some(columns),
                Some(expected) if expected != columns => {
                    out.set_error(Error::invalid_query(alloc::format!(
                        "branch {} of this set operation selects {columns} columns but branch \
                         1 selects {expected}; every branch must select the same number",
                        index + 1
                    )));
                }
                Some(_) => {}
            }

            if index != 0 {
                let op = self.operators[index - 1];
                check_capability::<D>(out, op);
                out.push_raw(" ");
                out.push_raw(op.as_sql());
                out.push_raw(" ");
            }

            check_branch::<D>(out, index, &self.branches[index]);
            self.branches[index].render_into(out);
        }

        if !self.order_by.is_empty() {
            out.push_raw(" ORDER BY ");
            for (i, clause) in self.order_by.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                // PostgreSQL and MySQL only accept a result column name here, so a
                // qualified one is rewritten to the bare name the compound exposes.
                clause.expr.unqualify();
                render_order(out, clause);
            }
        }

        render_limit::<D>(out, self.limit, self.offset);
    }

    fn capacity_hint(&self) -> usize {
        // Each branch budgets for its own clauses; this covers the operators, the spacing
        // and the compound's `ORDER BY` / `LIMIT`.
        let mut bytes = 32;
        for branch in &self.branches {
            bytes += branch.capacity_hint();
        }
        bytes + self.operators.len() * 12 + self.order_by.len() * 24
    }

    /// Refuse a chain that mixes `INTERSECT` with `UNION` or `EXCEPT`.
    ///
    /// The SQL has no parentheses, because SQLite cannot parse a parenthesised compound
    /// operand, so the meaning of a mixed chain depends on the database's precedence rules:
    /// PostgreSQL and MySQL bind `INTERSECT` tighter, SQLite goes left to right. Rather than
    /// run a statement that means two different things, the query fails and says why.
    fn check_precedence<D: Dialect>(&self, out: &mut Sql<D>) {
        let Some(first) = self.operators.first() else {
            return;
        };

        if self
            .operators
            .iter()
            .any(|op| op.is_intersect() != first.is_intersect())
        {
            out.set_error(Error::unsupported(
                "this set operation mixes `INTERSECT` with `UNION` or `EXCEPT`, which \
                 PostgreSQL and MySQL group differently from SQLite; run one operator per set \
                 query, or combine them through a subquery",
            ));
        }
    }
}

/// Reject an `ALL` operator the dialect cannot spell.
fn check_capability<D: Dialect>(out: &mut Sql<D>, op: SetOp) {
    let suggestion = match op {
        SetOp::IntersectAll if !D::SUPPORTS_INTERSECT_ALL => {
            Some("`intersect`, which removes duplicates")
        }
        SetOp::ExceptAll if !D::SUPPORTS_EXCEPT_ALL => Some("`except`, which removes duplicates"),
        _ => None,
    };

    if let Some(suggestion) = suggestion {
        out.set_error(Error::unsupported(alloc::format!(
            "`{}` has no `{}`; use {suggestion} instead",
            D::NAME,
            op.as_sql()
        )));
    }
}

/// Reject the clauses that belong to the compound rather than to a branch.
///
/// A parenthesised branch could carry them on PostgreSQL and SQLite, but MySQL's grammar
/// wants a derived table, and quietly dropping them would run something other than what was
/// written. Naming the branch is the honest answer.
fn check_branch<D: Dialect>(out: &mut Sql<D>, index: usize, branch: &SelectQuery) {
    if !branch.order_by.is_empty() {
        reject_branch::<D>(
            out,
            index,
            "ORDER BY",
            "call `order_by` on the set query so it sorts the whole result",
        );
    }
    if branch.limit.is_some() {
        reject_branch::<D>(
            out,
            index,
            "LIMIT",
            "call `limit` on the set query so it applies to the whole result",
        );
    }
    if branch.offset.is_some() {
        reject_branch::<D>(
            out,
            index,
            "OFFSET",
            "call `offset` on the set query so it applies to the whole result",
        );
    }
    if branch.lock.is_some() {
        reject_branch::<D>(
            out,
            index,
            "FOR UPDATE",
            "there is no set-query equivalent, so run the locked `SELECT` on its own",
        );
    }
}

fn reject_branch<D: Dialect>(out: &mut Sql<D>, index: usize, clause: &str, fix: &str) {
    out.set_error(Error::unsupported(alloc::format!(
        "`{clause}` on branch {} of a set operation is not portable; {fix}",
        index + 1
    )));
}

impl<'a, E: Executor, M, R> SetQuery<'a, E, M, R> {
    /// Start a compound query from a branch and an operator.
    pub(crate) fn new(exec: E, first: SelectQuery, op: SetOp, second: SelectQuery) -> Self {
        let mut query = SetParts::new(first);
        query.push(op, second);
        Self {
            exec,
            query,
            phantom: PhantomData,
        }
    }

    /// Add a `UNION` branch: rows from both sides, with duplicates removed.
    pub fn union<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::Union, other.query);
        self
    }

    /// Add a `UNION ALL` branch: rows from both sides, keeping duplicates.
    pub fn union_all<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::UnionAll, other.query);
        self
    }

    /// Add an `INTERSECT` branch: rows that both sides contain, once.
    pub fn intersect<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::Intersect, other.query);
        self
    }

    /// Add an `INTERSECT ALL` branch: rows that both sides contain, keeping the smaller
    /// count. Not every dialect can spell this; see [`Dialect::SUPPORTS_INTERSECT_ALL`].
    pub fn intersect_all<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::IntersectAll, other.query);
        self
    }

    /// Add an `EXCEPT` branch: rows from the left that the right does not contain.
    pub fn except<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::Except, other.query);
        self
    }

    /// Add an `EXCEPT ALL` branch: rows from the left minus the right, by count. Not every
    /// dialect can spell this; see [`Dialect::SUPPORTS_EXCEPT_ALL`].
    pub fn except_all<'a2, E2, M2>(mut self, other: Select<'a2, E2, M2, R>) -> Self
    where
        E2: Executor<Driver = E::Driver>,
    {
        self.query.push(SetOp::ExceptAll, other.query);
        self
    }

    /// Add an `ORDER BY` term for the whole compound.
    ///
    /// The term names an output column of the branches, and it is written unqualified
    /// because PostgreSQL and MySQL accept nothing else there. An expression other than a
    /// column is written as given and may be rejected by those databases: they allow only
    /// result column names.
    pub fn order_by<O: IntoOrder>(mut self, order: O) -> Self {
        self.query.order_by.push(order.into_order());
        self
    }

    /// Add a descending `ORDER BY` term for the whole compound.
    pub fn order_by_desc<O: IntoOrder>(mut self, order: O) -> Self {
        self.query.order_by.push(order.into_order().reverse());
        self
    }

    /// `LIMIT n` on the whole compound.
    pub fn limit(mut self, limit: i64) -> Self {
        self.query.limit = Some(limit);
        self
    }

    /// `OFFSET n` on the whole compound.
    pub fn offset(mut self, offset: i64) -> Self {
        self.query.offset = Some(offset);
        self
    }

    /// Render the exact SQL this query will run, without running it.
    ///
    /// Like [`Select::to_sql`], a capability the dialect lacks is recorded as
    /// [`Sql::error`] rather than returned here.
    pub fn to_sql(&self) -> Result<Sql<DialectOf<E>>, Error> {
        Ok(self.render())
    }

    /// Render the SQL with its parameters inlined, for logging.
    pub fn inspect(&self) -> Result<alloc::string::String, Error> {
        Ok(alloc::string::ToString::to_string(&self.render().inspect()))
    }

    fn render(&self) -> Sql<DialectOf<E>> {
        let mut query = self.query.clone();
        let mut out = Sql::with_capacity(query.capacity_hint());
        out.set_table_prefix(self.exec.table_prefix());
        query.render_into(&mut out);
        // Sealed here, so the text `to_sql` shows and the text the driver runs are the same
        // string — which is what the prepared-statement cache key depends on.
        out.seal()
    }
}

impl<'a, E: Executor, M, R> SetQuery<'a, E, M, R>
where
    R: Decode<E::Driver>,
{
    /// Run the query and return every row.
    pub async fn all(self) -> Result<Vec<R>, ErrorOf<E>> {
        let sql = self
            .render()
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let mut exec = self.exec;
        let rows = exec.query(&sql).await?;
        decode_rows::<E, R>(&rows)
    }

    /// Run the query and return the first row, if any.
    ///
    /// This sets `limit(1)` on the compound, replacing any limit already set, exactly as
    /// [`Select::first`] does.
    pub async fn first(mut self) -> Result<Option<R>, ErrorOf<E>> {
        self.query.limit = Some(1);
        let mut rows = self.all().await?;
        Ok(rows.pop())
    }
}

impl<'a, E, M, R> IntoFuture for SetQuery<'a, E, M, R>
where
    E: Executor + Send + 'a,
    M: 'a,
    R: Decode<E::Driver> + 'a,
{
    type Output = Result<Vec<R>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.all())
    }
}
