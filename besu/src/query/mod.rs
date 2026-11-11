//! Query builders.

pub(crate) mod delete;
pub(crate) mod insert;
pub(crate) mod select;
pub(crate) mod set;
pub(crate) mod update;

pub use delete::{Delete, DeleteReturning};
pub use insert::{Insert, InsertReturning, OnConflict};
pub use select::Select;
pub use set::{SetOp, SetQuery};
pub use update::{Update, UpdateReturning};

use alloc::vec::Vec;

use besu_core::{Dialect, Sql, TableDef};

use crate::expr::{ExprKind, NullsOrder, Order, OrderClause};

/// How a table is joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// `INNER JOIN`
    Inner,
    /// `LEFT JOIN`
    Left,
    /// `RIGHT JOIN`
    Right,
    /// `FULL OUTER JOIN`
    Full,
    /// `CROSS JOIN`
    Cross,
}

impl JoinKind {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Inner => "INNER JOIN",
            Self::Left => "LEFT JOIN",
            Self::Right => "RIGHT JOIN",
            Self::Full => "FULL OUTER JOIN",
            Self::Cross => "CROSS JOIN",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JoinClause {
    pub(crate) kind: JoinKind,
    pub(crate) table: &'static TableDef,
    pub(crate) on: Option<ExprKind>,
}

/// Write a possibly schema-qualified table name.
pub(crate) fn push_table<D: Dialect>(out: &mut Sql<D>, table: &TableDef) {
    out.push_table(table.schema, table.name);
}

/// The bytes a table contributes to a `SELECT` list, quoted and comma-separated.
///
/// This is only ever an estimate used to size the output buffer. Six bytes covers the two
/// quote pairs, the dot and the separator, so it errs high rather than low, which is the
/// direction that saves a copy.
pub(crate) fn table_list_bytes(table: &TableDef) -> usize {
    table
        .columns
        .iter()
        .map(|column| table.name.len() + column.name.len() + 6)
        .sum()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectQuery {
    pub(crate) from: &'static TableDef,
    /// `None` means "every column of every table in the query".
    pub(crate) projection: Option<Vec<ExprKind>>,
    pub(crate) joins: Vec<JoinClause>,
    pub(crate) filter: Vec<ExprKind>,
    pub(crate) group_by: Vec<ExprKind>,
    pub(crate) having: Vec<ExprKind>,
    pub(crate) order_by: Vec<OrderClause>,
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
    pub(crate) distinct: bool,
    pub(crate) lock: Option<Lock>,
}

/// A `FOR UPDATE` / `FOR SHARE` clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Lock {
    pub(crate) strength: LockStrength,
    pub(crate) skip_locked: bool,
    pub(crate) no_wait: bool,
}

/// The strength of a row lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockStrength {
    /// `FOR UPDATE`: lock the rows for writing.
    Update,
    /// `FOR SHARE`: lock the rows against writes by others.
    Share,
}

impl SelectQuery {
    pub(crate) fn new(from: &'static TableDef) -> Self {
        Self {
            from,
            projection: None,
            joins: Vec::new(),
            filter: Vec::new(),
            group_by: Vec::new(),
            having: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
            distinct: false,
            lock: None,
        }
    }

    /// Render the query.
    ///
    /// Takes `self` because rendering moves the bound values into the statement instead of
    /// cloning them. [`Select::to_sql`](crate::Select::to_sql) clones the query first, so a
    /// builder can still be rendered and then run.
    pub(crate) fn build<D: Dialect>(mut self, prefix: Option<alloc::sync::Arc<str>>) -> Sql<D> {
        let mut out = Sql::with_capacity(self.capacity_hint());
        out.set_table_prefix(prefix);
        self.render_into(&mut out);
        // Sealed here rather than at the call site so that every path — `to_sql`, `all`,
        // `stream`, a relation's second statement, a chunked batch — sends the same text the
        // driver caches under.
        out.seal()
    }

    /// Render this query into an existing statement.
    ///
    /// This is what makes a subquery a subquery: its parameters continue the enclosing
    /// statement's numbering, and any error it records is the enclosing statement's error.
    /// The table prefix is the one already set on `out`, so a prefixed statement stays
    /// prefixed all the way down.
    pub(crate) fn render_into<D: Dialect>(&mut self, out: &mut Sql<D>) {
        out.push_raw("SELECT ");
        if self.distinct {
            out.push_raw("DISTINCT ");
        }

        match &mut self.projection {
            Some(projection) => {
                for (i, expr) in projection.iter_mut().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    expr.render(out);
                }
            }
            None => {
                let mut first = true;
                for table in
                    core::iter::once(self.from).chain(self.joins.iter().map(|join| join.table))
                {
                    for column in table.columns {
                        if !first {
                            out.push_raw(", ");
                        }
                        first = false;
                        out.push_qualified(table.name, column.name);
                    }
                }
            }
        }

        out.push_raw(" FROM ");
        push_table(out, self.from);

