//! Query builders.

pub(crate) mod cte;
pub(crate) mod delete;
pub(crate) mod insert;
pub(crate) mod select;
pub(crate) mod set;
pub(crate) mod update;

pub use cte::Cte;
pub use delete::{Delete, DeleteReturning};
pub use insert::{Insert, InsertReturning, OnConflict};
pub use select::Select;
pub use set::{SetOp, SetQuery};
pub use update::{Update, UpdateReturning};

use alloc::vec::Vec;

use besu_core::{ColumnDef, Dialect, Sql, TableDef};

use crate::expr::{ExprKind, NullsOrder, Order, OrderClause};

/// Where a `SELECT` reads its rows from.
///
/// A statement's source is either a table in the database or a common table expression
/// defined by the same statement. They render differently in three places — the name, the
/// prefix, and the default projection — so the distinction has to be explicit rather than a
/// bare `&'static TableDef`.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum From {
    /// A table in the database, which is schema-qualified and given the tenant prefix.
    Table(&'static TableDef),
    /// A common table expression, which is neither.
    Cte(CteSource),
}

/// A common table expression used as a `SELECT`'s source.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct CteSource {
    /// The name written in the `WITH` clause.
    pub name: &'static str,
    /// The columns the expression produces, in order.
    pub columns: &'static [ColumnDef],
}

impl From {
    /// The columns the source produces, in order.
    pub(crate) fn columns(&self) -> &'static [ColumnDef] {
        match self {
            Self::Table(def) => def.columns,
            Self::Cte(source) => source.columns,
        }
    }

    /// The quoted name, as it appears after `FROM`.
    pub(crate) fn render<D: Dialect>(&self, out: &mut Sql<D>) {
        match self {
            Self::Table(def) => out.push_table(def.schema, def.name),
            // A CTE is a name in the statement, not a table in the database, so it is never
            // schema-qualified and never given the tenant prefix.
            Self::Cte(source) => out.push_ident(source.name),
        }
    }

    /// The quoted and qualified name of one of the source's columns.
    pub(crate) fn render_column<D: Dialect>(&self, out: &mut Sql<D>, column: &str) {
        match self {
            Self::Table(def) => out.push_qualified(def.name, column),
            Self::Cte(source) => {
                out.push_ident(source.name);
                out.push_raw(".");
                out.push_ident(column);
            }
        }
    }

    /// The bytes the whole source contributes to a `SELECT` list, for sizing the buffer.
    pub(crate) fn list_bytes(&self) -> usize {
        match self {
            Self::Table(def) => table_list_bytes(def),
            Self::Cte(source) => source
                .columns
                .iter()
                .map(|column| source.name.len() + column.name.len() + 6)
                .sum(),
        }
    }
}

/// The `WITH` clause a statement may carry.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    /// Whether `WITH RECURSIVE` is written.
    ///
    /// It is set when a definition refers to itself, and left off otherwise. Writing
    /// `RECURSIVE` for a clause that does not need it is valid SQL, but saying exactly what
    /// the statement means is worth the flag.
    pub(crate) recursive: bool,
    pub(crate) entries: Vec<CteEntry>,
}

impl WithClause {
    fn push(&mut self, mut other: WithClause) {
        self.recursive |= other.recursive;
        for entry in other.entries.drain(..) {
            if self
                .entries
                .iter()
                .all(|existing| existing.name != entry.name)
            {
                self.entries.push(entry);
            }
        }
    }
}

/// One named expression in a `WITH` clause.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CteEntry {
    pub(crate) name: &'static str,
    pub(crate) columns: &'static [ColumnDef],
    pub(crate) body: crate::query::set::SetParts,
}

/// The expression a compound query is the definition of.
///
/// When a CTE is extended into a compound — `anchor.union_all(recursive_term)` — the
/// compound *is* the definition, and the statement that runs it is a `SELECT` from the name
/// it defines.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CteRoot {
    pub(crate) name: &'static str,
    pub(crate) columns: &'static [ColumnDef],
}

