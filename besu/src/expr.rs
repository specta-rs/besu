//! Typed SQL expressions.
//!
//! An [`Expr<T, S>`] is a SQL expression that produces a value of SQL type `S` in a query
//! whose row set is `T`. Conditions are just expressions whose type is `Bool`, which is why
//! `and`, `or` and `not` compose with everything else instead of living in a parallel
//! universe.
//!
//! Expressions hold their values, so building one never borrows anything and a finished
//! query is self-contained.

use core::marker::PhantomData;

use alloc::string::String;
use alloc::vec::Vec;

use besu_core::{Dialect, Executor, Sql, TypeDef, Value};

use crate::types::{
    Bool, Bytes, Char, Column, Date, Decimal, Float4, Float8, I128, Int1, Int2, Int4, Int8, Json,
    Jsonb, Nullability, SqlType, Text, Time, Timestamp, TimestampTz, U128, UInt1, UInt2, UInt4,
    UInt8, Uuid, VarChar,
};

use crate::condition::Contains;

macro_rules! expr_comparisons {
    ($( $(#[$attr:meta])* fn $name:ident => $op:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<A, M>(self, other: A) -> Expr<T, Bool>
            where
                A: IntoExpr<T, S, M>,
            {
                Expr::from_kind(ExprKind::Binary {
                    op: $op,
                    left: Operand::new(self.into_kind()),
                    right: Operand::new(other.into_expr().into_kind()),
                })
            }
        )*
    };
}

use crate::table::Table;

/// The kinds of expression Besu knows how to render.
///
/// This is deliberately a small, closed set: everything SQL can express that is worth
/// type-checking has a variant, and everything else goes through [`ExprKind::Raw`]. Because
/// it is dialect-independent, rendering happens once, at the very end, against whichever
/// dialect the query is built for.
#[derive(Debug, Clone, PartialEq)]
#[doc(hidden)]
pub enum ExprKind {
    /// A plain column reference.
    Column {
        table: &'static str,
        column: &'static str,
    },
    /// A bound parameter.
    Value(Value),
    /// `*`, only valid in a few places such as `COUNT(*)`.
    Star,
    /// Raw SQL. Never derived from user input.
    Raw(String),
    /// `left op right`, always parenthesised so precedence is never surprising.
    Binary {
        op: &'static str,
        left: Operand,
        right: Operand,
    },
    /// `op expr`, as in `NOT x` or `-x`.
    Prefix { op: &'static str, expr: Operand },
    /// `expr op`, as in `x IS NULL`.
    Postfix { op: &'static str, expr: Operand },
    /// `NAME(args...)`.
    Function {
        name: &'static str,
        args: Vec<ExprKind>,
        distinct: bool,
    },
    /// `expr [NOT] BETWEEN low AND high`.
    Between {
        expr: Operand,
        low: Operand,
        high: Operand,
        negated: bool,
    },
    /// `expr [NOT] IN (list)`.
    InList {
        expr: Operand,
        list: Vec<ExprKind>,
        negated: bool,
    },
    /// `CASE ... WHEN ... THEN ... ELSE ... END`.
    Case {
        operand: Option<Operand>,
        branches: Vec<(ExprKind, ExprKind)>,
        fallback: Option<Operand>,
    },
    /// `CAST(expr AS type)`.
    Cast { expr: Operand, ty: TypeDef },
    /// A column of the row an upsert is trying to insert.
    ///
    /// PostgreSQL and SQLite name that row `excluded`; MySQL 8.0.19 names it with a row
    /// alias, which the insert builder substitutes for `row` before rendering. Keeping the
    /// name in the node rather than hard-coding both spellings here is what lets one
    /// [`excluded`](fn@excluded) call site work on every dialect.
    UpsertRef {
        /// How the dialect names the incoming row: `excluded`, or MySQL's row alias.
        row: &'static str,
        /// The column to read.
        column: &'static str,
    },
    /// A value inside a JSON document, reached by a dotted path.
    JsonPath {
        expr: Operand,
        path: &'static str,
        /// `true` for the text form (`->>`), `false` for the JSON form (`->`).
        as_text: bool,
    },
    /// A full-text match, spelled however the dialect spells one.
    FullText {
        column: Operand,
        /// The search text, bound as a parameter so it never becomes SQL.
        query: Value,
    },
    /// `[NOT] EXISTS (subquery)`.
    Exists {
        /// The subquery, boxed because it is much larger than the other nodes.
        query: Box<Subquery>,
        /// `true` for `NOT EXISTS`.
        negated: bool,
    },
    /// `expr [NOT] IN (subquery)`.
    InSubquery {
        /// The value tested against the subquery's column.
        expr: Operand,
        /// The subquery, which must project exactly one column.
        query: Box<Subquery>,
        /// `true` for `NOT IN`.
        negated: bool,
    },
}

/// A child of an expression node.
///
/// Almost every expression in a real query is a column compared with a literal, and a
/// `Box<ExprKind>` for each side of that comparison was two heap allocations per `WHERE`
/// clause purely to hold a table name and a `Value`. The two common cases are stored inline
/// and only anything genuinely nested is boxed.
#[derive(Debug, Clone, PartialEq)]
#[doc(hidden)]
pub enum Operand {
    /// A plain column reference.
    Column {
        /// The table the column belongs to, or `""` when it is unqualified.
        table: &'static str,
        /// The column name.
        column: &'static str,
    },
    /// A bound parameter.
    Value(Value),
    /// Anything else, boxed because the node is larger than the pointer that holds it.
    Expr(Box<ExprKind>),
}

impl Operand {
    /// Wrap an expression node, keeping the two common shapes inline.
    pub(crate) fn new(kind: ExprKind) -> Self {
        match kind {
            ExprKind::Column { table, column } => Self::Column { table, column },
            ExprKind::Value(value) => Self::Value(value),
            other => Self::Expr(Box::new(other)),
        }
    }

    /// Render this operand into `out`.
    pub(crate) fn render<D: Dialect>(&mut self, out: &mut Sql<D>) {
        match self {
            Self::Column { table, column } => {
                if table.is_empty() {
                    out.push_ident(column);
                } else {
                    out.push_qualified(table, column);
                }
            }
            Self::Value(value) => out.bind_value(core::mem::replace(value, Value::Null)),
            Self::Expr(kind) => kind.render(out),
        }
    }

    /// Remove the table name from a column reference.
    fn unqualify(&mut self) {
        match self {
            Self::Column { table, .. } => *table = "",
            Self::Expr(kind) => kind.unqualify(),
            Self::Value(_) => {}
        }
    }

    /// Whether this operand reads a column of the row being inserted.
    fn reads_upsert_row(&self) -> bool {
        match self {
            Self::Expr(kind) => kind.reads_upsert_row(),
            Self::Column { .. } | Self::Value(_) => false,
        }
    }

    /// Point every reference to the incoming row at `row`.
    fn set_upsert_row(&mut self, row: &'static str) {
        if let Self::Expr(kind) = self {
            kind.set_upsert_row(row);
        }
    }
}

impl ExprKind {
    /// Render this expression into `out`.
    ///
    /// Takes `&mut self` because rendering *moves* the bound values into the statement
    /// instead of cloning them: a text parameter is a `String`, and the clone was an
    /// allocation per parameter on the hot path. A builder that only has a borrow (such as
    /// `to_sql`, which is meant to leave the query usable) clones the query once and then
    /// moves out of the copy.
    pub(crate) fn render<D: Dialect>(&mut self, out: &mut Sql<D>) {
        match self {
            Self::Column { table, column } => {
                if table.is_empty() {
                    out.push_ident(column);
                } else {
                    out.push_qualified(table, column);
                }
            }
            Self::Value(value) => out.bind_value(core::mem::replace(value, Value::Null)),
            Self::Star => out.push_raw("*"),
            Self::Raw(sql) => out.push_raw(sql),
            Self::Binary { op, left, right } => {
                if matches!(*op, "ILIKE" | "NOT ILIKE") && !D::SUPPORTS_ILIKE {
                    out.set_error(besu_core::Error::unsupported(alloc::format!(
                        "`{}` has no `ILIKE`; use `LIKE` with a lowercased column instead",
                        D::NAME
                    )));
                }
                out.push_raw("(");
                left.render(out);
                out.push_raw(" ");
                out.push_raw(op);
                out.push_raw(" ");
                right.render(out);
                out.push_raw(")");
            }
            Self::Prefix { op, expr } => {
                out.push_raw("(");
                out.push_raw(op);
                out.push_raw(" ");
                expr.render(out);
                out.push_raw(")");
            }
            Self::Postfix { op, expr } => {
                out.push_raw("(");
                expr.render(out);
                out.push_raw(" ");
                out.push_raw(op);
                out.push_raw(")");
            }
            Self::Function {
                name,
                args,
                distinct,
            } => {
                out.push_raw(name);
                out.push_raw("(");
                if *distinct {
                    out.push_raw("DISTINCT ");
                }
                for (i, arg) in args.iter_mut().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    arg.render(out);
                }
                out.push_raw(")");
            }
            Self::Between {
                expr,
                low,
                high,
                negated,
            } => {
                out.push_raw("(");
                expr.render(out);
                out.push_raw(if *negated {
                    " NOT BETWEEN "
                } else {
                    " BETWEEN "
                });
                low.render(out);
                out.push_raw(" AND ");
                high.render(out);
                out.push_raw(")");
            }
            Self::InList {
                expr,
                list,
                negated,
            } => {
                if list.is_empty() {
                    // `IN ()` is a syntax error in every SQL dialect, but an empty list has
                    // an obvious meaning. `1 = 0` is false and `1 = 1` is true, both are
                    // portable, and every planner folds the constant away, so the plan is
                    // the same as a hand-written `WHERE false`.
                    out.push_raw(if *negated { "(1 = 1)" } else { "(1 = 0)" });
                } else {
                    out.push_raw("(");
                    expr.render(out);
                    out.push_raw(if *negated { " NOT IN (" } else { " IN (" });
                    for (i, item) in list.iter_mut().enumerate() {
                        if i != 0 {
                            out.push_raw(", ");
                        }
                        item.render(out);
                    }
                    out.push_raw("))");
                }
            }
            Self::Case {
                operand,
                branches,
                fallback,
            } => {
                out.push_raw("(CASE");
                if let Some(operand) = operand {
                    out.push_raw(" ");
                    operand.render(out);
                }
                for (when, then) in branches.iter_mut() {
                    out.push_raw(" WHEN ");
                    when.render(out);
                    out.push_raw(" THEN ");
                    then.render(out);
                }
                if let Some(fallback) = fallback {
                    out.push_raw(" ELSE ");
                    fallback.render(out);
                }
                out.push_raw(" END)");
            }
            Self::Cast { expr, ty } => {
                out.push_raw("CAST(");
                expr.render(out);
                out.push_raw(" AS ");
                out.push_raw(&D::cast_type_name(ty.clone()));
                out.push_raw(")");
            }
            Self::UpsertRef { row, column } => {
                // The row name is quoted rather than written bare so a row alias the caller
                // chose cannot turn into SQL. `"excluded"` is the same identifier as
                // `excluded` on PostgreSQL and SQLite.
                out.push_ident(row);
                out.push_raw(".");
                out.push_ident(column);
            }
            Self::JsonPath {
                expr,
                path,
                as_text,
            } => {
                use besu_core::JsonPathStyle;
                match D::JSON_PATH {
                    JsonPathStyle::Postgres => {
                        expr.render(out);
                        out.push_raw(if *as_text { " #>> '{" } else { " #> '{" });
                        for (i, segment) in path.split('.').enumerate() {
                            if i != 0 {
                                out.push_raw(",");
                            }
                            out.push_raw(segment.replace('\'', "''").as_str());
                        }
                        out.push_raw("}'");
                    }
                    JsonPathStyle::Sqlite => {
                        // `->` and `->>` have been available since SQLite 3.38 and keep the
                        // JSON/text distinction that `json_extract` does not: it returns a
                        // native scalar, which cannot decode as JSON.
                        expr.render(out);
                        out.push_raw(if *as_text { " ->> '$." } else { " -> '$." });
                        out.push_raw(path);
                        out.push_raw("'");
                    }
                    JsonPathStyle::Mysql => {
                        if *as_text {
                            out.push_raw("JSON_UNQUOTE(");
                        }
                        out.push_raw("JSON_EXTRACT(");
                        expr.render(out);
                        out.push_raw(", '$.");
                        out.push_raw(path);
                        // The path is a quoted string literal, so it needs its closing
                        // quote — the first version of this arm simply forgot it.
                        out.push_raw("'");
                        out.push_raw(")");
                        if *as_text {
                            out.push_raw(")");
                        }
                    }
                }
            }
            Self::FullText { column, query } => {
                use besu_core::dialect::FullTextStyle;
                match D::FULL_TEXT {
                    FullTextStyle::Postgres => {
                        // `plainto_tsquery` takes the search text rather than a `tsquery`
                        // literal, so the query parses from a plain bound string and there is
                        // no syntax for the caller to get wrong.
                        out.push_raw("to_tsvector('english', ");
                        column.render(out);
                        out.push_raw(") @@ plainto_tsquery('english', ");
                        out.bind_value(core::mem::replace(query, Value::Null));
                        out.push_raw(")");
                    }
                    FullTextStyle::Mysql => {
                        out.push_raw("MATCH(");
                        column.render(out);
                        out.push_raw(") AGAINST (");
                        out.bind_value(core::mem::replace(query, Value::Null));
                        out.push_raw(" IN BOOLEAN MODE)");
                    }
                    FullTextStyle::Sqlite => {
                        column.render(out);
                        out.push_raw(" MATCH ");
                        out.bind_value(core::mem::replace(query, Value::Null));
                    }
                    FullTextStyle::Unsupported => {
                        // The error is what the caller sees; the text still has to be
                        // well-formed so `inspect` and the query log stay readable.
                        out.set_error(besu_core::Error::unsupported(alloc::format!(
                            "`{}` has no full-text match; write it with `raw`, or query a \
                             dialect that does",
                            D::NAME
                        )));
                        column.render(out);
                        out.push_raw(" MATCH ");
                        out.bind_value(core::mem::replace(query, Value::Null));
                    }
                }
            }
            Self::Exists { query, negated } => {
                out.push_raw(if *negated { "NOT EXISTS (" } else { "EXISTS (" });
                // Rendered into the same statement, so its parameters continue the outer
                // statement's numbering, and the dialect is the outer one by construction.
                query.query.render_into(out);
                out.push_raw(")");
            }
            Self::InSubquery {
                expr,
                query,
                negated,
            } => {
                let columns = query.query.projection_len();
                if columns != 1 {
                    out.set_error(besu_core::Error::invalid_query(alloc::format!(
                        "an `IN (SELECT ...)` subquery must select exactly one column, but this \
                         one selects {columns}"
                    )));
                }
                out.push_raw("(");
                expr.render(out);
                out.push_raw(if *negated { " NOT IN (" } else { " IN (" });
                query.query.render_into(out);
                out.push_raw("))");
            }
        }
    }

    /// Remove the table name from every column reference inside this expression.
    ///
    /// A set query's `ORDER BY` may only name a result column: PostgreSQL rejects a
    /// qualified name there with *"missing FROM-clause entry"*, and MySQL rejects it with
    /// *"cannot be used in global ORDER clause"*. Writing `"age"` instead of
    /// `"users"."age"` is the portable spelling, and it still means the same column because
    /// the compound's output columns take their names from the first branch.
    pub(crate) fn unqualify(&mut self) {
        match self {
            Self::Column { table, .. } => *table = "",
            Self::Binary { left, right, .. } => {
                left.unqualify();
                right.unqualify();
            }
            Self::Prefix { expr, .. } | Self::Postfix { expr, .. } => expr.unqualify(),
            Self::Function { args, .. } => {
                for arg in args {
                    arg.unqualify();
                }
            }
            Self::Between {
                expr, low, high, ..
            } => {
                expr.unqualify();
                low.unqualify();
                high.unqualify();
            }
            Self::InList { expr, list, .. } => {
                expr.unqualify();
                for item in list {
                    item.unqualify();
                }
            }
            Self::Case {
                operand,
                branches,
                fallback,
            } => {
                if let Some(operand) = operand {
                    operand.unqualify();
                }
                for (when, then) in branches {
                    when.unqualify();
                    then.unqualify();
                }
                if let Some(fallback) = fallback {
                    fallback.unqualify();
                }
            }
            Self::Cast { expr, .. }
            | Self::JsonPath { expr, .. }
            | Self::InSubquery { expr, .. } => expr.unqualify(),
            // `FullText` needs a column qualified only when a query has more than one table,
            // and a compound `ORDER BY` never does.
            Self::FullText { column, .. } => column.unqualify(),
            // Nothing to rewrite: a value has no table, a nested `EXISTS` already
            // contains only what its own tables provide, and the incoming row of an upsert
            // is named by the statement rather than by the row set.
            Self::Value(_)
            | Self::Raw(_)
            | Self::Star
            | Self::Exists { .. }
            | Self::UpsertRef { .. } => {}
        }
    }

    /// Whether this expression reads a column of the row being inserted.
    ///
    /// MySQL only accepts a row alias on an `INSERT` whose `ON DUPLICATE KEY UPDATE` mentions
    /// it, so the insert builder asks before writing the alias at all.
    pub(crate) fn reads_upsert_row(&self) -> bool {
        match self {
            Self::UpsertRef { .. } => true,
            Self::Binary { left, right, .. } => left.reads_upsert_row() || right.reads_upsert_row(),
            Self::Prefix { expr, .. } | Self::Postfix { expr, .. } => expr.reads_upsert_row(),
            Self::Function { args, .. } => args.iter().any(Self::reads_upsert_row),
            Self::Between {
                expr, low, high, ..
            } => expr.reads_upsert_row() || low.reads_upsert_row() || high.reads_upsert_row(),
            Self::InList { expr, list, .. } => {
                expr.reads_upsert_row() || list.iter().any(Self::reads_upsert_row)
            }
            Self::Case {
                operand,
                branches,
                fallback,
            } => {
                operand.as_ref().is_some_and(Operand::reads_upsert_row)
                    || branches
                        .iter()
                        .any(|(when, then)| when.reads_upsert_row() || then.reads_upsert_row())
                    || fallback.as_ref().is_some_and(Operand::reads_upsert_row)
            }
            Self::Cast { expr, .. }
            | Self::JsonPath { expr, .. }
            | Self::InSubquery { expr, .. } => expr.reads_upsert_row(),
            Self::FullText { column, .. } => column.reads_upsert_row(),
            Self::Column { .. }
            | Self::Value(_)
            | Self::Star
            | Self::Raw(_)
            | Self::Exists { .. } => false,
        }
    }

    /// Point every reference to the incoming row at `row`.
    ///
    /// MySQL's row alias is chosen per statement, so the name cannot be fixed when
    /// [`excluded`](fn@excluded) builds the node; the insert builder rewrites it once it
    /// knows what the statement will call the row.
    pub(crate) fn set_upsert_row(&mut self, row: &'static str) {
        match self {
            Self::UpsertRef { row: name, .. } => *name = row,
            Self::Binary { left, right, .. } => {
                left.set_upsert_row(row);
                right.set_upsert_row(row);
            }
            Self::Prefix { expr, .. } | Self::Postfix { expr, .. } => expr.set_upsert_row(row),
            Self::Function { args, .. } => {
                for arg in args {
                    arg.set_upsert_row(row);
                }
            }
            Self::Between {
                expr, low, high, ..
            } => {
                expr.set_upsert_row(row);
                low.set_upsert_row(row);
                high.set_upsert_row(row);
            }
            Self::InList { expr, list, .. } => {
                expr.set_upsert_row(row);
                for item in list {
                    item.set_upsert_row(row);
                }
            }
            Self::Case {
                operand,
                branches,
                fallback,
            } => {
                if let Some(operand) = operand {
                    operand.set_upsert_row(row);
                }
                for (when, then) in branches {
                    when.set_upsert_row(row);
                    then.set_upsert_row(row);
                }
                if let Some(fallback) = fallback {
                    fallback.set_upsert_row(row);
                }
            }
            Self::Cast { expr, .. }
            | Self::JsonPath { expr, .. }
            | Self::InSubquery { expr, .. } => expr.set_upsert_row(row),
            Self::FullText { column, .. } => column.set_upsert_row(row),
            Self::Column { .. }
            | Self::Value(_)
            | Self::Star
            | Self::Raw(_)
            | Self::Exists { .. } => {}
        }
    }
}

/// A typed SQL expression.
///
/// `T` is the row set the expression belongs to and `S` is the SQL type it produces. Both
/// are erased before SQL is rendered; they only exist to make invalid queries fail to
/// compile.
pub struct Expr<T, S: SqlType = Bool> {
    pub(crate) kind: ExprKind,
    phantom: PhantomData<fn() -> (T, S)>,
}

impl<T, S: SqlType> Expr<T, S> {
    pub(crate) fn from_kind(kind: ExprKind) -> Self {
        Self {
            kind,
            phantom: PhantomData,
        }
    }

    pub(crate) fn into_kind(self) -> ExprKind {
        self.kind
    }

    /// A raw SQL expression. The string is used verbatim, so it must never contain user
    /// input. Prefer the typed helpers.
    pub fn raw(sql: impl Into<String>) -> Self {
        Self::from_kind(ExprKind::Raw(sql.into()))
    }

    /// `CAST(this AS S2)`.
    pub fn cast<S2: SqlType>(self) -> Expr<T, S2> {
        Expr::from_kind(ExprKind::Cast {
            expr: Operand::new(self.into_kind()),
            ty: S2::DEF,
        })
    }

    expr_comparisons! {
        /// `=` comparison.
        fn eq => "=",
        /// `<>` comparison.
        fn ne => "<>",
        /// `>` comparison.
        fn gt => ">",
        /// `>=` comparison.
        fn gte => ">=",
        /// `<` comparison.
        fn lt => "<",
        /// `<=` comparison.
        fn lte => "<=",
        /// `LIKE` comparison.
        fn like => "LIKE",
        /// `NOT LIKE` comparison.
        fn not_like => "NOT LIKE",
        /// `ILIKE` comparison.
        fn ilike => "ILIKE",
        /// `NOT ILIKE` comparison.
        fn not_ilike => "NOT ILIKE",
    }

    /// `this + other`.
    pub fn plus<A, M>(self, other: A) -> Expr<T, S>
    where
        A: IntoExpr<T, S, M>,
    {
        self.binary("+", other.into_expr().into_kind())
    }

    /// `this - other`.
    pub fn minus<A, M>(self, other: A) -> Expr<T, S>
    where
        A: IntoExpr<T, S, M>,
    {
        self.binary("-", other.into_expr().into_kind())
    }

    /// `this * other`.
    pub fn times<A, M>(self, other: A) -> Expr<T, S>
    where
        A: IntoExpr<T, S, M>,
    {
        self.binary("*", other.into_expr().into_kind())
    }

    /// `this / other`.
    pub fn divide<A, M>(self, other: A) -> Expr<T, S>
    where
        A: IntoExpr<T, S, M>,
    {
        self.binary("/", other.into_expr().into_kind())
    }

    /// `this % other`.
    pub fn modulo<A, M>(self, other: A) -> Expr<T, S>
    where
        A: IntoExpr<T, S, M>,
    {
        self.binary("%", other.into_expr().into_kind())
    }

    fn binary(self, op: &'static str, right: ExprKind) -> Self {
        Expr::from_kind(ExprKind::Binary {
            op,
            left: Operand::new(self.into_kind()),
            right: Operand::new(right),
        })
    }

    /// Sort ascending.
    pub fn asc(self) -> OrderClause {
        OrderClause {
            expr: self.into_kind(),
            direction: Order::Asc,
            nulls: None,
        }
    }

    /// Sort descending.
    pub fn desc(self) -> OrderClause {
        OrderClause {
            expr: self.into_kind(),
            direction: Order::Desc,
            nulls: None,
        }
    }

    /// Place `NULL`s first in the sort order.
    pub fn nulls_first(self) -> OrderClause {
        OrderClause {
            expr: self.into_kind(),
            direction: Order::Asc,
            nulls: Some(NullsOrder::First),
        }
    }

    /// Place `NULL`s last in the sort order.
    pub fn nulls_last(self) -> OrderClause {
        OrderClause {
            expr: self.into_kind(),
            direction: Order::Asc,
            nulls: Some(NullsOrder::Last),
        }
    }
}

impl<T, S: SqlType> Clone for Expr<T, S> {
    fn clone(&self) -> Self {
        Self::from_kind(self.kind.clone())
    }
}

impl<T, S: SqlType> core::fmt::Debug for Expr<T, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Expr").field("kind", &self.kind).finish()
    }
}