        for join in &mut self.joins {
            if join.kind == JoinKind::Full && !D::SUPPORTS_FULL_JOIN {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` has no `FULL OUTER JOIN`",
                    D::NAME
                )));
            }
            if (join.kind == JoinKind::Right) && !D::SUPPORTS_RIGHT_JOIN {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` has no `RIGHT JOIN`",
                    D::NAME
                )));
            }
            out.push_raw(" ");
            out.push_raw(join.kind.as_sql());
            out.push_raw(" ");
            push_table(out, join.table);
            if let Some(on) = &mut join.on {
                out.push_raw(" ON ");
                on.render(out);
            }
        }

        if !self.filter.is_empty() {
            out.push_raw(" WHERE ");
            for (i, condition) in self.filter.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(" AND ");
                }
                condition.render(out);
            }
        }

        if !self.group_by.is_empty() {
            out.push_raw(" GROUP BY ");
            for (i, expr) in self.group_by.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                expr.render(out);
            }
        }

        if !self.having.is_empty() {
            out.push_raw(" HAVING ");
            for (i, condition) in self.having.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(" AND ");
                }
                condition.render(out);
            }
        }

        if !self.order_by.is_empty() {
            out.push_raw(" ORDER BY ");
            for (i, clause) in self.order_by.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                render_order(out, clause);
            }
        }

        render_limit::<D>(out, self.limit, self.offset);

        if let Some(lock) = &self.lock {
            if !D::SUPPORTS_ROW_LOCKING {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` has no row locking; `FOR UPDATE` is not available",
                    D::NAME
                )));
                return;
            }
            out.push_raw(match lock.strength {
                LockStrength::Update => " FOR UPDATE",
                LockStrength::Share => " FOR SHARE",
            });
            if lock.skip_locked {
                out.push_raw(" SKIP LOCKED");
            }
            if lock.no_wait {
                out.push_raw(" NOWAIT");
            }
        }
    }

    /// The number of SQL columns the projection produces.
    ///
    /// A set operation requires every branch to select the same number of columns, and an
    /// `IN (SELECT ...)` requires exactly one. `R` makes both true for every query the
    /// builder can express, but checking the rendered column count costs nothing and turns a
    /// future hole in the type layer into a good error instead of invalid SQL.
    pub(crate) fn projection_len(&self) -> usize {
        match &self.projection {
            Some(projection) => projection.len(),
            None => core::iter::once(self.from)
                .chain(self.joins.iter().map(|join| join.table))
                .map(|table| table.columns.len())
                .sum(),
        }
    }

    /// A rough upper bound on the bytes this query will render to.
    pub(crate) fn capacity_hint(&self) -> usize {
        // "SELECT " and the clauses that never vary in wording.
        let mut bytes = 32;

        match &self.projection {
            Some(projection) => bytes += projection.len() * 24,
            None => {
                bytes += table_list_bytes(self.from);
                for join in &self.joins {
                    bytes += table_list_bytes(join.table);
                }
            }
        }

        // A condition is at worst a few dozen bytes of quoting and operators around its
        // parameters, which do not appear in the text at all.
        bytes
            + (self.filter.len() + self.having.len()) * 40
            + self.joins.len() * 48
            + self.group_by.len() * 24
            + self.order_by.len() * 24
    }
}

/// Write `LIMIT` and `OFFSET`, in the order and spelling the dialect needs.
pub(crate) fn render_limit<D: Dialect>(out: &mut Sql<D>, limit: Option<i64>, offset: Option<i64>) {
    match (limit, offset) {
        (Some(limit), _) => {
            out.push_raw(" LIMIT ");
            out.bind(limit);
            if let Some(offset) = offset {
                out.push_raw(" OFFSET ");
                out.bind(offset);
            }
        }
        (None, Some(offset)) => {
            if D::REQUIRES_LIMIT_WITH_OFFSET {
                out.push_raw(" LIMIT ");
                out.bind(D::UNBOUNDED_LIMIT);
            }
            out.push_raw(" OFFSET ");
            out.bind(offset);
        }
        (None, None) => {}
    }
}

/// Render one `ORDER BY` term.
pub(crate) fn render_order<D: Dialect>(out: &mut Sql<D>, clause: &mut OrderClause) {
    // MySQL has no `NULLS FIRST` / `NULLS LAST`, so the standard emulation is to sort by
    // `expr IS NULL` first. That produces the same ordering for the common cases.
    if !D::SUPPORTS_NULLS_ORDER {
        if let Some(nulls) = clause.nulls {
            out.push_raw("(");
            clause.expr.render(out);
            out.push_raw(" IS NULL) ");
            out.push_raw(match nulls {
                NullsOrder::First => "DESC, ",
                NullsOrder::Last => "ASC, ",
            });
        }
    }

    clause.expr.render(out);
    out.push_raw(match clause.direction {
        Order::Asc => " ASC",
        Order::Desc => " DESC",
    });

    if D::SUPPORTS_NULLS_ORDER {
        if let Some(nulls) = clause.nulls {
            out.push_raw(match nulls {
                NullsOrder::First => " NULLS FIRST",
                NullsOrder::Last => " NULLS LAST",
            });
        }
    }
}
