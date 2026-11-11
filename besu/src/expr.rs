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

use besu_core::{Dialect, Sql, TypeDef, Value};

use crate::types::{
    Bool, Bytes, Char, Column, Date, Decimal, Float4, Float8, Int1, Int2, Int4, Int8, Json, Jsonb,
    Nullability, SqlType, Text, Time, Timestamp, TimestampTz, UInt1, UInt2, UInt4, UInt8, Uuid,
    VarChar,
};

use crate::condition::{Contains, Literal};
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
        left: Box<ExprKind>,
        right: Box<ExprKind>,
    },
    /// `op expr`, as in `NOT x` or `-x`.
    Prefix {
        op: &'static str,
        expr: Box<ExprKind>,
    },
    /// `expr op`, as in `x IS NULL`.
    Postfix {
        op: &'static str,
        expr: Box<ExprKind>,
    },
    /// `NAME(args...)`.
    Function {
        name: &'static str,
        args: Vec<ExprKind>,
        distinct: bool,
    },
    /// `expr [NOT] BETWEEN low AND high`.
    Between {
        expr: Box<ExprKind>,
        low: Box<ExprKind>,
        high: Box<ExprKind>,
        negated: bool,
    },
    /// `expr [NOT] IN (list)`.
    InList {
        expr: Box<ExprKind>,
        list: Vec<ExprKind>,
        negated: bool,
    },
    /// `CASE ... WHEN ... THEN ... ELSE ... END`.
    Case {
        operand: Option<Box<ExprKind>>,
        branches: Vec<(ExprKind, ExprKind)>,
        fallback: Option<Box<ExprKind>>,
    },
    /// `CAST(expr AS type)`.
    Cast {
        expr: Box<ExprKind>,
        ty: TypeDef,
    },
}

impl ExprKind {
    /// Render this expression into `out`.
    pub(crate) fn render<D: Dialect>(&self, out: &mut Sql<D>) {
        match self {
            Self::Column { table, column } => {
                if table.is_empty() {
                    out.push_ident(column);
                } else {
                    out.push_qualified(table, column);
                }
            }
            Self::Value(value) => out.bind(value.clone()),
            Self::Star => out.push_raw("*"),
            Self::Raw(sql) => out.push_raw(sql),
            Self::Binary { op, left, right } => {
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
                for (i, arg) in args.iter().enumerate() {
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
                out.push_raw("(");
                expr.render(out);
                out.push_raw(if *negated { " NOT IN (" } else { " IN (" });
                for (i, item) in list.iter().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    item.render(out);
                }
                out.push_raw("))");
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
                for (when, then) in branches {
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
                out.push_raw(&D::type_name(ty.clone()));
                out.push_raw(")");
            }
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
            expr: Box::new(self.into_kind()),
            ty: S2::DEF,
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
pub enum ColumnMarker {}
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
pub trait IntoExpr<T, S: SqlType, M = ValueMarker> {
    /// The table this expression references, or [`Literal`] if it references none.
    ///
    /// This is what makes "you cannot mention a table you did not join" a compile error.
    type Table;

    /// Convert into an expression.
    fn into_expr(self) -> Expr<T, S>;
}

impl<RowSet, T: Table, S: SqlType, N: Nullability> IntoExpr<RowSet, S, ColumnMarker>
    for Column<T, S, N>
{
    type Table = T;

    fn into_expr(self) -> Expr<RowSet, S> {
        Expr::from_kind(ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        })
    }
}

impl<T, S: SqlType> IntoExpr<T, S, ExprMarker> for Expr<T, S> {
    type Table = Literal;

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
    type Table = Literal;

    fn into_expr(self) -> Expr<T, S> {
        Expr::from_kind(ExprKind::Value(self.into_sql_value()))
    }
}

impl<T, S: SqlType, V: IntoSqlValue<S>> IntoExpr<T, S, OptionMarker> for Option<V> {
    type Table = Literal;

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
                    left: Box::new(a.into_expr().into_kind()),
                    right: Box::new(b.into_expr().into_kind()),
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
pub fn is_null<RowSet, A, PA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS NULL",
        expr: Box::new(a.into_typed().into_kind()),
    })
}

/// `a IS NOT NULL`.
pub fn is_not_null<RowSet, A, PA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS NOT NULL",
        expr: Box::new(a.into_typed().into_kind()),
    })
}