// ---------------------------------------------------------------------------------------
// Turning things into expressions
// ---------------------------------------------------------------------------------------

/// Internal marker: the value came from a [`Column`].
///
/// The path parameter is what proves the column's table is in the query. Because it lives in
/// the marker rather than in a separate bound, a *literal* on the other side of a comparison
/// simply cannot be mistaken for a column that needs checking — which is what lets both
/// operands be checked.
pub struct ColumnMarker<P>(PhantomData<P>);

/// Internal marker: a boolean column used directly as a condition.
pub struct ColumnCondition<P>(PhantomData<P>);

/// Internal marker: the value is already an [`Expr`].
pub enum ExprMarker {}
/// Internal marker: the value is a plain literal.
pub enum ValueMarker {}
/// Internal marker: the value is an `Option`, so it may bind `NULL`.
pub enum OptionMarker {}

/// Anything that can be used where a SQL expression is expected.
///
/// The `M` parameter only exists to keep the implementations from overlapping; it is always
/// inferred.
pub trait IntoExpr<T, S: SqlType, M> {
    /// Convert into an expression.
    fn into_expr(self) -> Expr<T, S>;
}

impl<RowSet, T: Table, S: SqlType, N: Nullability, P> IntoExpr<RowSet, S, ColumnMarker<P>>
    for Column<T, S, N>