/// Write a `WITH` clause and the separator that follows it.
pub(crate) fn render_with<D: Dialect>(out: &mut Sql<D>, with: &mut Option<WithClause>) {
    let Some(with) = with else {
        return;
    };
    if with.entries.is_empty() {
        return;
    }

    out.push_raw("WITH ");
    if with.recursive {
        out.push_raw("RECURSIVE ");
    }
    for (i, entry) in with.entries.iter_mut().enumerate() {
        if i != 0 {
            out.push_raw(", ");
        }
        out.push_ident(entry.name);
        out.push_raw(" AS (");
        entry.body.render_branches(out);
        out.push_raw(")");
    }
    out.push_raw(" ");
}

/// Write an application-level comment after a finished statement.
///
/// The comment is appended once, after the dialect's own suffix, so it is the last thing in
/// the text. It is part of the statement string rather than a driver feature, which means a
/// slow-query log, a `pg_stat_statements` entry and the `Logged` event all show it without
/// any of them knowing about comments.
///
/// An embedded `*/` would end the comment early and turn the rest of the label into SQL, so
/// it is written as `* /`. The label therefore cannot be used to inject anything, and a
/// comment that happens to contain the sequence simply reads slightly differently.
pub(crate) fn render_comment<D: Dialect>(out: &mut Sql<D>, comment: Option<&str>) {
    let Some(comment) = comment else {
        return;
    };
    if comment.is_empty() {
        return;
    }
    out.push_raw(" /* ");
    match comment.find("*/") {
        Some(_) => out.push_raw(&comment.replace("*/", "* /")),
        None => out.push_raw(comment),
    }
    out.push_raw(" */");
}

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
    pub(crate) source: From,
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
    pub(crate) from: From,
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
    /// An application-level label, rendered as a trailing `/* ... */`.
    pub(crate) comment: Option<alloc::string::String>,
    /// The common table expressions this statement defines.
    pub(crate) with: Option<WithClause>,
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
        Self::from_source(From::Table(from))
    }

    pub(crate) fn from_source(from: From) -> Self {
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
            comment: None,
            with: None,
        }
    }

    /// Merge a `WITH` clause into this statement's, keeping the first entry of each name.
    pub(crate) fn push_with(&mut self, with: WithClause) {
        match &mut self.with {
            Some(existing) => existing.push(with),
            None => self.with = Some(with),
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
        // driver caches under. The comment goes after the suffix, so it is the last thing in
        // the statement.
        let mut out = out.seal();
        render_comment(&mut out, self.comment.as_deref());
        out
    }

    /// Render this query into an existing statement.
    ///
    /// This is what makes a subquery a subquery: its parameters continue the enclosing
    /// statement's numbering, and any error it records is the enclosing statement's error.
    /// The table prefix is the one already set on `out`, so a prefixed statement stays
    /// prefixed all the way down.
    pub(crate) fn render_into<D: Dialect>(&mut self, out: &mut Sql<D>) {
        render_with(out, &mut self.with);

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
                for index in 0..=self.joins.len() {
                    let source = if index == 0 {
                        &self.from
                    } else {
                        &self.joins[index - 1].source
                    };
                    for column in source.columns() {
                        if !first {
                            out.push_raw(", ");
                        }
                        first = false;
                        source.render_column(out, column.name);
                    }
                }
            }
        }

        out.push_raw(" FROM ");
        self.from.render(out);

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
            join.source.render(out);
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
            None => core::iter::once(&self.from)
                .chain(self.joins.iter().map(|join| &join.source))
                .map(|source| source.columns().len())
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
                bytes += self.from.list_bytes();
                for join in &self.joins {
                    bytes += join.source.list_bytes();
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
            + self.comment.as_ref().map_or(0, alloc::string::String::len)
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