/// `a IS TRUE`.
pub fn is_true<RowSet, A, PA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS TRUE",
        expr: Box::new(a.into_typed().into_kind()),
    })
}

/// `a IS FALSE`.
pub fn is_false<RowSet, A, PA>(a: A) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
{
    Expr::from_kind(ExprKind::Postfix {
        op: "IS FALSE",
        expr: Box::new(a.into_typed().into_kind()),
    })
}

/// `a IN (values...)`.
pub fn in_list<RowSet, A, I, V, PA>(a: A, values: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
    I: IntoIterator<Item = V>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::InList {
        expr: Box::new(a.into_typed().into_kind()),
        list: values
            .into_iter()
            .map(|v| ExprKind::Value(v.into()))
            .collect(),
        negated: false,
    })
}

/// `a NOT IN (values...)`.
pub fn not_in<RowSet, A, I, V, PA>(a: A, values: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
    I: IntoIterator<Item = V>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::InList {
        expr: Box::new(a.into_typed().into_kind()),
        list: values
            .into_iter()
            .map(|v| ExprKind::Value(v.into()))
            .collect(),
        negated: true,
    })
}

/// `a BETWEEN low AND high`.
pub fn between<RowSet, A, V, PA>(a: A, low: V, high: V) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::Between {
        expr: Box::new(a.into_typed().into_kind()),
        low: Box::new(ExprKind::Value(low.into())),
        high: Box::new(ExprKind::Value(high.into())),
        negated: false,
    })
}