where
    RowSet: Contains<P, T>,
{
    fn into_expr(self) -> Expr<RowSet, S> {
        Expr::from_kind(ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        })
    }
}

impl<T, S: SqlType> IntoExpr<T, S, ExprMarker> for Expr<T, S> {
    fn into_expr(self) -> Expr<T, S> {
        self
    }
}

/// A Rust value that can be bound as a SQL parameter of a particular type.
///
/// This is implemented per `(Rust type, SQL type)` pair rather than as a blanket over
/// `Into<S::Rust>`. The difference matters for inference: with a blanket, the compiler can
/// never rule out that a `Column` could be a value, so `eq(users.id, posts.author_id)` would
/// be ambiguous. An explicit list keeps every call site inferable.
///
/// Implement this for your own types to use them as parameters.
pub trait IntoSqlValue<S: SqlType> {
    /// Convert into a bound value.
    fn into_sql_value(self) -> Value;
}

macro_rules! numeric_values {
    ($ty:ty, [$($target:ty),* $(,)?]) => {
        $(
            impl IntoSqlValue<$target> for $ty {
                fn into_sql_value(self) -> Value {
                    Value::from(self as <$target as SqlType>::Rust)
                }
            }
        )*
    };
}

numeric_values!(i8, [Int1, Int2, Int4, Int8, Float4, Float8]);
numeric_values!(i16, [Int2, Int4, Int8, Float4, Float8]);
numeric_values!(i32, [Int4, Int8, Float8]);
numeric_values!(i64, [Int8]);
numeric_values!(u8, [UInt1, Int2, Int4, Int8]);
numeric_values!(u16, [UInt2, Int4, Int8]);
numeric_values!(u32, [UInt4, Int8]);
numeric_values!(u64, [UInt8]);
numeric_values!(u128, [U128]);
numeric_values!(i128, [I128]);
numeric_values!(f32, [Float4, Float8]);
numeric_values!(f64, [Float8]);

