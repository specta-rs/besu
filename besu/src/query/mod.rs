//! Query builders.

pub(crate) mod delete;
pub(crate) mod insert;
pub(crate) mod select;
pub(crate) mod update;

pub use delete::{Delete, DeleteReturning};
pub use insert::{Insert, InsertReturning, OnConflict};
pub use select::Select;
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

#[derive(Debug, Clone)]
pub(crate) struct JoinClause {
    pub(crate) kind: JoinKind,
    pub(crate) table: &'static TableDef,
    pub(crate) on: Option<ExprKind>,
}

/// Write a possibly schema-qualified table name.
pub(crate) fn push_table<D: Dialect>(out: &mut Sql<D>, table: &TableDef) {
    out.push_table(table.schema, table.name);
}

#[derive(Debug, Clone)]
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

    pub(crate) fn build<D: Dialect>(&self, prefix: Option<alloc::sync::Arc<str>>) -> Sql<D> {
        let mut out = Sql::new();
        out.set_table_prefix(prefix);

        out.push_raw("SELECT ");
        if self.distinct {
            out.push_raw("DISTINCT ");
        }

        match &self.projection {
            Some(projection) => {
                for (i, expr) in projection.iter().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    expr.render(&mut out);
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
        push_table(&mut out, self.from);

        for join in &self.joins {
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
            push_table(&mut out, join.table);
            if let Some(on) = &join.on {
                out.push_raw(" ON ");
                on.render(&mut out);
            }
        }

        if !self.filter.is_empty() {
            out.push_raw(" WHERE ");
            for (i, condition) in self.filter.iter().enumerate() {
                if i != 0 {
                    out.push_raw(" AND ");
                }
                condition.render(&mut out);
            }
        }

        if !self.group_by.is_empty() {
            out.push_raw(" GROUP BY ");
            for (i, expr) in self.group_by.iter().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                expr.render(&mut out);
            }
        }

        if !self.having.is_empty() {
            out.push_raw(" HAVING ");
            for (i, condition) in self.having.iter().enumerate() {
                if i != 0 {
                    out.push_raw(" AND ");
                }
                condition.render(&mut out);
            }
        }

        if !self.order_by.is_empty() {
            out.push_raw(" ORDER BY ");
            for (i, clause) in self.order_by.iter().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                render_order(&mut out, clause);
            }
        }

        self.render_limit(&mut out);

        if let Some(lock) = &self.lock {
            if !D::SUPPORTS_ROW_LOCKING {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` has no row locking; `FOR UPDATE` is not available",
                    D::NAME
                )));
                return out;
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

        out
    }

    fn render_limit<D: Dialect>(&self, out: &mut Sql<D>) {
        match (self.limit, self.offset) {
            (Some(limit), _) => {
                out.push_raw(" LIMIT ");
                out.bind(limit);
                if let Some(offset) = self.offset {
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
}

fn render_order<D: Dialect>(out: &mut Sql<D>, clause: &OrderClause) {
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