/// `a NOT BETWEEN low AND high`.
pub fn not_between<RowSet, A, V, PA>(a: A, low: V, high: V) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet>,
    V: Into<Value>,
{
    Expr::from_kind(ExprKind::Between {
        expr: Box::new(a.into_typed().into_kind()),
        low: Box::new(ExprKind::Value(low.into())),
        high: Box::new(ExprKind::Value(high.into())),
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

impl<RowSet, T: Table, N: Nullability, P> IntoCondition<RowSet, (ColumnMarker, P)>
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
    let _ = PhantomData::<(PA, PB)>;
    Expr::from_kind(ExprKind::Binary {
        op: "AND",
        left: Box::new(a.into_condition()),
        right: Box::new(b.into_condition()),
    })
}

/// `a OR b`.
pub fn or<RowSet, PA, PB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoCondition<RowSet, PA>,
    B: IntoCondition<RowSet, PB>,
{
    let _ = PhantomData::<(PA, PB)>;
    Expr::from_kind(ExprKind::Binary {
        op: "OR",
        left: Box::new(a.into_condition()),
        right: Box::new(b.into_condition()),
    })
}

/// `NOT a`.
pub fn not<RowSet, PA, A>(a: A) -> Expr<RowSet, Bool>
where
    A: IntoCondition<RowSet, PA>,
{
    Expr::from_kind(ExprKind::Prefix {
        op: "NOT",
        expr: Box::new(a.into_condition()),
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
pub trait TypedExpr<RowSet> {
    /// The SQL type this expression produces.
    type SqlType: SqlType;

    /// The table this expression references, or [`Literal`] if it references none.
    type Table;

    /// Convert into an expression.
    fn into_typed(self) -> Expr<RowSet, Self::SqlType>;
}

impl<RowSet, T: Table, S: SqlType, N: Nullability> TypedExpr<RowSet> for Column<T, S, N> {
    type SqlType = S;
    type Table = T;

    fn into_typed(self) -> Expr<RowSet, S> {
        Expr::from_kind(ExprKind::Column {
            table: T::DEF.name,
            column: self.name(),
        })
    }
}

impl<RowSet, S: SqlType> TypedExpr<RowSet> for Expr<RowSet, S> {
    type SqlType = S;
    type Table = Literal;

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
            pub fn $name<RowSet, A>(value: A) -> Expr<RowSet, $out>
            where
                A: TypedExpr<RowSet>,
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
            pub fn $name<RowSet, A>(value: A) -> Expr<RowSet, A::SqlType>
            where
                A: TypedExpr<RowSet>,
            {
                call($sql, alloc::vec![value.into_typed().into_kind()])
            }
        )*
    };
}

unary_to! {
    /// `COUNT(expr)`.
    count -> Int8 = "COUNT",
    /// `SUM(expr)`, widened to a float because integer sums overflow.
    sum -> Float8 = "SUM",
    /// `AVG(expr)`.
    avg -> Float8 = "AVG",
    /// `LOWER(expr)`.
    lower -> Text = "LOWER",
    /// `UPPER(expr)`.
    upper -> Text = "UPPER",
    /// `LENGTH(expr)`.
    length -> Int8 = "LENGTH",
    /// `TRIM(expr)`.
    trim -> Text = "TRIM",
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
pub fn count_distinct<RowSet, A, P>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet>,
{
    let _ = PhantomData::<P>;
    Expr::from_kind(ExprKind::Function {
        name: "COUNT",
        args: alloc::vec![value.into_typed().into_kind()],
        distinct: true,
    })
}

/// `CONCAT(a, b)`.
pub fn concat<RowSet, A, B, PA, PB>(a: A, b: B) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet>,
    B: TypedExpr<RowSet>,
{
    call(
        "CONCAT",
        alloc::vec![a.into_typed().into_kind(), b.into_typed().into_kind()],
    )
}

/// `COALESCE(a, b)`, falling back to `b` when `a` is `NULL`.
pub fn coalesce<RowSet, A, B, MB, PA, PB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "COALESCE",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// The database's current timestamp.
pub fn now<T>() -> Expr<T, TimestampTz> {
    Expr::from_kind(ExprKind::Raw("CURRENT_TIMESTAMP".into()))
}

/// A raw SQL condition. The string is inserted verbatim, so never pass it user input.
pub fn raw<T>(sql: impl Into<String>) -> Expr<T, Bool> {
    Expr::from_kind(ExprKind::Raw(sql.into()))
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
                    left: Box::new(ExprKind::Column {
                        table: T::DEF.name,
                        column: self.name(),
                    }),
                    right: Box::new(other.into_expr().into_kind()),
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

    /// `IS NULL`.
    pub fn is_null<RowSet, P>(self) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
    {
                Expr::from_kind(ExprKind::Postfix {
            op: "IS NULL",
            expr: Box::new(self.into_expr_kind()),
        })
    }

    /// `IS NOT NULL`.
    pub fn is_not_null<RowSet, P>(self) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
    {
                Expr::from_kind(ExprKind::Postfix {
            op: "IS NOT NULL",
            expr: Box::new(self.into_expr_kind()),
        })
    }

    /// `IN (values...)`.
    pub fn in_list<RowSet, P, V>(
        self,
        values: impl IntoIterator<Item = V>,
    ) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        V: Into<Value>,
    {
                Expr::from_kind(ExprKind::InList {
            expr: Box::new(self.into_expr_kind()),
            list: values
                .into_iter()
                .map(|v| ExprKind::Value(v.into()))
                .collect(),
            negated: false,
        })
    }

    /// `NOT IN (values...)`.
    pub fn not_in<RowSet, P, V>(
        self,
        values: impl IntoIterator<Item = V>,
    ) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        V: Into<Value>,
    {
                Expr::from_kind(ExprKind::InList {
            expr: Box::new(self.into_expr_kind()),
            list: values
                .into_iter()
                .map(|v| ExprKind::Value(v.into()))
                .collect(),
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
            expr: Box::new(self.into_expr_kind()),
            low: Box::new(ExprKind::Value(low.into())),
            high: Box::new(ExprKind::Value(high.into())),
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
            args: alloc::vec![self.into_expr_kind(), ExprKind::Value(fallback.into().into())],
            distinct: false,
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