impl IntoSqlValue<Bool> for bool {
    fn into_sql_value(self) -> Value {
        Value::Bool(self)
    }
}

macro_rules! text_values {
    ($ty:ty) => {
        impl IntoSqlValue<Text> for $ty {
            fn into_sql_value(self) -> Value {
                Value::Text(self.into())
            }
        }

        impl<const N: u32> IntoSqlValue<VarChar<N>> for $ty {
            fn into_sql_value(self) -> Value {
                Value::Text(self.into())
            }
        }

        impl<const N: u32> IntoSqlValue<Char<N>> for $ty {
            fn into_sql_value(self) -> Value {
                Value::Text(self.into())
            }
        }

        impl IntoSqlValue<Decimal<0, 0>> for $ty {
            fn into_sql_value(self) -> Value {
                Value::Text(self.into())
            }
        }
    };
}

text_values!(&str);
text_values!(String);
text_values!(&String);
text_values!(alloc::borrow::Cow<'_, str>);

impl IntoSqlValue<Bytes> for Vec<u8> {
    fn into_sql_value(self) -> Value {
        Value::Bytes(self)
    }
}

impl IntoSqlValue<Bytes> for &[u8] {
    fn into_sql_value(self) -> Value {
        Value::Bytes(self.into())
    }
}

impl IntoSqlValue<Uuid> for uuid::Uuid {
    fn into_sql_value(self) -> Value {
        Value::Uuid(self)
    }
}

impl IntoSqlValue<Date> for chrono::NaiveDate {
    fn into_sql_value(self) -> Value {
        Value::Date(self)
    }
}

impl IntoSqlValue<Time> for chrono::NaiveTime {
    fn into_sql_value(self) -> Value {
        Value::Time(self)
    }
}

impl IntoSqlValue<Timestamp> for chrono::NaiveDateTime {
    fn into_sql_value(self) -> Value {
        Value::Timestamp(self)
    }
}

impl IntoSqlValue<TimestampTz> for chrono::DateTime<chrono::Utc> {
    fn into_sql_value(self) -> Value {
        Value::TimestampTz(self)
    }
}

impl IntoSqlValue<Json> for serde_json::Value {
    fn into_sql_value(self) -> Value {
        Value::Json(self.to_string().into_bytes())
    }
}

impl IntoSqlValue<Jsonb> for serde_json::Value {
    fn into_sql_value(self) -> Value {
        Value::Json(self.to_string().into_bytes())
    }
}

impl<T, S: SqlType, V: IntoSqlValue<S>> IntoExpr<T, S, ValueMarker> for V {
    fn into_expr(self) -> Expr<T, S> {
        Expr::from_kind(ExprKind::Value(self.into_sql_value()))
    }
}

impl<T, S: SqlType, V: IntoSqlValue<S>> IntoExpr<T, S, OptionMarker> for Option<V> {
    fn into_expr(self) -> Expr<T, S> {
        Expr::from_kind(ExprKind::Value(match self {
            Some(value) => value.into_sql_value(),
            None => Value::Null,
        }))
    }
}

/// Two column types must match for a comparison.
///
/// `(A, B): SameType` only holds when the two are equal, so the compiler can *infer* the
/// right-hand type from the left-hand one. `eq(users.id, 5)` therefore checks `5` as an
/// `i64` — a mistake like `eq(users.id, "five")` is a compile error rather than a silent
/// coercion or a runtime failure.
pub trait SameType {}

impl<T> SameType for (T, T) {}

// ---------------------------------------------------------------------------------------
// Comparisons
// ---------------------------------------------------------------------------------------

macro_rules! comparisons {
    ($( $(#[$attr:meta])* $name:ident => $op:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<RowSet, SA, SB, MA, MB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
            where
                SA: SqlType,
                SB: SqlType,
                A: IntoExpr<RowSet, SA, MA>,
                B: IntoExpr<RowSet, SB, MB>,
                (SA, SB): SameType,
            {
                Expr::from_kind(ExprKind::Binary {
                    op: $op,
                    left: Operand::new(a.into_expr().into_kind()),
                    right: Operand::new(b.into_expr().into_kind()),
                })
            }
        )*
    };
}

comparisons! {
    /// `a = b`.
    eq => "=",
    /// `a <> b`.
    ne => "<>",
    /// `a > b`.
    gt => ">",
    /// `a >= b`.
    gte => ">=",
    /// `a < b`.
    lt => "<",
    /// `a <= b`.
    lte => "<=",
    /// `a LIKE b`. The pattern is bound as a parameter, so `%` and `_` behave as SQL
    /// wildcards without any string interpolation.
    like => "LIKE",
    /// `a NOT LIKE b`.
    not_like => "NOT LIKE",
    /// `a ILIKE b`. PostgreSQL only.
    ilike => "ILIKE",
    /// `a NOT ILIKE b`. PostgreSQL only.
    not_ilike => "NOT ILIKE",
}

/// `a IS NULL`.
pub fn is_null<RowSet, A, MA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS NULL",
        expr: Operand::new(a.into_typed().into_kind()),
    })
}

/// `a IS NOT NULL`.
pub fn is_not_null<RowSet, A, MA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS NOT NULL",
        expr: Operand::new(a.into_typed().into_kind()),
    })
}

/// `a IS TRUE`.
pub fn is_true<RowSet, A, MA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS TRUE",
        expr: Operand::new(a.into_typed().into_kind()),
    })
}

/// `a IS FALSE`.
pub fn is_false<RowSet, A, MA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS FALSE",
        expr: Operand::new(a.into_typed().into_kind()),
    })
}

/// `a IN (values...)`.
///
/// An empty list is rendered as the false constant rather than the invalid `IN ()`, so a
/// list built from a collection that happens to be empty is a query that runs and returns
/// no rows.
pub fn in_list<RowSet, A, I, V, MA>(a: A, values: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    I: IntoIterator<Item = V>,
    V: IntoSqlValue<A::SqlType>,
{
    Expr::from_kind(ExprKind::InList {
        expr: Operand::new(a.into_typed().into_kind()),
        list: values
            .into_iter()
            .map(|v| ExprKind::Value(v.into_sql_value()))
            .collect(),
        negated: false,
    })
}

/// `a NOT IN (values...)`.
///
/// An empty list is rendered as the true constant, because a value is not in nothing.
pub fn not_in<RowSet, A, I, V, MA>(a: A, values: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    I: IntoIterator<Item = V>,
    V: IntoSqlValue<A::SqlType>,
{
    Expr::from_kind(ExprKind::InList {
        expr: Operand::new(a.into_typed().into_kind()),
        list: values
            .into_iter()
            .map(|v| ExprKind::Value(v.into_sql_value()))
            .collect(),
        negated: true,
    })
}

/// `a BETWEEN low AND high`.
pub fn between<RowSet, A, V, MA>(a: A, low: V, high: V) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::Between {
        expr: Operand::new(a.into_typed().into_kind()),
        low: Operand::new(ExprKind::Value(low.into())),
        high: Operand::new(ExprKind::Value(high.into())),
        negated: false,
    })
}

/// `a NOT BETWEEN low AND high`.
pub fn not_between<RowSet, A, V, MA>(a: A, low: V, high: V) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::Between {
        expr: Operand::new(a.into_typed().into_kind()),
        low: Operand::new(ExprKind::Value(low.into())),
        high: Operand::new(ExprKind::Value(high.into())),
        negated: true,
    })
}

// ---------------------------------------------------------------------------------------
// Boolean combinators
// ---------------------------------------------------------------------------------------

/// Anything usable as a `WHERE` condition.
///
/// Both `Expr<T, Bool>` and a boolean column work, so `filter(users.active)` and
/// `filter(users.active.eq(true))` are both valid.
pub trait IntoCondition<T, M> {
    /// Convert into a boolean expression.
    fn into_condition(self) -> ExprKind;
}

impl<T> IntoCondition<T, ExprMarker> for Expr<T, Bool> {
    fn into_condition(self) -> ExprKind {
        self.into_kind()
    }
}

impl<RowSet, T: Table, N: Nullability, P> IntoCondition<RowSet, ColumnCondition<P>>
    for Column<T, Bool, N>
where
    RowSet: Contains<P, T>,
{
    fn into_condition(self) -> ExprKind {
        ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        }
    }
}

/// `a AND b`.
pub fn and<RowSet, PA, PB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoCondition<RowSet, PA>,
    B: IntoCondition<RowSet, PB>,
{
    Expr::from_kind(ExprKind::Binary {
        op: "AND",
        left: Operand::new(a.into_condition()),
        right: Operand::new(b.into_condition()),
    })
}

/// `a OR b`.
pub fn or<RowSet, PA, PB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoCondition<RowSet, PA>,
    B: IntoCondition<RowSet, PB>,
{
    Expr::from_kind(ExprKind::Binary {
        op: "OR",
        left: Operand::new(a.into_condition()),
        right: Operand::new(b.into_condition()),
    })
}

/// `NOT a`.
pub fn not<RowSet, PA, A>(a: A) -> Expr<RowSet, Bool>
where
    A: IntoCondition<RowSet, PA>,
{
    Expr::from_kind(ExprKind::Prefix {
        op: "NOT",
        expr: Operand::new(a.into_condition()),
    })
}

// ---------------------------------------------------------------------------------------
// Subqueries
// ---------------------------------------------------------------------------------------

/// A `SELECT` nested inside another statement.
///
/// This is the erased form of a [`Select`](crate::Select): the row set and row type are not
/// part of it, because the statement that contains it never decodes the subquery's rows. It
/// is rendered by the enclosing statement's dialect and shares that statement's parameter
/// numbering.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct Subquery {
    pub(crate) query: crate::query::SelectQuery,
}

/// A finished `SELECT` that can be nested inside another statement.
///
/// Implemented for [`Select`](crate::Select), which is what makes
/// `exists(db.select(posts).filter(..))` read like the SQL it produces. The executor is
/// dropped when the query is nested: the subquery is rendered by the statement that contains
/// it, so it shares that statement's dialect and its parameters continue that statement's
/// numbering.
///
/// The row set of a nested query is not constrained against the enclosing query's, because
/// the compiler cannot express "the tables this subquery borrows from the outer query are
/// in the outer query" without the outer query's type — which is not in scope at the point
/// the subquery is built. Correlating requires [`Select::correlate`](crate::Select::correlate)
/// to put the outer table in the subquery's row set, which is what makes the reference
/// compile; the reverse check is left to the database.
pub trait IntoSubquery {
    /// The row type the subquery produces.
    ///
    /// `in_query` uses it to require the subquery to produce one value of the column's Rust
    /// type, which is as close to comparing SQL types as `pick` allows.
    type Row;

    /// Take the finished query.
    fn into_subquery(self) -> Subquery;
}

impl<'a, E: Executor, M, R> IntoSubquery for crate::query::Select<'a, E, M, R> {
    type Row = R;

    fn into_subquery(self) -> Subquery {
        Subquery { query: self.query }
    }
}

/// `EXISTS (subquery)`: whether the subquery returns at least one row.
///
/// ```ignore
/// let with_posts = db
///     .select(users)
///     .filter(exists(
///         db.select(posts)
///             .correlate(users)
///             .filter(posts.author_id.eq(users.id)),
///     ))
///     .await?;
/// ```
///
/// ```sql
/// SELECT ... FROM "users" WHERE EXISTS (
///     SELECT ... FROM "posts" WHERE ("posts"."author_id" = "users"."id")
/// )
/// ```
///
/// A correlated subquery references the outer query's columns, so the subquery has to be
/// told which outer tables it may use: that is what
/// [`correlate`](crate::Select::correlate) does. Without it the reference to `users` inside
/// the subquery is a compile error, because `users` is not in the subquery's own row set.
///
/// What the subquery selects is irrelevant to `EXISTS` — only whether a row exists — so the
/// projection is not restricted and typically stays the default one.
pub fn exists<RowSet, Q: IntoSubquery>(query: Q) -> Expr<RowSet, Bool> {
    Expr::from_kind(ExprKind::Exists {
        query: Box::new(query.into_subquery()),
        negated: false,
    })
}

/// `NOT EXISTS (subquery)`: whether the subquery returns no row at all.
///
/// This is the anti-join spelling: `NOT EXISTS (...)` is null-safe in a way that
/// `NOT IN (...)` is not, because a `NULL` in an `IN` list makes the whole comparison
/// unknown. Prefer it when the subquery can produce `NULL`.
pub fn not_exists<RowSet, Q: IntoSubquery>(query: Q) -> Expr<RowSet, Bool> {
    Expr::from_kind(ExprKind::Exists {
        query: Box::new(query.into_subquery()),
        negated: true,
    })
}

// ---------------------------------------------------------------------------------------
// Expressions with a known type
// ---------------------------------------------------------------------------------------

/// Something that is already an expression of a known SQL type.
///
/// This is the counterpart to [`IntoExpr`]: [`IntoExpr`] leaves the SQL type open so the
/// compiler can coerce a literal to the column it is compared against, whereas this trait
/// fixes it. Functions like [`sum`] need the fixed form, because they choose their own
/// result type and would otherwise have nothing to infer the argument's type from.
pub trait TypedExpr<RowSet, M> {
    /// The SQL type this expression produces.
    type SqlType: SqlType;

    /// Convert into an expression.
    fn into_typed(self) -> Expr<RowSet, Self::SqlType>;
}

impl<RowSet, T: Table, S: SqlType, N: Nullability, P> TypedExpr<RowSet, ColumnMarker<P>>
    for Column<T, S, N>
where
    RowSet: Contains<P, T>,
{
    type SqlType = S;

    fn into_typed(self) -> Expr<RowSet, S> {
        Expr::from_kind(ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        })
    }
}

impl<RowSet, S: SqlType> TypedExpr<RowSet, ExprMarker> for Expr<RowSet, S> {
    type SqlType = S;

    fn into_typed(self) -> Expr<RowSet, S> {
        self
    }
}

fn call<T, S: SqlType>(name: &'static str, args: Vec<ExprKind>) -> Expr<T, S> {
    Expr::from_kind(ExprKind::Function {
        name,
        args,
        distinct: false,
    })
}

macro_rules! unary_to {
    ($( $(#[$attr:meta])* $name:ident -> $out:ty = $sql:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<RowSet, A, MA>(value: A) -> Expr<RowSet, $out>
            where
                A: TypedExpr<RowSet, MA>,
            {
                call($sql, alloc::vec![value.into_typed().into_kind()])
            }
        )*
    };
}

macro_rules! unary_same {
    ($( $(#[$attr:meta])* $name:ident = $sql:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<RowSet, A, MA>(value: A) -> Expr<RowSet, A::SqlType>
            where
                A: TypedExpr<RowSet, MA>,
            {
                call($sql, alloc::vec![value.into_typed().into_kind()])
            }
        )*
    };
}

unary_to! {
    /// `COUNT(expr)`.
    count -> Int8 = "COUNT",
    /// `LOWER(expr)`.
    lower -> Text = "LOWER",
    /// `UPPER(expr)`.
    upper -> Text = "UPPER",
    /// `LENGTH(expr)`.
    length -> Int8 = "LENGTH",
    /// `TRIM(expr)`.
    trim -> Text = "TRIM",
}

/// `SUM(expr)`, widened to a float because integer sums overflow.
///
/// The sum is cast to a double in the database rather than left as whatever the database
/// decides: PostgreSQL and MySQL return `NUMERIC` for a sum of integers, which no driver can
/// decode as `f64` without another dependency. Casting keeps the result type honest.
pub fn sum<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "SUM",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `AVG(expr)`, cast to a double for the same reason as [`sum`].
pub fn avg<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "AVG",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

fn cast_to_float<T, S: SqlType>(expr: Expr<T, S>) -> Expr<T, Float8> {
    Expr::from_kind(ExprKind::Cast {
        expr: Operand::new(expr.into_kind()),
        ty: Float8::DEF,
    })
}

unary_same! {
    /// `MIN(expr)`.
    min = "MIN",
    /// `MAX(expr)`.
    max = "MAX",
    /// `ABS(expr)`.
    abs = "ABS",
    /// `ROUND(expr)`.
    round = "ROUND",
    /// `CEIL(expr)`.
    ceil = "CEIL",
    /// `FLOOR(expr)`.
    floor = "FLOOR",
}

/// `COUNT(*)`.
pub fn count_all<T>() -> Expr<T, Int8> {
    call("COUNT", alloc::vec![ExprKind::Star])
}

/// `COUNT(DISTINCT expr)`.
pub fn count_distinct<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Function {
        name: "COUNT",
        args: alloc::vec![value.into_typed().into_kind()],
        distinct: true,
    })
}

/// `CONCAT(a, b)`.
pub fn concat<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
    B: TypedExpr<RowSet, MB>,
{
    call(
        "CONCAT",
        alloc::vec![a.into_typed().into_kind(), b.into_typed().into_kind()],
    )
}

/// `COALESCE(a, b)`, falling back to `b` when `a` is `NULL`.
pub fn coalesce<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "COALESCE",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// A `CASE` expression under construction.
///
/// The result type is a type parameter because a bare literal could equally be `TEXT`,
/// `VARCHAR(n)` or `CHAR(n)`. Write it when the compiler cannot infer it:
///
/// ```ignore
/// case::<_, Text>()
///     .when(users.age.gte(18), "adult")
///     .otherwise("minor")
/// ```
pub struct CaseBuilder<T, S: SqlType> {
    branches: Vec<(ExprKind, ExprKind)>,
    phantom: PhantomData<fn() -> (T, S)>,
}

/// Start a `CASE` expression.
pub fn case<T, S: SqlType>() -> CaseBuilder<T, S> {
    CaseBuilder {
        branches: Vec::new(),
        phantom: PhantomData,
    }
}

impl<T, S: SqlType> CaseBuilder<T, S> {
    /// `WHEN condition THEN value`.
    pub fn when<F, PF, V, M>(mut self, condition: F, value: V) -> Self
    where
        F: IntoCondition<T, PF>,
        V: IntoExpr<T, S, M>,
    {
        let _ = PhantomData::<(PF, M)>;
        self.branches
            .push((condition.into_condition(), value.into_expr().into_kind()));
        self
    }

    /// `ELSE value`, which finishes the expression.
    pub fn otherwise<V, M>(self, value: V) -> Expr<T, S>
    where
        V: IntoExpr<T, S, M>,
    {
        let _ = PhantomData::<M>;
        Expr::from_kind(ExprKind::Case {
            operand: None,
            branches: self.branches,
            fallback: Some(Operand::new(value.into_expr().into_kind())),
        })
    }
}

/// The JSON value at `path` inside `document`, keeping its JSON type.
///
/// `path` is a dotted key path — `"a.b"` reaches `document["a"]["b"]` — and must be a
/// literal, because it is rendered into the SQL rather than bound. Keys containing a dot are
/// not expressible; use [`raw`] for those.
pub fn json_get<RowSet, A, MA>(document: A, path: &'static str) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPath {
        expr: Operand::new(document.into_typed().into_kind()),
        path,
        as_text: false,
    })
}

/// The text at `path` inside `document`, unquoted.
///
/// This is `->>` rather than `->`: a JSON string comes back without its quotes, which is
/// almost always what you want when comparing against a `String` column or parameter.
pub fn json_text<RowSet, A, MA>(document: A, path: &'static str) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPath {
        expr: Operand::new(document.into_typed().into_kind()),
        path,
        as_text: true,
    })
}

/// A full-text match: `column` contains the search `query`.
///
/// The query is bound as a parameter, so untrusted input never becomes SQL and each database
/// parses it with its own search grammar. Every dialect spells the match differently:
///
/// | Dialect | Rendered |
/// | --- | --- |
/// | PostgreSQL | `to_tsvector('english', col) @@ plainto_tsquery('english', $1)` |
/// | MySQL | `MATCH(col) AGAINST ($1 IN BOOLEAN MODE)` |
/// | SQLite | `col MATCH $1` |
///
/// The differences go beyond spelling, so what the schema needs differs too. MySQL requires a
/// `FULLTEXT` index over exactly the matched columns. SQLite matches an FTS5 virtual table,
/// which the schema layer cannot declare; create it with [`sql!`](crate::sql) and select it
/// through a table handle. PostgreSQL needs nothing: `to_tsvector` builds the vector per row,
/// so an unindexed query is correct and merely slow.
///
/// PostgreSQL's text search configuration is fixed at `english` for now, which is why a
/// stemmed search such as `"borrow"` still finds `"borrowing"`. A parameter for it, and
/// ranking and highlighting alongside it, are the rest of
/// [#57](https://github.com/specta-rs/besu/issues/57).
pub fn matches<RowSet, A, MA>(column: A, query: impl Into<String>) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::FullText {
        column: Operand::new(column.into_typed().into_kind()),
        query: Value::Text(query.into()),
    })
}

/// The database's current timestamp.
pub fn now<T>() -> Expr<T, TimestampTz> {
    Expr::from_kind(ExprKind::Raw("CURRENT_TIMESTAMP".into()))
}

/// A raw SQL condition. The string is inserted verbatim, so never pass it user input.
pub fn raw<T>(sql: impl Into<String>) -> Expr<T, Bool> {
    Expr::from_kind(ExprKind::Raw(sql.into()))
}

/// A column of the row an upsert is trying to insert.
///
/// This is the row that would have been written had it not collided, and reading it is how
/// an upsert copies an incoming value into the existing row:
///
/// ```ignore
/// db.insert(users)
///     .value(users.insert("oscar@example.com", "Oscar", 21))
///     .on_conflict(users.email)
///     .do_update(users.name, excluded(users.name))
/// ```
///
/// `excluded` is the spelling the SQL standard and PostgreSQL give this pseudo-table, and it
/// is the one to write: on MySQL the same call renders through the statement's row alias
/// (`new."name"` by default, see [`Insert::row_alias`](crate::Insert::row_alias)) instead, so
/// one call site works on every database. [`new_row`] is the same function under MySQL's name.
///
/// The returned expression has no row set of its own, so it borrows the one its use implies:
/// it can only be used where the statement being built supplies it.
pub fn excluded<T: Table, S: SqlType, N: Nullability, RowSet>(
    column: Column<T, S, N>,
) -> Expr<RowSet, S> {
    Expr::from_kind(ExprKind::UpsertRef {
        row: "excluded",
        column: column.name(),
    })
}

/// A column of the row an upsert is trying to insert.
///
/// This is [`excluded`] under the name MySQL's documentation uses for the same thing: it
/// reads the incoming row, not the stored one. Both spellings render the dialect's own
/// syntax, so pick whichever reads better; `excluded` is the SQL standard's name.
pub fn new_row<T: Table, S: SqlType, N: Nullability, RowSet>(
    column: Column<T, S, N>,
) -> Expr<RowSet, S> {
    excluded(column)
}

// ---------------------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------------------

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// `ASC`
    Asc,
    /// `DESC`
    Desc,
}

/// Where `NULL`s sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullsOrder {
    /// `NULLS FIRST`
    First,
    /// `NULLS LAST`
    Last,
}

/// One `ORDER BY` term.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderClause {
    pub(crate) expr: ExprKind,
    pub(crate) direction: Order,
    pub(crate) nulls: Option<NullsOrder>,
}

impl OrderClause {
    /// Place `NULL`s first.
    pub fn nulls_first(mut self) -> Self {
        self.nulls = Some(NullsOrder::First);
        self
    }

    /// Place `NULL`s last.
    pub fn nulls_last(mut self) -> Self {
        self.nulls = Some(NullsOrder::Last);
        self
    }

    /// Reverse the direction, keeping any `NULLS` placement.
    pub fn reverse(self) -> Self {
        Self {
            direction: match self.direction {
                Order::Asc => Order::Desc,
                Order::Desc => Order::Asc,
            },
            ..self
        }
    }
}

/// Anything usable in `ORDER BY`.
pub trait IntoOrder {
    /// Convert into an order clause.
    fn into_order(self) -> OrderClause;
}

impl IntoOrder for OrderClause {
    fn into_order(self) -> OrderClause {
        self
    }
}

impl<T, S: SqlType> IntoOrder for Expr<T, S> {
    fn into_order(self) -> OrderClause {
        self.asc()
    }
}

impl<T: Table, S: SqlType, N: Nullability> IntoOrder for Column<T, S, N> {
    fn into_order(self) -> OrderClause {
        OrderClause {
            expr: ExprKind::Column {
                table: T::DEF.name,
                column: self.name(),
            },
            direction: Order::Asc,
            nulls: None,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Column conveniences
// ---------------------------------------------------------------------------------------

/// Comparison and ordering helpers, available directly on every column.
///
/// These read like the SQL they produce: `users.age.gt(18)` becomes `"users"."age" > $1`.
macro_rules! column_comparisons {
    ($( $(#[$attr:meta])* fn $name:ident => $op:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<RowSet, P1, B, MB>(self, other: B) -> Expr<RowSet, Bool>
            where
                RowSet: Contains<P1, T>,
                B: IntoExpr<RowSet, S, MB>,
            {
                let _ = PhantomData::<P1>;
                Expr::from_kind(ExprKind::Binary {
                    op: $op,
                    left: Operand::new(ExprKind::Column {
                        table: T::DEF.name,
                        column: self.name(),
                    }),
                    right: Operand::new(other.into_expr().into_kind()),
                })
            }
        )*
    };
}

impl<T: Table, S: SqlType, N: Nullability> Column<T, S, N> {
    column_comparisons! {
        /// `=` comparison.
        fn eq => "=",
        /// `<>` comparison.
        fn ne => "<>",
        /// `>` comparison.
        fn gt => ">",
        /// `>=` comparison.
        fn gte => ">=",
        /// `<` comparison.
        fn lt => "<",
        /// `<=` comparison.
        fn lte => "<=",
        /// `LIKE` comparison.
        fn like => "LIKE",
        /// `NOT LIKE` comparison.
        fn not_like => "NOT LIKE",
        /// `ILIKE` comparison.
        fn ilike => "ILIKE",
        /// `NOT ILIKE` comparison.
        fn not_ilike => "NOT ILIKE",
    }

    /// Full-text match against `query`, rendered per dialect; see the [`matches`](fn@matches) function.
    ///
    /// This is the method form, so the column is checked against the query's row set:
    ///
    /// ```ignore
    /// db.select(posts).filter(posts.title.matches("rust")).await?;
    /// ```
    pub fn matches<RowSet, P>(self, query: impl Into<String>) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
    {
        let _ = PhantomData::<P>;
        crate::expr::matches(self, query)
    }

    /// `IS NULL`.
    pub fn is_null<RowSet, P>(self) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
    {
        Expr::from_kind(ExprKind::Postfix {
            op: "IS NULL",
            expr: Operand::new(self.into_expr_kind()),
        })
    }

    /// `IS NOT NULL`.
    pub fn is_not_null<RowSet, P>(self) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
    {
        Expr::from_kind(ExprKind::Postfix {
            op: "IS NOT NULL",
            expr: Operand::new(self.into_expr_kind()),
        })
    }

    /// `IN (values...)`.
    ///
    /// The values are checked against this column's SQL type, so `users.age.in_list(["a"])`
    /// does not compile. An empty list renders as the false constant instead of the invalid
    /// `IN ()`.
    pub fn in_list<RowSet, P, V>(self, values: impl IntoIterator<Item = V>) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        V: IntoSqlValue<S>,
    {
        Expr::from_kind(ExprKind::InList {
            expr: Operand::new(self.into_expr_kind()),
            list: values
                .into_iter()
                .map(|v| ExprKind::Value(v.into_sql_value()))
                .collect(),
            negated: false,
        })
    }

    /// `NOT IN (values...)`.
    ///
    /// An empty list renders as the true constant: a value is not in nothing.
    pub fn not_in<RowSet, P, V>(self, values: impl IntoIterator<Item = V>) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        V: IntoSqlValue<S>,
    {
        Expr::from_kind(ExprKind::InList {
            expr: Operand::new(self.into_expr_kind()),
            list: values
                .into_iter()
                .map(|v| ExprKind::Value(v.into_sql_value()))
                .collect(),
            negated: true,
        })
    }

    /// `IN (subquery)`, where the subquery selects one column of this column's type.
    ///
    /// ```ignore
    /// let authors: Vec<User> = db
    ///     .select(users)
    ///     .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
    ///     .await?;
    /// ```
    ///
    /// The subquery's row type must be this column's Rust type, which is the closest the
    /// compiler can get to the SQL types matching: `pick` erases the SQL type to whatever
    /// the selection decodes as, so `i64` against `i64` is checked but a `VARCHAR` against
    /// an integer is not. It also means a nullable column's pick is `Option<T>` and is
    /// rejected; select it through [`as_expr`](Self::as_expr) to compare the same value
    /// without the nullability.
    ///
    /// This is not null-safe: if the subquery produces a `NULL`, the comparison is unknown
    /// rather than false. Use [`not_exists`] for an anti-join.
    pub fn in_query<RowSet, P, Q>(self, query: Q) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        Q: IntoSubquery,
        (Q::Row, S::Rust): SameType,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::InSubquery {
            expr: Operand::new(self.into_expr_kind()),
            query: Box::new(query.into_subquery()),
            negated: false,
        })
    }

    /// `NOT IN (subquery)`.
    ///
    /// See [`in_query`](Self::in_query) for the type check, and prefer [`not_exists`] when
    /// the subquery can produce `NULL`, because `NOT IN` is `UNKNOWN` rather than true then.
    pub fn not_in_query<RowSet, P, Q>(self, query: Q) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        Q: IntoSubquery,
        (Q::Row, S::Rust): SameType,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::InSubquery {
            expr: Operand::new(self.into_expr_kind()),
            query: Box::new(query.into_subquery()),
            negated: true,
        })
    }

    /// `BETWEEN low AND high`.
    pub fn between<RowSet, P, V>(self, low: V, high: V) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        V: Into<Value>,
    {
        Expr::from_kind(ExprKind::Between {
            expr: Operand::new(self.into_expr_kind()),
            low: Operand::new(ExprKind::Value(low.into())),
            high: Operand::new(ExprKind::Value(high.into())),
            negated: false,
        })
    }

    /// `COALESCE(this, fallback)`.
    pub fn coalesce<RowSet, P, V>(self, fallback: V) -> Expr<RowSet, S>
    where
        RowSet: Contains<P, T>,
        V: Into<S::Rust>,
    {
        Expr::from_kind(ExprKind::Function {
            name: "COALESCE",
            args: alloc::vec![
                self.into_expr_kind(),
                ExprKind::Value(fallback.into().into())
            ],
            distinct: false,
        })
    }

    /// `this + other`.
    pub fn plus<RowSet, A, M>(self, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        self.binary::<RowSet, A, M>("+", other)
    }

    /// `this - other`.
    pub fn minus<RowSet, A, M>(self, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        self.binary::<RowSet, A, M>("-", other)
    }

    /// `this * other`.
    pub fn times<RowSet, A, M>(self, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        self.binary::<RowSet, A, M>("*", other)
    }

    /// `this / other`.
    pub fn divide<RowSet, A, M>(self, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        self.binary::<RowSet, A, M>("/", other)
    }

    /// `this % other`.
    pub fn modulo<RowSet, A, M>(self, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        self.binary::<RowSet, A, M>("%", other)
    }

    fn binary<RowSet, A, M>(self, op: &'static str, other: A) -> Expr<RowSet, S>
    where
        A: IntoExpr<RowSet, S, M>,
    {
        Expr::from_kind(ExprKind::Binary {
            op,
            left: Operand::new(self.into_expr_kind()),
            right: Operand::new(other.into_expr().into_kind()),
        })
    }

    /// Sort ascending.
    pub fn asc(self) -> OrderClause {
        OrderClause {
            expr: self.into_expr_kind(),
            direction: Order::Asc,
            nulls: None,
        }
    }

    /// Sort descending.
    pub fn desc(self) -> OrderClause {
        OrderClause {
            expr: self.into_expr_kind(),
            direction: Order::Desc,
            nulls: None,
        }
    }

    /// Sort ascending, with `NULL`s first.
    pub fn asc_nulls_first(self) -> OrderClause {
        self.asc().nulls_first()
    }

    /// Sort descending, with `NULL`s last.
    pub fn desc_nulls_last(self) -> OrderClause {
        self.desc().nulls_last()
    }

    /// Use this column as an expression.
    ///
    /// This is only needed when you want an expression combinator that is not offered as a
    /// column method, for example `users.age.as_expr().cast::<Float8>()`.
    pub fn as_expr<RowSet>(self) -> Expr<RowSet, S> {
        Expr::from_kind(self.into_expr_kind())
    }

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        }
    }
}
