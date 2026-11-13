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
    Array, Bit, BitString, Bool, Bytes, Char, Cidr, Column, Date, Decimal, Float4, Float8, I128,
    Inet, Int1, Int2, Int4, Int8, Json, Jsonb, Nullability, SqlInterval, SqlType, Text, Time,
    Timestamp, TimestampTz, TypeWitness, U128, UInt1, UInt2, UInt4, UInt8, Uuid, VarBit, VarChar,
};
use crate::types::{GeographyOf, Geometry, GeometryOf, Range, Vector, VectorOf};

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
    /// A column of a name that exists only in the statement: a common table expression or a
    /// table alias.
    ///
    /// Identical to [`ExprKind::Column`] to look at, except that neither name takes the
    /// tenant prefix: such a name is a name in the statement, not a table in the database,
    /// so rewriting it would produce `"tenant_adults"` and a missing-relation error in a
    /// prefixed database.
    UnprefixedColumn {
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
    /// `left IS [NOT] DISTINCT FROM right`.
    ///
    /// This is its own node rather than a [`ExprKind::Binary`] because MySQL has no such
    /// keyword: its null-safe equality operator `<=>` is `IS NOT DISTINCT FROM`, so the
    /// negated form is written around it. Keeping the meaning here lets each dialect spell
    /// the operation its own way.
    DistinctFrom {
        left: Operand,
        right: Operand,
        /// `true` for `IS DISTINCT FROM`, `false` for `IS NOT DISTINCT FROM`.
        negated: bool,
    },
    /// A regular-expression match.
    ///
    /// The three databases disagree about the operator itself, so the node records the
    /// meaning — match or no match, case sensitive or not — and the dialect writes it.
    Regex {
        left: Operand,
        right: Operand,
        /// `true` for a case-insensitive match.
        case_insensitive: bool,
        /// `true` for a negated match.
        negated: bool,
    },
    /// `expr FILTER (WHERE condition)`, which narrows an aggregate to some of its rows.
    Filter { expr: Operand, condition: Operand },
    /// `expr OVER (window)`, which turns an aggregate or a window function into one that
    /// looks at a window of rows rather than at the whole result.
    Over {
        expr: Operand,
        /// Boxed because a window is much larger than the other nodes.
        window: Box<Window>,
    },
    /// `EXTRACT(field FROM expr)`, spelled per dialect.
    Extract { field: DateField, expr: Operand },
    /// The number of whole days from `right` to `left`.
    DateDiff { left: Operand, right: Operand },
    /// `expr ± interval`.
    DateAdd {
        expr: Operand,
        interval: Interval,
        /// `true` for subtraction.
        subtract: bool,
        /// Whether the value being shifted is a `DATE` rather than a timestamp.
        date_only: bool,
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
    /// A value inside a JSON document, reached by a structured path.
    JsonPath {
        expr: Operand,
        /// The path, which each dialect renders in its own grammar.
        path: JsonPath,
        /// `true` for the text form (`->>`), `false` for the JSON form (`->`).
        as_text: bool,
    },
    /// `document -> key` / `document ->> key`, with the key bound as a parameter.
    ///
    /// This is the spelling to reach for when the key is not known until run time: the
    /// path form has to be rendered into the statement, while this one binds the key, so a
    /// key that comes from a user is never SQL.
    JsonKey {
        expr: Operand,
        /// The key, bound as a text parameter.
        key: Value,
        /// `true` for `->>`, which returns text rather than JSON.
        as_text: bool,
    },
    /// A JSON function that takes a path, as in MySQL's `JSON_VALUE` or `JSON_SET`.
    ///
    /// The path cannot be an ordinary argument because its grammar differs per dialect, so it
    /// travels as a [`JsonPath`] and is rendered in place.
    JsonPathFunction {
        /// The canonical function name, as [`Dialect::function`] knows it.
        name: &'static str,
        document: Operand,
        path: JsonPath,
        /// Any arguments after the path.
        extra: Vec<Operand>,
    },
    /// A JSON containment, existence or path operator.
    JsonOp {
        op: JsonOperator,
        left: Operand,
        right: JsonRight,
    },
    /// `left op right` for an operator only PostgreSQL has: `@>`, `<@`, `&&` or `||`.
    ///
    /// The node is separate from [`ExprKind::Binary`] because a dialect without arrays must
    /// reject it rather than emit PostgreSQL's spelling, which the server would parse as a
    /// syntax error a long way from the query that caused it.
    ArrayOp {
        op: &'static str,
        left: Operand,
        right: Operand,
    },
    /// `array[index]`, PostgreSQL's subscript.
    Subscript { array: Operand, index: Operand },
    /// `expr = ANY(array)`, or `<> ALL(array)` when negated.
    ///
    /// The array is a single bound parameter rather than one parameter per element, which
    /// is what lets an `IN` list be as long as the database's array limit instead of its
    /// parameter limit.
    AnyOf {
        expr: Operand,
        /// The declared element type, which PostgreSQL's array encoding needs.
        element: TypeDef,
        values: Vec<Value>,
        negated: bool,
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
    /// `(SELECT ...)`, a subquery that produces a single value.
    ///
    /// The parentheses are part of the node rather than of the caller because a scalar
    /// subquery is only valid where a value is: in a projection, in a comparison and as an
    /// argument to a function.
    ScalarSubquery {
        /// The subquery, which must project exactly one column.
        query: Box<Subquery>,
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

/// One step of a [`JsonPath`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonSegment {
    /// An object member, by name.
    Key(String),
    /// An array element, by position.
    Index(i64),
    /// Raw path text in the dialect's own grammar.
    ///
    /// This is the escape hatch for everything the structured segments cannot express. It is
    /// inserted verbatim, so it must never contain user input.
    Raw(String),
}

/// A structured path into a JSON document.
///
/// A path is built from segments rather than written as one dotted string so that each
/// dialect can render it in its own grammar: PostgreSQL takes a `text[]` path, MySQL and
/// SQLite take a `$.a[0]` path. That is the difference that makes a key containing a dot,
/// a comma or a quote expressible — `root().key("a.b")` is one key, not two.
///
/// ```ignore
/// JsonPath::root().key("a").index(0).key("b")
/// ```
///
/// [`JsonPath::raw`] appends text in the dialect's grammar for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsonPath {
    segments: Vec<JsonSegment>,
}

impl JsonPath {
    /// The document itself.
    pub const fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// The object member called `key`.
    ///
    /// The name is treated as one member, so a dot inside it does not become a separator.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.segments.push(JsonSegment::Key(key.into()));
        self
    }

    /// The element at `index`, counted from zero.
    pub fn index(mut self, index: i64) -> Self {
        self.segments.push(JsonSegment::Index(index));
        self
    }

    /// Append raw path text in the dialect's own grammar.
    ///
    /// This is the escape hatch for syntax the structured segments do not cover. The text is
    /// never escaped or validated, so never pass it user input.
    pub fn raw(mut self, raw: impl Into<String>) -> Self {
        self.segments.push(JsonSegment::Raw(raw.into()));
        self
    }

    /// Whether the path names the document itself.
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Write ` #> '...'` or ` #>> '...'` after an already rendered document.
    fn render_document<D: Dialect>(&self, out: &mut Sql<D>, as_text: bool) {
        out.push_raw(if as_text { " #>> '" } else { " #> '" });
        out.push_raw(&self.postgres_literal());
        out.push_raw("'");
    }

    /// The PostgreSQL `text[]` literal for this path, as a single-quoted string's contents.
    fn postgres_literal(&self) -> String {
        let mut out = String::from("{");
        for (i, segment) in self.segments.iter().enumerate() {
            if i != 0 {
                out.push(',');
            }
            match segment {
                JsonSegment::Key(key) => push_pg_array_element(&mut out, key),
                JsonSegment::Index(index) => out.push_str(&index.to_string()),
                JsonSegment::Raw(raw) => out.push_str(raw),
            }
        }
        out.push('}');
        out
    }

    /// Write a MySQL JSON path literal, including its quotes, in place of a document.
    fn render_mysql_path<D: Dialect>(&self, out: &mut Sql<D>) {
        out.push_raw("'");
        out.push_raw(&self.mysql_path_text());
        out.push_raw("'");
    }

    /// Write a SQLite JSON path literal, including its quotes.
    fn render_sqlite_path<D: Dialect>(&self, out: &mut Sql<D>) {
        out.push_raw("'");
        out.push_raw(&self.sqlite_path_text());
        out.push_raw("'");
    }

    /// The path as MySQL spells it, before SQL-literal escaping.
    fn mysql_path_text(&self) -> String {
        let mut out = String::from("$");
        for segment in &self.segments {
            match segment {
                JsonSegment::Key(key) => {
                    out.push('.');
                    push_mysql_quoted_member(&mut out, key);
                }
                JsonSegment::Index(index) => {
                    out.push('[');
                    out.push_str(&index.to_string());
                    out.push(']');
                }
                JsonSegment::Raw(raw) => out.push_str(raw),
            }
        }
        // A MySQL string literal treats a backslash as an escape, so every backslash the
        // path needs has to be doubled before it reaches the parser.
        out.replace('\\', "\\\\").replace('\'', "''")
    }

    /// The path as SQLite spells it, before SQL-literal escaping.
    fn sqlite_path_text(&self) -> String {
        let mut out = String::from("$");
        for segment in &self.segments {
            match segment {
                JsonSegment::Key(key) => {
                    out.push('.');
                    push_sqlite_quoted_member(&mut out, key);
                }
                JsonSegment::Index(index) => {
                    out.push('[');
                    out.push_str(&index.to_string());
                    out.push(']');
                }
                JsonSegment::Raw(raw) => out.push_str(raw),
            }
        }
        out.replace('\'', "''")
    }
}

/// A key is safe to write bare when it is an ASCII identifier.
fn is_simple_json_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Append one element of a PostgreSQL array literal.
fn push_pg_array_element(out: &mut String, element: &str) {
    // A single quote is escaped wherever it appears: the finished array literal is wrapped in
    // `'…'` as an SQL string, and PostgreSQL reads a doubled quote as one quote.
    let needs_quotes = element.is_empty()
        || element
            .chars()
            .any(|ch| matches!(ch, '\'' | ',' | '{' | '}' | '"' | '\\'));
    if !needs_quotes {
        out.push_str(element);
        return;
    }
    out.push('"');
    for ch in element.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\'' => {
                out.push('\'');
                out.push('\'');
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
}

/// Append a MySQL JSON path member, quoting it when it is not a bare identifier.
fn push_mysql_quoted_member(out: &mut String, key: &str) {
    if is_simple_json_key(key) {
        out.push_str(key);
        return;
    }
    out.push('"');
    for ch in key.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
}

/// Append a SQLite JSON path member, quoting it when it is not a bare identifier.
fn push_sqlite_quoted_member(out: &mut String, key: &str) {
    if is_simple_json_key(key) {
        out.push_str(key);
        return;
    }
    out.push('"');
    for ch in key.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

/// A JSON operator that the dialects spell in incompatible ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum JsonOperator {
    /// The left document contains the right one.
    Contains,
    /// The left document is contained by the right one.
    ContainedBy,
    /// The document has a top-level key.
    HasKey,
    /// The document has at least one of the keys.
    HasAnyKey,
    /// The document has every one of the keys.
    HasAllKey,
    /// Remove a path from the document.
    Strip,
    /// The document satisfies a JSON path predicate.
    MatchesPath,
    /// The document satisfies a JSON path `@@` match.
    Matches,
    /// The two documents have an array element or object member in common.
    ///
    /// MySQL spells it `JSON_OVERLAPS`; PostgreSQL has no value-overlap operator for JSON,
    /// so it records an `unsupported` error there.
    Overlaps,
}

/// The right-hand side of a [`JsonOperator`].
#[derive(Debug, Clone, PartialEq)]
#[doc(hidden)]
pub enum JsonRight {
    /// An expression, compared as JSON.
    Operand(Operand),
    /// A single key, bound on PostgreSQL and rendered into a path elsewhere.
    Key(String),
    /// A list of keys, bound as a `text[]` on PostgreSQL.
    Keys(Vec<String>),
    /// A document path.
    Path(JsonPath),
}

impl JsonRight {
    /// Remove the table name from a column reference inside this side.
    fn unqualify(&mut self) {
        if let Self::Operand(operand) = self {
            operand.unqualify();
        }
    }

    /// Whether this side reads the row an upsert is inserting.
    fn reads_upsert_row(&self) -> bool {
        match self {
            Self::Operand(operand) => operand.reads_upsert_row(),
            Self::Key(_) | Self::Keys(_) | Self::Path(_) => false,
        }
    }

    /// Point every reference to the incoming upsert row at `row`.
    fn set_upsert_row(&mut self, row: &'static str) {
        if let Self::Operand(operand) = self {
            operand.set_upsert_row(row);
        }
    }
}

/// Render an argument that has to be JSON rather than a string parameter.
///
/// MySQL takes a string parameter as a JSON string, so `JSON_SET(doc, '$.a', ?)` with `4`
/// stores `"4"`. Casting the parameter to `JSON` is what makes the value mean the document
/// the caller passed.
fn render_json_argument<D: Dialect>(out: &mut Sql<D>, operand: &mut Operand) {
    out.push_raw("CAST(");
    operand.render(out);
    out.push_raw(" AS JSON)");
}

/// Render a JSON operator in whichever grammar the dialect has.
fn render_json_op<D: Dialect>(
    out: &mut Sql<D>,
    op: JsonOperator,
    left: &mut Operand,
    right: &mut JsonRight,
) {
    use besu_core::JsonPathStyle;
    if D::JSON_PATH == JsonPathStyle::Postgres {
        left.render(out);
        let operator = match op {
            JsonOperator::Contains => " @> ",
            JsonOperator::ContainedBy => " <@ ",
            JsonOperator::HasKey => " ? ",
            JsonOperator::HasAnyKey => " ?| ",
            JsonOperator::HasAllKey => " ?& ",
            JsonOperator::Strip => " #- ",
            JsonOperator::MatchesPath => " @? ",
            JsonOperator::Matches => " @@ ",
            JsonOperator::Overlaps => {
                // `?|` asks whether a key is present; there is no value-overlap operator for
                // JSON on PostgreSQL, so the closest spelling is refused rather than guessed.
                out.set_error(besu_core::Error::unsupported(
                    "`postgres` has no JSON overlap; compare with `@>` or `?|` instead",
                ));
                " @> "
            }
        };
        out.push_raw(operator);
        match right {
            JsonRight::Operand(operand) => operand.render(out),
            JsonRight::Key(key) => {
                out.bind_value(Value::Text(core::mem::take(key)));
                // `@?` and `@@` take a `jsonpath`, and a text parameter does not resolve
                // either operator without the cast.
                if matches!(op, JsonOperator::MatchesPath | JsonOperator::Matches) {
                    out.push_raw("::jsonpath");
                }
            }
            JsonRight::Keys(keys) => out.bind_value(Value::Array {
                element: TypeDef::Text,
                values: core::mem::take(keys).into_iter().map(Value::Text).collect(),
            }),
            JsonRight::Path(path) => {
                out.push_raw("'");
                out.push_raw(&path.postgres_literal());
                out.push_raw("'");
            }
        }
        return;
    }

    if D::JSON_PATH == JsonPathStyle::Mysql {
        match op {
            JsonOperator::Contains => {
                out.push_raw("JSON_CONTAINS(");
                left.render(out);
                out.push_raw(", ");
                match right {
                    JsonRight::Operand(operand) => render_json_argument::<D>(out, operand),
                    _ => out.set_error(besu_core::Error::unsupported(
                        "`JSON_CONTAINS` compares two documents, not a key",
                    )),
                }
                out.push_raw(")");
            }
            JsonOperator::ContainedBy => {
                // `JSON_CONTAINS(target, candidate)` asks whether the target contains the
                // candidate, so "is contained by" swaps the two sides.
                out.push_raw("JSON_CONTAINS(");
                match right {
                    JsonRight::Operand(operand) => render_json_argument::<D>(out, operand),
                    _ => out.set_error(besu_core::Error::unsupported(
                        "`JSON_CONTAINS` compares two documents, not a key",
                    )),
                }
                out.push_raw(", ");
                left.render(out);
                out.push_raw(")");
            }
            JsonOperator::HasKey | JsonOperator::HasAnyKey | JsonOperator::HasAllKey => {
                out.push_raw("JSON_CONTAINS_PATH(");
                left.render(out);
                out.push_raw(if op == JsonOperator::HasAllKey {
                    ", 'all'"
                } else {
                    ", 'one'"
                });
                let keys: Vec<String> = match right {
                    JsonRight::Key(key) => core::iter::once(core::mem::take(key)).collect(),
                    JsonRight::Keys(keys) => core::mem::take(keys),
                    _ => {
                        out.set_error(besu_core::Error::unsupported(
                            "a key test needs literal key names",
                        ));
                        Vec::new()
                    }
                };
                for key in keys {
                    out.push_raw(", '");
                    out.push_raw(&JsonPath::root().key(key).mysql_path_text());
                    out.push_raw("'");
                }
                out.push_raw(")");
            }
            JsonOperator::Overlaps => {
                out.push_raw("JSON_OVERLAPS(");
                left.render(out);
                out.push_raw(", ");
                match right {
                    JsonRight::Operand(operand) => render_json_argument::<D>(out, operand),
                    _ => out.set_error(besu_core::Error::unsupported(
                        "`JSON_OVERLAPS` compares two documents, not a key",
                    )),
                }
                out.push_raw(")");
            }
            JsonOperator::Strip => {
                out.push_raw("JSON_REMOVE(");
                left.render(out);
                match right {
                    JsonRight::Path(path) => {
                        out.push_raw(", ");
                        path.render_mysql_path(out);
                    }
                    _ => out.set_error(besu_core::Error::unsupported("`JSON_REMOVE` needs a path")),
                }
                out.push_raw(")");
            }
            JsonOperator::MatchesPath | JsonOperator::Matches => {
                out.set_error(besu_core::Error::unsupported(
                    "`mysql` has no JSON path predicate".to_owned(),
                ));
                left.render(out);
            }
        }
        return;
    }

    out.set_error(besu_core::Error::unsupported(alloc::format!(
        "`{}` has no JSON operators; write the query with `raw` if this database has them",
        D::NAME
    )));
    left.render(out);
}

/// Render a path-taking JSON function, whose path grammar differs per dialect.
fn render_json_path_function<D: Dialect>(
    out: &mut Sql<D>,
    name: &'static str,
    document: &mut Operand,
    path: &JsonPath,
    extra: &mut [Operand],
) {
    use besu_core::JsonPathStyle;
    match D::JSON_PATH {
        JsonPathStyle::Mysql | JsonPathStyle::Sqlite => {
            let Some(spelling) = D::function(name) else {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` has no `{name}` function",
                    D::NAME
                )));
                document.render(out);
                return;
            };
            out.push_raw(spelling.name());
            out.push_raw("(");
            document.render(out);
            out.push_raw(", ");
            if D::JSON_PATH == JsonPathStyle::Mysql {
                path.render_mysql_path(out);
            } else {
                path.render_sqlite_path(out);
            }
            for operand in extra {
                out.push_raw(", ");
                if D::JSON_PATH == JsonPathStyle::Mysql {
                    render_json_argument::<D>(out, operand);
                } else {
                    operand.render(out);
                }
            }
            out.push_raw(")");
        }
        JsonPathStyle::Postgres => {
            // PostgreSQL's path-taking function takes a `text[]` path and always wants the
            // `create_missing` flag, so it is the one spelling that is not the same shape.
            if name == "json_set" {
                out.push_raw("JSONB_SET(");
                document.render(out);
                out.push_raw(", '");
                out.push_raw(&path.postgres_literal());
                out.push_raw("', ");
                match extra {
                    [value] => value.render(out),
                    _ => out.set_error(besu_core::Error::unsupported(
                        "`json_set` takes exactly one value",
                    )),
                }
                out.push_raw(", true)");
            } else {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`postgres` has no `{name}` function"
                )));
                document.render(out);
            }
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
            Self::UnprefixedColumn { table, column } => {
                out.push_ident(table);
                out.push_raw(".");
                out.push_ident(column);
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
            Self::DistinctFrom {
                left,
                right,
                negated,
            } => render_distinct_from::<D>(out, left, right, *negated),
            Self::Regex {
                left,
                right,
                case_insensitive,
                negated,
            } => render_regex::<D>(out, left, right, *case_insensitive, *negated),
            Self::Filter { expr, condition } => {
                // MySQL has no `FILTER` clause at all; `SUM(IF(cond, x, NULL))` is the
                // equivalent, and it is a different expression rather than a dialect
                // spelling, so it is not rendered here.
                if D::NAME == "mysql" {
                    out.set_error(besu_core::Error::unsupported(
                        "`mysql` has no `FILTER (WHERE ...)`; use a `case` expression inside \
                         the aggregate instead, as in `sum(case().when(cond, value).otherwise(0))`",
                    ));
                }
                out.push_raw("(");
                expr.render(out);
                out.push_raw(" FILTER (WHERE ");
                condition.render(out);
                out.push_raw("))");
            }
            Self::Over { expr, window } => {
                expr.render(out);
                out.push_raw(" OVER (");
                window.render::<D>(out);
                out.push_raw(")");
            }
            Self::Extract { field, expr } => render_extract::<D>(out, *field, expr),
            Self::DateDiff { left, right } => render_date_diff::<D>(out, left, right),
            Self::DateAdd {
                expr,
                interval,
                subtract,
                date_only,
            } => render_date_add::<D>(out, expr, *interval, *subtract, *date_only),
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
                use besu_core::dialect::FunctionStyle;
                let style = D::function(name);
                if style.is_none() {
                    out.set_error(besu_core::Error::unsupported(alloc::format!(
                        "`{}` has no `{name}` function; write it with `raw` if this database \
                         spells it differently",
                        D::NAME
                    )));
                }
                let spelling = style.unwrap_or(FunctionStyle::Named(name));
                if let FunctionStyle::Bare(spelling) = spelling {
                    // `CURRENT_DATE` and friends are keywords rather than calls, so writing
                    // parentheses after them would be a syntax error.
                    out.push_raw(spelling);
                    return;
                }
                out.push_raw(spelling.name());
                out.push_raw("(");
                if *distinct {
                    out.push_raw("DISTINCT ");
                }
                let round_to_numeric = D::ROUND_TO_NEEDS_NUMERIC && *name == "round";
                let two_arguments = args.len() > 1;
                for (i, arg) in args.iter_mut().enumerate() {
                    if i != 0 {
                        match spelling {
                            // MySQL's `GROUP_CONCAT(a SEPARATOR b)` is the only spelling of
                            // a two-argument aggregate joined by an explicit separator.
                            FunctionStyle::Separator(_) => out.push_raw(" SEPARATOR "),
                            _ => out.push_raw(", "),
                        }
                    }
                    // PostgreSQL has `round(numeric, int)` and no `round(double precision,
                    // int)`, and the helper is typed for any number. Casting the value makes
                    // the portable call work without changing what the helper promises.
                    if round_to_numeric && two_arguments && i == 0 {
                        out.push_raw("CAST(");
                        arg.render(out);
                        out.push_raw(" AS numeric)");
                    } else {
                        arg.render(out);
                    }
                }
                if let FunctionStyle::WithSuffix(_, extra) = spelling {
                    for literal in extra {
                        out.push_raw(", ");
                        out.push_raw(literal);
                    }
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
                        path.render_document(out, *as_text);
                    }
                    JsonPathStyle::Mysql => {
                        if *as_text {
                            out.push_raw("JSON_UNQUOTE(");
                        }
                        out.push_raw("JSON_EXTRACT(");
                        expr.render(out);
                        out.push_raw(", ");
                        path.render_mysql_path(out);
                        out.push_raw(")");
                        if *as_text {
                            out.push_raw(")");
                        }
                    }
                    JsonPathStyle::Sqlite => {
                        // `->` and `->>` have been available since SQLite 3.38 and keep the
                        // JSON/text distinction that `json_extract` loses.
                        expr.render(out);
                        out.push_raw(if *as_text { " ->> " } else { " -> " });
                        path.render_sqlite_path(out);
                    }
                }
            }
            Self::JsonKey { expr, key, as_text } => {
                use besu_core::JsonPathStyle;
                match D::JSON_PATH {
                    JsonPathStyle::Postgres => {
                        expr.render(out);
                        out.push_raw(if *as_text { " ->> " } else { " -> " });
                        out.bind_value(core::mem::replace(key, Value::Null));
                    }
                    // Neither MySQL nor SQLite has an operator that takes a key on its own:
                    // both are reached through a path, which is what the bound key becomes.
                    JsonPathStyle::Mysql | JsonPathStyle::Sqlite => {
                        let path = match key {
                            Value::Text(key) => JsonPath::root().key(key.clone()),
                            _ => JsonPath::root(),
                        };
                        match D::JSON_PATH {
                            JsonPathStyle::Mysql => {
                                if *as_text {
                                    out.push_raw("JSON_UNQUOTE(");
                                }
                                out.push_raw("JSON_EXTRACT(");
                                expr.render(out);
                                out.push_raw(", ");
                                path.render_mysql_path(out);
                                out.push_raw(")");
                                if *as_text {
                                    out.push_raw(")");
                                }
                            }
                            _ => {
                                // `->>` keeps the JSON/text distinction that
                                // `json_extract` loses.
                                expr.render(out);
                                out.push_raw(if *as_text { " ->> " } else { " -> " });
                                path.render_sqlite_path(out);
                            }
                        }
                    }
                }
            }
            Self::JsonOp { op, left, right } => render_json_op::<D>(out, *op, left, right),
            Self::JsonPathFunction {
                name,
                document,
                path,
                extra,
            } => render_json_path_function::<D>(out, name, document, path, extra),
            Self::ArrayOp { op, left, right } => {
                if !D::SUPPORTS_ARRAYS {
                    out.set_error(besu_core::Error::unsupported(alloc::format!(
                        "`{}` has no array operators; only PostgreSQL has an array type",
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
            Self::Subscript { array, index } => {
                if !D::SUPPORTS_ARRAYS {
                    out.set_error(besu_core::Error::unsupported(alloc::format!(
                        "`{}` has no array subscript; only PostgreSQL has an array type",
                        D::NAME
                    )));
                }
                array.render(out);
                out.push_raw("[");
                index.render(out);
                out.push_raw("]");
            }
            Self::AnyOf {
                expr,
                element,
                values,
                negated,
            } => {
                if values.is_empty() {
                    // `IN ()` is a syntax error in every dialect, but an empty list has an
                    // obvious meaning, and the constant is both portable and parameter-free.
                    out.push_raw(if *negated { "(1 = 1)" } else { "(1 = 0)" });
                } else if D::SUPPORTS_ARRAYS {
                    // One array parameter instead of one per element, which is what keeps a
                    // large `IN` list under the 65,535-parameter limit.
                    out.push_raw("(");
                    expr.render(out);
                    out.push_raw(if *negated { " <> ALL(" } else { " = ANY(" });
                    out.bind_value(Value::Array {
                        element: element.clone(),
                        values: core::mem::take(values),
                    });
                    out.push_raw("))");
                } else {
                    // Every other dialect binds one parameter per element. The list is
                    // already in memory, so this is the same `IN (...)` it always was.
                    out.push_raw("(");
                    expr.render(out);
                    out.push_raw(if *negated { " NOT IN (" } else { " IN (" });
                    for (i, value) in core::mem::take(values).into_iter().enumerate() {
                        if i != 0 {
                            out.push_raw(", ");
                        }
                        out.bind_value(value);
                    }
                    out.push_raw("))");
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
            Self::ScalarSubquery { query } => {
                let columns = query.query.projection_len();
                if columns != 1 {
                    out.set_error(besu_core::Error::invalid_query(alloc::format!(
                        "a scalar subquery must select exactly one column, but this one selects \
                         {columns}"
                    )));
                }
                out.push_raw("(");
                // Rendered into the same statement, so its parameters continue the outer
                // statement's numbering.
                query.query.render_into(out);
                out.push_raw(")");
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
            | Self::JsonKey { expr, .. }
            | Self::InSubquery { expr, .. }
            | Self::Extract { expr, .. }
            | Self::DateAdd { expr, .. } => expr.unqualify(),
            Self::JsonOp { left, right, .. } => {
                left.unqualify();
                right.unqualify();
            }
            Self::ArrayOp { left, right, .. } => {
                left.unqualify();
                right.unqualify();
            }
            Self::JsonPathFunction {
                document, extra, ..
            } => {
                document.unqualify();
                for operand in extra {
                    operand.unqualify();
                }
            }
            Self::Subscript { array, index } => {
                array.unqualify();
                index.unqualify();
            }
            Self::AnyOf { expr, values, .. } => {
                expr.unqualify();
                // A value has no table name, but the list is emptied as the node is
                // consumed elsewhere; nothing to do here.
                let _ = values;
            }
            Self::DistinctFrom { left, right, .. }
            | Self::Regex { left, right, .. }
            | Self::DateDiff { left, right, .. } => {
                left.unqualify();
                right.unqualify();
            }
            Self::Filter { expr, condition } => {
                expr.unqualify();
                condition.unqualify();
            }
            Self::Over { expr, window } => {
                expr.unqualify();
                window.unqualify();
            }
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
            | Self::ScalarSubquery { .. }
            | Self::UnprefixedColumn { .. }
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
            | Self::JsonKey { expr, .. }
            | Self::InSubquery { expr, .. }
            | Self::Extract { expr, .. }
            | Self::DateAdd { expr, .. } => expr.reads_upsert_row(),
            Self::JsonOp { left, right, .. } => left.reads_upsert_row() || right.reads_upsert_row(),
            Self::ArrayOp { left, right, .. } => {
                left.reads_upsert_row() || right.reads_upsert_row()
            }
            Self::JsonPathFunction {
                document, extra, ..
            } => document.reads_upsert_row() || extra.iter().any(Operand::reads_upsert_row),
            Self::Subscript { array, index } => {
                array.reads_upsert_row() || index.reads_upsert_row()
            }
            Self::AnyOf { expr, values, .. } => {
                let _ = values;
                expr.reads_upsert_row()
            }
            Self::DistinctFrom { left, right, .. }
            | Self::Regex { left, right, .. }
            | Self::DateDiff { left, right, .. } => {
                left.reads_upsert_row() || right.reads_upsert_row()
            }
            Self::Filter { expr, condition } => {
                expr.reads_upsert_row() || condition.reads_upsert_row()
            }
            Self::Over { expr, window } => expr.reads_upsert_row() || window.reads_upsert_row(),
            Self::FullText { column, .. } => column.reads_upsert_row(),
            Self::Column { .. }
            | Self::UnprefixedColumn { .. }
            | Self::Value(_)
            | Self::Star
            | Self::Raw(_)
            | Self::Exists { .. }
            | Self::ScalarSubquery { .. } => false,
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
            | Self::JsonKey { expr, .. }
            | Self::InSubquery { expr, .. }
            | Self::Extract { expr, .. }
            | Self::DateAdd { expr, .. } => expr.set_upsert_row(row),
            Self::JsonOp { left, right, .. } => {
                left.set_upsert_row(row);
                right.set_upsert_row(row);
            }
            Self::ArrayOp { left, right, .. } => {
                left.set_upsert_row(row);
                right.set_upsert_row(row);
            }
            Self::JsonPathFunction {
                document, extra, ..
            } => {
                document.set_upsert_row(row);
                for operand in extra {
                    operand.set_upsert_row(row);
                }
            }
            Self::Subscript { array, index } => {
                array.set_upsert_row(row);
                index.set_upsert_row(row);
            }
            Self::AnyOf { expr, values, .. } => {
                let _ = values;
                expr.set_upsert_row(row);
            }
            Self::DistinctFrom { left, right, .. }
            | Self::Regex { left, right, .. }
            | Self::DateDiff { left, right, .. } => {
                left.set_upsert_row(row);
                right.set_upsert_row(row);
            }
            Self::Filter { expr, condition } => {
                expr.set_upsert_row(row);
                condition.set_upsert_row(row);
            }
            Self::Over { expr, window } => {
                expr.set_upsert_row(row);
                window.set_upsert_row(row);
            }
            Self::FullText { column, .. } => column.set_upsert_row(row),
            Self::Column { .. }
            | Self::UnprefixedColumn { .. }
            | Self::Value(_)
            | Self::Star
            | Self::Raw(_)
            | Self::Exists { .. }
            | Self::ScalarSubquery { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------------------
// Dialect-spelled nodes
// ---------------------------------------------------------------------------------------

/// `left IS [NOT] DISTINCT FROM right`, in whichever spelling the dialect has.
fn render_distinct_from<D: Dialect>(
    out: &mut Sql<D>,
    left: &mut Operand,
    right: &mut Operand,
    negated: bool,
) {
    use besu_core::dialect::DistinctFromStyle;
    match D::DISTINCT_FROM {
        DistinctFromStyle::Standard => {
            out.push_raw("(");
            left.render(out);
            out.push_raw(if negated {
                " IS DISTINCT FROM "
            } else {
                " IS NOT DISTINCT FROM "
            });
            right.render(out);
            out.push_raw(")");
        }
        DistinctFromStyle::MySql => {
            // `<=>` is null-safe equality, so it *is* `IS NOT DISTINCT FROM` and the negated
            // form is the same comparison under `NOT`.
            if negated {
                out.push_raw("(NOT (");
            } else {
                out.push_raw("(");
            }
            left.render(out);
            out.push_raw(" <=> ");
            right.render(out);
            if negated {
                out.push_raw("))");
            } else {
                out.push_raw(")");
            }
        }
        DistinctFromStyle::Unsupported => {
            out.set_error(besu_core::Error::unsupported(alloc::format!(
                "`{}` has no null-safe comparison; compare with `eq` and an `OR` over the \
                 two `is_null` tests instead",
                D::NAME
            )));
            // A well-formed stand-in, so `inspect` and the query log still read sensibly.
            out.push_raw("(");
            left.render(out);
            out.push_raw(" <=> ");
            right.render(out);
            out.push_raw(")");
        }
    }
}

/// A regular-expression match, in whichever spelling the dialect has.
fn render_regex<D: Dialect>(
    out: &mut Sql<D>,
    left: &mut Operand,
    right: &mut Operand,
    case_insensitive: bool,
    negated: bool,
) {
    use besu_core::dialect::RegexStyle;
    match D::REGEX {
        RegexStyle::Postgres => {
            out.push_raw("(");
            left.render(out);
            out.push_raw(match (case_insensitive, negated) {
                (false, false) => " ~ ",
                (true, false) => " ~* ",
                (false, true) => " !~ ",
                (true, true) => " !~* ",
            });
            right.render(out);
            out.push_raw(")");
        }
        RegexStyle::MySql => {
            // MySQL has no operator for the negated form, and the case-insensitive form is a
            // function, so the three spellings do not compose the way PostgreSQL's do.
            if negated {
                out.push_raw("(NOT ");
            } else {
                out.push_raw("(");
            }
            if case_insensitive {
                out.push_raw("REGEXP_LIKE(");
                left.render(out);
                out.push_raw(", ");
                right.render(out);
                out.push_raw(", 'i')");
            } else {
                left.render(out);
                out.push_raw(" REGEXP ");
                right.render(out);
            }
            out.push_raw(")");
        }
        RegexStyle::Unsupported => {
            out.set_error(besu_core::Error::unsupported(alloc::format!(
                "`{}` has no regular-expression match; write it with `raw` if the server has \
                 an extension that provides one",
                D::NAME
            )));
            out.push_raw("(");
            left.render(out);
            out.push_raw(" REGEXP ");
            right.render(out);
            out.push_raw(")");
        }
    }
}

/// A field of a date or time, as `EXTRACT` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateField {
    /// The calendar year.
    Year,
    /// The quarter, 1 to 4.
    Quarter,
    /// The month, 1 to 12.
    Month,
    /// The ISO week number.
    Week,
    /// The day of the month.
    Day,
    /// The day of the week. PostgreSQL counts from Sunday, MySQL and SQLite differ; see the
    /// documentation of [`extract`].
    DayOfWeek,
    /// The day of the year, 1 to 366.
    DayOfYear,
    /// The hour, 0 to 23.
    Hour,
    /// The minute, 0 to 59.
    Minute,
    /// The second, including a fractional part where the database has one.
    Second,
    /// Seconds since the Unix epoch.
    Epoch,
}

impl DateField {
    /// The word PostgreSQL and the SQL standard use in `EXTRACT`.
    const fn sql_name(self) -> &'static str {
        match self {
            Self::Year => "YEAR",
            Self::Quarter => "QUARTER",
            Self::Month => "MONTH",
            Self::Week => "WEEK",
            Self::Day => "DAY",
            Self::DayOfWeek => "DOW",
            Self::DayOfYear => "DOY",
            Self::Hour => "HOUR",
            Self::Minute => "MINUTE",
            Self::Second => "SECOND",
            Self::Epoch => "EPOCH",
        }
    }

    /// The unit name `date_trunc` takes as its first argument.
    const fn date_trunc_name(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Quarter => "quarter",
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            // `date_trunc` has no notion of a day of the week, a day of the year or an
            // epoch, so the closest unit is used and documented rather than rejected.
            Self::DayOfWeek | Self::DayOfYear => "day",
            Self::Epoch => "second",
        }
    }

    /// The `strftime` conversion that produces the same number, as SQLite spells it.
    const fn sqlite_format(self) -> Option<&'static str> {
        match self {
            Self::Year => Some("%Y"),
            Self::Month => Some("%m"),
            // `%V` is the ISO-8601 week, which is what PostgreSQL's `EXTRACT(WEEK)`
            // returns; `%W` is Monday-of-week-0 and disagrees at every year boundary.
            Self::Week => Some("%V"),
            Self::Day => Some("%d"),
            Self::DayOfWeek => Some("%w"),
            Self::DayOfYear => Some("%j"),
            Self::Hour => Some("%H"),
            Self::Minute => Some("%M"),
            Self::Second => Some("%S"),
            Self::Epoch => Some("%s"),
            // SQLite has no quarter conversion; it is derived from the month instead.
            Self::Quarter => None,
        }
    }
}

/// `EXTRACT(field FROM expr)`, or the dialect's equivalent.
fn render_extract<D: Dialect>(out: &mut Sql<D>, field: DateField, expr: &mut Operand) {
    match D::NAME {
        "postgres" => {
            out.push_raw("EXTRACT(");
            out.push_raw(field.sql_name());
            out.push_raw(" FROM ");
            expr.render(out);
            out.push_raw(")");
        }
        "mysql" => match field {
            // MySQL's `EXTRACT` only names the simple units; the rest are functions.
            // `EXTRACT(WEEK)` uses mode 0, which is Sunday-based, while PostgreSQL's is
            // ISO-8601 — `WEEK(x, 3)` is the ISO one.
            DateField::Week => {
                out.push_raw("WEEK(");
                expr.render(out);
                out.push_raw(", 3)");
            }
            DateField::DayOfWeek => {
                out.push_raw("DAYOFWEEK(");
                expr.render(out);
                out.push_raw(")");
            }
            DateField::DayOfYear => {
                out.push_raw("DAYOFYEAR(");
                expr.render(out);
                out.push_raw(")");
            }
            DateField::Epoch => {
                out.push_raw("UNIX_TIMESTAMP(");
                expr.render(out);
                out.push_raw(")");
            }
            other => {
                out.push_raw("EXTRACT(");
                out.push_raw(other.sql_name());
                out.push_raw(" FROM ");
                expr.render(out);
                out.push_raw(")");
            }
        },
        "sqlite" if field == DateField::Quarter => {
            out.push_raw("((strftime('%m', ");
            expr.render(out);
            out.push_raw(") + 2) / 3)");
        }
        "sqlite" => match field.sqlite_format() {
            Some(format) => {
                out.push_raw("strftime('");
                out.push_raw(format);
                out.push_raw("', ");
                expr.render(out);
                out.push_raw(")");
            }
            None => {
                out.set_error(besu_core::Error::unsupported(alloc::format!(
                    "`{}` cannot extract {:?}",
                    D::NAME,
                    field
                )));
                expr.render(out);
            }
        },
        _ => {
            out.set_error(besu_core::Error::unsupported(alloc::format!(
                "`{}` has no `EXTRACT`; this dialect has not said how it reads a date field",
                D::NAME
            )));
            expr.render(out);
        }
    }
}

/// The whole days from `right` to `left`, in whichever spelling the dialect has.
fn render_date_diff<D: Dialect>(out: &mut Sql<D>, left: &mut Operand, right: &mut Operand) {
    match D::NAME {
        "postgres" => {
            // The difference of two dates is already an integer number of days.
            out.push_raw("(CAST(");
            left.render(out);
            out.push_raw(" AS DATE) - CAST(");
            right.render(out);
            out.push_raw(" AS DATE))");
        }
        "mysql" => {
            out.push_raw("DATEDIFF(");
            left.render(out);
            out.push_raw(", ");
            right.render(out);
            out.push_raw(")");
        }
        "sqlite" => {
            out.push_raw("(julianday(");
            left.render(out);
            out.push_raw(") - julianday(");
            right.render(out);
            out.push_raw("))");
        }
        _ => {
            out.set_error(besu_core::Error::unsupported(alloc::format!(
                "`{}` has no date difference; this dialect has not said how it counts days",
                D::NAME
            )));
            left.render(out);
        }
    }
}

/// `expr ± interval`, in whichever spelling the dialect has.
fn render_date_add<D: Dialect>(
    out: &mut Sql<D>,
    expr: &mut Operand,
    interval: Interval,
    subtract: bool,
    date_only: bool,
) {
    match D::NAME {
        "postgres" => {
            // `date + interval` is a `timestamp` on PostgreSQL even when both sides are
            // dates, so a helper typed for a `DATE` has to cast the result back or the
            // driver decodes a `NaiveDate` from a timestamp.
            if date_only {
                out.push_raw("CAST(");
            }
            out.push_raw("(");
            expr.render(out);
            out.push_raw(if subtract {
                " - make_interval(months => "
            } else {
                " + make_interval(months => "
            });
            out.push_raw(&alloc::format!("{}", interval.months));
            out.push_raw(", days => ");
            out.push_raw(&alloc::format!("{}", interval.days));
            out.push_raw(", secs => ");
            out.push_raw(&decimal_seconds(interval.micros));
            // `make_interval` takes `secs` as a double, and a bare decimal literal is typed
            // as `numeric`, which does not resolve the function.
            out.push_raw("::double precision))");
            if date_only {
                out.push_raw(" AS DATE)");
            }
        }
        "mysql" => {
            // MySQL takes one unit per `INTERVAL`, so a compound interval is nested. The
            // subtraction is applied once, at the outside, so the units keep their sign.
            let wrap = if subtract { "DATE_SUB(" } else { "DATE_ADD(" };
            let mut depth = 0;
            if interval.months != 0 {
                out.push_raw(wrap);
                depth += 1;
            }
            if interval.days != 0 {
                out.push_raw(wrap);
                depth += 1;
            }
            if interval.micros != 0 {
                out.push_raw(wrap);
                depth += 1;
            }
            if depth == 0 {
                // An empty interval is the value itself, but it still has to be a call so the
                // statement does not silently change shape.
                out.push_raw(if subtract { "DATE_SUB(" } else { "DATE_ADD(" });
                expr.render(out);
                out.push_raw(", INTERVAL 0 DAY)");
                return;
            }
            expr.render(out);
            if interval.micros != 0 {
                out.push_raw(", INTERVAL ");
                out.push_raw(&alloc::format!("{}", interval.micros));
                out.push_raw(" MICROSECOND)");
            }
            if interval.days != 0 {
                out.push_raw(", INTERVAL ");
                out.push_raw(&alloc::format!("{}", interval.days));
                out.push_raw(" DAY)");
            }
            if interval.months != 0 {
                out.push_raw(", INTERVAL ");
                out.push_raw(&alloc::format!("{}", interval.months));
                out.push_raw(" MONTH)");
            }
        }
        "sqlite" => {
            // SQLite's date arithmetic is a modifier list on `date`/`datetime`, and the
            // result is text, so `datetime` keeps the time of day and `date` does not.
            let function = if date_only { "date" } else { "datetime" };
            out.push_raw(function);
            out.push_raw("(");
            expr.render(out);
            let sign = if subtract { '-' } else { '+' };
            if interval.months != 0 {
                out.push_raw(&alloc::format!(", '{sign}{} months'", interval.months));
            }
            if interval.days != 0 {
                out.push_raw(&alloc::format!(", '{sign}{} days'", interval.days));
            }
            if interval.micros != 0 {
                out.push_raw(&alloc::format!(
                    ", '{sign}{} microseconds'",
                    interval.micros
                ));
            }
            out.push_raw(")");
        }
        _ => {
            out.set_error(besu_core::Error::unsupported(alloc::format!(
                "`{}` has no date arithmetic; add the interval in the application instead",
                D::NAME
            )));
            expr.render(out);
        }
    }
}

/// Write a whole number of microseconds as a decimal number of seconds.
///
/// `make_interval` takes `secs` as a double, and the value is a literal taken from an
/// [`Interval`], never from user input, so formatting it into the statement is safe.
fn decimal_seconds(micros: i64) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    let magnitude = micros.unsigned_abs();
    alloc::format!(
        "{sign}{}.{:06}",
        magnitude / 1_000_000,
        magnitude % 1_000_000
    )
}

/// A span of time added to, or subtracted from, a date or timestamp.
///
/// Months, days and microseconds are separate fields rather than one number of seconds
/// because a month is not a fixed number of days and a day is not a fixed number of seconds
/// across a daylight-saving transition. Every database keeps them separate for the same
/// reason, and this type mirrors that so `Interval::months(1)` means "a calendar month" in
/// all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interval {
    months: i32,
    days: i32,
    micros: i64,
}

impl Interval {
    /// An interval of zero, which is the value itself.
    pub const fn new() -> Self {
        Self {
            months: 0,
            days: 0,
            micros: 0,
        }
    }

    /// Another number of calendar months.
    pub const fn months(mut self, months: i32) -> Self {
        self.months = months;
        self
    }

    /// Another number of days.
    pub const fn days(mut self, days: i32) -> Self {
        self.days = days;
        self
    }

    /// Another number of microseconds.
    pub const fn micros(mut self, micros: i64) -> Self {
        self.micros = micros;
        self
    }

    /// Another number of whole seconds.
    pub const fn seconds(self, seconds: i64) -> Self {
        self.micros(seconds.saturating_mul(1_000_000))
    }

    /// Whether the interval changes nothing.
    pub const fn is_zero(&self) -> bool {
        self.months == 0 && self.days == 0 && self.micros == 0
    }
}

/// How a window frame counts rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameUnits {
    /// `ROWS`: a physical count of rows.
    Rows,
    /// `RANGE`: a distance in the ordering value.
    Range,
}

/// One end of a window frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameBound {
    /// `UNBOUNDED PRECEDING`.
    UnboundedPreceding,
    /// `n PRECEDING`.
    Preceding(u64),
    /// `CURRENT ROW`.
    CurrentRow,
    /// `n FOLLOWING`.
    Following(u64),
    /// `UNBOUNDED FOLLOWING`.
    UnboundedFollowing,
}

impl FrameBound {
    /// Write the bound, keeping the frame word that precedes it.
    fn render<D: Dialect>(&self, out: &mut Sql<D>, unit: &str) {
        match self {
            Self::UnboundedPreceding => out.push_raw("UNBOUNDED PRECEDING"),
            Self::Preceding(n) => {
                out.bind(*n as i64);
                out.push_raw(" ");
                out.push_raw(unit);
                out.push_raw(" PRECEDING");
            }
            Self::CurrentRow => out.push_raw("CURRENT ROW"),
            Self::Following(n) => {
                out.bind(*n as i64);
                out.push_raw(" ");
                out.push_raw(unit);
                out.push_raw(" FOLLOWING");
            }
            Self::UnboundedFollowing => out.push_raw("UNBOUNDED FOLLOWING"),
        }
    }
}

/// The window an `OVER` clause describes.
///
/// A window is a set of rows around the current one: which rows are peers
/// (`PARTITION BY`), in what order (`ORDER BY`), and how far either side of the current row
/// to look (`ROWS` or `RANGE`).
///
/// ```ignore
/// row_number().over(Window::new().partition_by(users.team).order_by(users.age.desc()))
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Window {
    partition_by: Vec<ExprKind>,
    order_by: Vec<OrderClause>,
    frame: Option<(FrameUnits, FrameBound, Option<FrameBound>)>,
}

impl Window {
    /// An empty window, which is the whole result set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a `PARTITION BY` expression, restarting the window for each distinct value.
    pub fn partition_by<O: IntoOrder>(mut self, expr: O) -> Self {
        self.partition_by.push(expr.into_order().expr);
        self
    }

    /// Add an `ORDER BY` term inside the window.
    pub fn order_by<O: IntoOrder>(mut self, order: O) -> Self {
        self.order_by.push(order.into_order());
        self
    }

    /// A `ROWS BETWEEN start AND end` frame.
    pub fn rows_between(mut self, start: FrameBound, end: FrameBound) -> Self {
        self.frame = Some((FrameUnits::Rows, start, Some(end)));
        self
    }

    /// A `RANGE BETWEEN start AND end` frame.
    pub fn range_between(mut self, start: FrameBound, end: FrameBound) -> Self {
        self.frame = Some((FrameUnits::Range, start, Some(end)));
        self
    }

    /// A frame that starts at `start` and runs to the current row.
    pub fn rows_from(mut self, start: FrameBound) -> Self {
        self.frame = Some((FrameUnits::Rows, start, None));
        self
    }

    fn render<D: Dialect>(&mut self, out: &mut Sql<D>) {
        let mut first = true;
        if !self.partition_by.is_empty() {
            out.push_raw("PARTITION BY ");
            for expr in &mut self.partition_by {
                if !first {
                    out.push_raw(", ");
                }
                first = false;
                expr.render(out);
            }
        }
        if !self.order_by.is_empty() {
            if !first {
                out.push_raw(" ");
            }
            out.push_raw("ORDER BY ");
            for (i, clause) in self.order_by.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                crate::query::render_order(out, clause);
            }
            first = false;
        }
        if let Some((units, start, end)) = &self.frame {
            if !first {
                out.push_raw(" ");
            }
            let (keyword, unit) = match units {
                FrameUnits::Rows => ("ROWS", "ROW"),
                FrameUnits::Range => ("RANGE", ""),
            };
            out.push_raw(keyword);
            out.push_raw(" BETWEEN ");
            start.render(out, unit);
            out.push_raw(" AND ");
            match end {
                Some(end) => end.render(out, unit),
                // `ROWS n PRECEDING` with no end runs to the current row.
                None => out.push_raw("CURRENT ROW"),
            }
        }
    }

    fn unqualify(&mut self) {
        for expr in &mut self.partition_by {
            expr.unqualify();
        }
        for clause in &mut self.order_by {
            clause.expr.unqualify();
        }
    }

    fn reads_upsert_row(&self) -> bool {
        self.partition_by.iter().any(ExprKind::reads_upsert_row)
            || self
                .order_by
                .iter()
                .any(|clause| clause.expr.reads_upsert_row())
    }

    fn set_upsert_row(&mut self, row: &'static str) {
        for expr in &mut self.partition_by {
            expr.set_upsert_row(row);
        }
        for clause in &mut self.order_by {
            clause.expr.set_upsert_row(row);
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

    /// Erase the type-state, leaving the expression the query builder renders.
    ///
    /// This is the escape hatch for the places the typed builder is in the way, chiefly
    /// [`DynamicSelect`](crate::DynamicSelect). Every other method should prefer the typed
    /// form: `into_kind` gives up the row-set and SQL-type checks, so the database is the
    /// only thing left to catch a mistake.
    pub fn into_kind(self) -> ExprKind {
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

    /// `this IS DISTINCT FROM other`: a comparison that treats `NULL` as a value.
    ///
    /// `this.ne(other)` is `UNKNOWN` when either side is `NULL`, so it never matches; this
    /// is `true` when exactly one side is `NULL`, which is what "the values differ" means.
    /// MySQL has no such keyword and spells it `NOT (this <=> other)`.
    pub fn is_distinct_from<A, M>(self, other: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, S, M>,
    {
        Expr::from_kind(ExprKind::DistinctFrom {
            left: Operand::new(self.into_kind()),
            right: Operand::new(other.into_expr().into_kind()),
            negated: true,
        })
    }

    /// `this IS NOT DISTINCT FROM other`: `=` for values, with `NULL` = `NULL` true.
    ///
    /// This is the safe way to compare a nullable column, where `this.eq(other)` would be
    /// `UNKNOWN` rather than `true` for two `NULL`s.
    pub fn is_not_distinct_from<A, M>(self, other: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, S, M>,
    {
        Expr::from_kind(ExprKind::DistinctFrom {
            left: Operand::new(self.into_kind()),
            right: Operand::new(other.into_expr().into_kind()),
            negated: false,
        })
    }

    /// `this ~ pattern` on PostgreSQL, `this REGEXP pattern` on MySQL.
    ///
    /// The pattern is bound as a parameter, so it never becomes SQL. SQLite has no regular
    /// expression matching, so the statement fails while it is built rather than at the
    /// server.
    pub fn regex_match<A, M>(self, pattern: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, Text, M>,
    {
        self.regex(pattern, false, false)
    }

    /// A case-insensitive regular-expression match: `~*` on PostgreSQL.
    pub fn regex_match_ci<A, M>(self, pattern: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, Text, M>,
    {
        self.regex(pattern, true, false)
    }

    /// A negated regular-expression match: `!~` on PostgreSQL.
    pub fn not_regex_match<A, M>(self, pattern: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, Text, M>,
    {
        self.regex(pattern, false, true)
    }

    /// A negated, case-insensitive regular-expression match: `!~*` on PostgreSQL.
    pub fn not_regex_match_ci<A, M>(self, pattern: A) -> Expr<T, Bool>
    where
        A: IntoExpr<T, Text, M>,
    {
        self.regex(pattern, true, true)
    }

    fn regex<A, M>(self, pattern: A, case_insensitive: bool, negated: bool) -> Expr<T, Bool>
    where
        A: IntoExpr<T, Text, M>,
    {
        Expr::from_kind(ExprKind::Regex {
            left: Operand::new(self.into_kind()),
            right: Operand::new(pattern.into_expr().into_kind()),
            case_insensitive,
            negated,
        })
    }

    /// `this FILTER (WHERE condition)`: restrict an aggregate to the rows that match.
    ///
    /// `FILTER` is part of the aggregate's own syntax, not a `WHERE` clause, so the rows
    /// outside the predicate still take part in the rest of the `SELECT`:
    ///
    /// ```ignore
    /// db.select(orders)
    ///     .pick((orders.customer_id, sum(orders.total).filter(orders.paid)))
    ///     .group_by(orders.customer_id)
    ///     .await?;
    /// ```
    pub fn filter<F, P>(self, condition: F) -> Expr<T, S>
    where
        F: IntoCondition<T, P>,
    {
        let _ = core::marker::PhantomData::<P>;
        let condition = Operand::new(condition.into_condition());
        Expr::from_kind(wrap_outside_cast(self.into_kind(), |expr| {
            ExprKind::Filter { expr, condition }
        }))
    }

    /// `this OVER (window)`: evaluate an aggregate or window function over a window.
    ///
    /// ```ignore
    /// row_number().over(Window::new().partition_by(users.team).order_by(users.age.asc()))
    /// ```
    pub fn over(self, window: Window) -> Expr<T, S> {
        Expr::from_kind(wrap_outside_cast(self.into_kind(), |expr| ExprKind::Over {
            expr,
            window: Box::new(window),
        }))
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
    };
}

text_values!(&str);
text_values!(String);
text_values!(&String);
text_values!(alloc::borrow::Cow<'_, str>);

/// A decimal is bound as its exact decimal text, whatever its precision and scale.
///
/// The implementation is generic over `PRECISION` and `SCALE` because the text does not
/// change with them: PostgreSQL parses the digits into its binary `NUMERIC` format and MySQL
/// lets the server coerce the string to the column's type. The previous version only covered
/// `Decimal<0, 0>`, which made every real `Decimal<10, 2>` column impossible to compare.
macro_rules! decimal_values {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<const PRECISION: u8, const SCALE: u8> IntoSqlValue<Decimal<PRECISION, SCALE>>
                for $ty
            {
                fn into_sql_value(self) -> Value {
                    Value::Decimal(self.into())
                }
            }
        )*
    };
}

decimal_values!(&str, String, &String, alloc::borrow::Cow<'_, str>);

/// A `Vec<S::Rust>` is an array of `S`.
///
/// The element's declared type travels with the values because PostgreSQL's binary array
/// header names the element OID and the server rejects a mismatch. `Vec<u8>` is deliberately
/// not covered: [`Bytes`] implements `IntoSqlValue` for it, and a byte vector means binary
/// data rather than an array of one-byte integers.
impl<S: SqlType> IntoSqlValue<Array<S>> for Vec<S::Rust>
where
    S::Rust: IntoSqlValue<S>,
    Array<S>: SqlType,
{
    fn into_sql_value(self) -> Value {
        // Each element goes through the *element's* conversion, so a `Vec<u128>` becomes
        // `UInt128` values and a `Vec<String>` in a decimal array becomes `Decimal` — the
        // encoder matches on the declared element type, and a bare `Into<Value>` would
        // produce `Text` or `UInt128` instead.
        Value::Array {
            element: S::DEF,
            values: self
                .into_iter()
                .map(IntoSqlValue::<S>::into_sql_value)
                .collect(),
        }
    }
}

impl<S: SqlType> IntoSqlValue<Array<S>> for &[S::Rust]
where
    S::Rust: Clone + IntoSqlValue<S>,
    Array<S>: SqlType,
{
    fn into_sql_value(self) -> Value {
        Value::Array {
            element: S::DEF,
            values: self
                .iter()
                .cloned()
                .map(IntoSqlValue::<S>::into_sql_value)
                .collect(),
        }
    }
}

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

impl IntoSqlValue<SqlInterval> for SqlInterval {
    fn into_sql_value(self) -> Value {
        Value::Interval(self)
    }
}

impl IntoSqlValue<Inet> for Inet {
    fn into_sql_value(self) -> Value {
        Value::Inet(self)
    }
}

impl IntoSqlValue<Cidr> for Cidr {
    fn into_sql_value(self) -> Value {
        Value::Cidr(self)
    }
}

impl<const N: u32> IntoSqlValue<Bit<N>> for BitString {
    fn into_sql_value(self) -> Value {
        Value::Bit(self)
    }
}

impl<const N: u32> IntoSqlValue<VarBit<N>> for BitString {
    fn into_sql_value(self) -> Value {
        Value::Bit(self)
    }
}

/// A range is bound with its subtype, because PostgreSQL's binary range format stores each
/// bound as the subtype's own encoding.
macro_rules! range_values {
    ($($sql:ty, $rust:ty => $element:expr, $convert:expr;)*) => {
        $(
            impl IntoSqlValue<Range<$sql>> for Range<$rust> {
                fn into_sql_value(self) -> Value {
                    Value::Range {
                        element: $element,
                        range: alloc::boxed::Box::new(self.map($convert)),
                    }
                }
            }
        )*
    };
}

range_values! {
    Int4, i32 => TypeDef::Int4, |value| Value::Int(i64::from(value));
    Int8, i64 => TypeDef::Int8, Value::Int;
    Timestamp, chrono::NaiveDateTime => TypeDef::Timestamp, Value::Timestamp;
    TimestampTz, chrono::DateTime<chrono::Utc> => TypeDef::TimestampTz, Value::TimestampTz;
    Date, chrono::NaiveDate => TypeDef::Date, Value::Date;
}

impl<const PRECISION: u8, const SCALE: u8> IntoSqlValue<Range<Decimal<PRECISION, SCALE>>>
    for Range<String>
{
    fn into_sql_value(self) -> Value {
        // The declared subtype is an unconstrained `numeric` in PostgreSQL's `numrange`; the
        // marker's precision and scale only describe the Rust field.
        Value::Range {
            element: TypeDef::Decimal(PRECISION, SCALE),
            range: alloc::boxed::Box::new(self.map(Value::Decimal)),
        }
    }
}

// An array of ranges binds each element through the range's own conversion, which is the
// blanket `Vec<S::Rust>: IntoSqlValue<Array<S>>` implementation already in this file.
impl IntoSqlValue<Geometry> for Geometry {
    fn into_sql_value(self) -> Value {
        Value::Geometry(self)
    }
}

/// A narrowed `GEOMETRY(n)` marker holds the same value as the generic one.
impl<const KIND: u8, const SRID: u32> IntoSqlValue<GeometryOf<KIND, SRID>> for Geometry {
    fn into_sql_value(self) -> Value {
        Value::Geometry(self)
    }
}

/// A `GEOGRAPHY` marker holds the same EWKB value as a `GEOMETRY` one.
impl<const KIND: u8, const SRID: u32> IntoSqlValue<GeographyOf<KIND, SRID>> for Geometry {
    fn into_sql_value(self) -> Value {
        Value::Geometry(self)
    }
}

impl IntoSqlValue<Vector> for Vector {
    fn into_sql_value(self) -> Value {
        Value::Vector(self)
    }
}

/// A `VECTOR(n)` marker holds the same value as the unbounded one.
impl<const N: u32> IntoSqlValue<VectorOf<N>> for Vector {
    fn into_sql_value(self) -> Value {
        Value::Vector(self)
    }
}

/// A vector column takes anything that converts into its components.
impl IntoSqlValue<Vector> for Vec<f32> {
    fn into_sql_value(self) -> Value {
        Value::Vector(Vector::new(self))
    }
}

impl<const N: u32> IntoSqlValue<VectorOf<N>> for Vec<f32> {
    fn into_sql_value(self) -> Value {
        Value::Vector(Vector::new(self))
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

/// Convert a derived insert's [`Set`](crate::table::Set) field using the column's SQL type.
///
/// The derive knows the column's `SqlType`, but a `Set<T>` only knows its Rust type — and the
/// same Rust type can mean two SQL types, `String` being both `TEXT` and `DECIMAL`. Going
/// through [`IntoSqlValue`] here is what makes an insert bind a `Decimal` as `NUMERIC` rather
/// than as text, which PostgreSQL would refuse.
#[doc(hidden)]
pub fn set_to_value<S: SqlType, T: IntoSqlValue<S> + Clone>(
    set: &crate::table::Set<T>,
) -> Option<Value> {
    match set {
        crate::table::Set::Unset => None,
        crate::table::Set::Null => Some(Value::Null),
        crate::table::Set::Value(value) => Some((*value).clone().into_sql_value()),
    }
}

/// Convert an `Option` from a model into a bound value, with the column's SQL type.
#[doc(hidden)]
pub fn option_to_value<S: SqlType, T: IntoSqlValue<S>>(value: Option<T>) -> Value {
    match value {
        Some(value) => value.into_sql_value(),
        None => Value::Null,
    }
}

/// Convert a model field into a bound value, with the column's SQL type.
#[doc(hidden)]
pub fn value_to_value<S: SqlType, T: IntoSqlValue<S>>(value: T) -> Value {
    value.into_sql_value()
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

/// `a IS DISTINCT FROM b`, which is `true` when exactly one side is `NULL`.
///
/// The two operands must have the same SQL type, exactly as `eq` requires. MySQL spells the
/// operation `NOT (a <=> b)`.
pub fn is_distinct_from<RowSet, SA, SB, MA, MB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
where
    SA: SqlType,
    SB: SqlType,
    A: IntoExpr<RowSet, SA, MA>,
    B: IntoExpr<RowSet, SB, MB>,
    (SA, SB): SameType,
{
    Expr::from_kind(ExprKind::DistinctFrom {
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
        negated: true,
    })
}

/// `a IS NOT DISTINCT FROM b`, which is `=` with `NULL` = `NULL` true.
pub fn is_not_distinct_from<RowSet, SA, SB, MA, MB, A, B>(a: A, b: B) -> Expr<RowSet, Bool>
where
    SA: SqlType,
    SB: SqlType,
    A: IntoExpr<RowSet, SA, MA>,
    B: IntoExpr<RowSet, SB, MB>,
    (SA, SB): SameType,
{
    Expr::from_kind(ExprKind::DistinctFrom {
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
        negated: false,
    })
}

/// `a ~ b` on PostgreSQL, `a REGEXP b` on MySQL. The pattern is a bound parameter.
pub fn regex_match<RowSet, A, MA, B, MB>(a: A, pattern: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    Expr::from_kind(ExprKind::Regex {
        left: Operand::new(a.into_typed().into_kind()),
        right: Operand::new(pattern.into_expr().into_kind()),
        case_insensitive: false,
        negated: false,
    })
}

/// A case-insensitive regular-expression match: `a ~* b` on PostgreSQL.
pub fn regex_match_ci<RowSet, A, MA, B, MB>(a: A, pattern: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    Expr::from_kind(ExprKind::Regex {
        left: Operand::new(a.into_typed().into_kind()),
        right: Operand::new(pattern.into_expr().into_kind()),
        case_insensitive: true,
        negated: false,
    })
}

/// A negated regular-expression match: `a !~ b` on PostgreSQL.
pub fn not_regex_match<RowSet, A, MA, B, MB>(a: A, pattern: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    Expr::from_kind(ExprKind::Regex {
        left: Operand::new(a.into_typed().into_kind()),
        right: Operand::new(pattern.into_expr().into_kind()),
        case_insensitive: false,
        negated: true,
    })
}

/// A negated, case-insensitive regular-expression match: `a !~* b` on PostgreSQL.
pub fn not_regex_match_ci<RowSet, A, MA, B, MB>(a: A, pattern: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    Expr::from_kind(ExprKind::Regex {
        left: Operand::new(a.into_typed().into_kind()),
        right: Operand::new(pattern.into_expr().into_kind()),
        case_insensitive: true,
        negated: true,
    })
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
    Expr::from_kind(ExprKind::AnyOf {
        expr: Operand::new(a.into_typed().into_kind()),
        element: <A::SqlType as SqlType>::DEF,
        values: values
            .into_iter()
            .map(IntoSqlValue::into_sql_value)
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
    Expr::from_kind(ExprKind::AnyOf {
        expr: Operand::new(a.into_typed().into_kind()),
        element: <A::SqlType as SqlType>::DEF,
        values: values
            .into_iter()
            .map(IntoSqlValue::into_sql_value)
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
/// This is the erased form of a [`Select`](crate::Select) or a
/// [`SetQuery`](crate::SetQuery): the row set and row type are not part of it, because the
/// statement that contains it never decodes the subquery's rows. It is rendered by the
/// enclosing statement's dialect and shares that statement's parameter numbering.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct Subquery {
    pub(crate) query: crate::query::QueryBody,
}

/// A finished query that can be nested inside another statement.
///
/// Implemented for [`Select`](crate::Select) and [`SetQuery`](crate::SetQuery), which is what
/// makes `exists(db.select(posts).filter(..))` read like the SQL it produces, and lets a
/// compound query stand wherever a `SELECT` does. The executor is dropped when the query is
/// nested: the subquery is rendered by the statement that contains it, so it shares that
/// statement's dialect and its parameters continue that statement's numbering.
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
        Subquery {
            query: crate::query::IntoQueryBody::into_query_body(self),
        }
    }
}

impl<'a, E: Executor, M, R> IntoSubquery for crate::query::SetQuery<'a, E, M, R> {
    type Row = R;

    fn into_subquery(self) -> Subquery {
        Subquery {
            query: crate::query::IntoQueryBody::into_query_body(self),
        }
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

/// `(SELECT ...)`: a subquery that produces a single value, usable wherever an expression is.
///
/// ```ignore
/// let rows: Vec<(String, i64)> = db
///     .select(users)
///     .pick((
///         users.name,
///         scalar(
///             db.select(posts)
///                 .correlate(users)
///                 .filter(posts.author_id.eq(users.id))
///                 .pick(count_all()),
///             Int8,
///         ),
///     ))
///     .all()
///     .await?;
/// ```
///
/// ```sql
/// SELECT "users"."name", (
///     SELECT COUNT(*) FROM "posts" WHERE ("posts"."author_id" = "users"."id")
/// ) FROM "users"
/// ```
///
/// # Why the type comes from the caller
///
/// [`pick`](crate::Select::pick) turns a selection into the Rust type it decodes, and that
/// loses the SQL type: `i64` is [`Int8`] as often as it is a `DECIMAL(39, 0)`, and `String`
/// is both [`Text`] and a `DECIMAL`. The `witness` argument supplies the missing half, and
/// it is either the marker itself or any column of the intended type:
///
/// ```ignore
/// scalar(q, Int8)          // COUNT(*)
/// scalar(q, users.id)      // the same type as `users.id`
/// ```
///
/// `(Q::Row, S::Rust): SameType` then checks that the subquery really decodes as the Rust
/// type the witness implies, which is the same check
/// [`in_query`](crate::Column::in_query) makes. A projection of more than one column is
/// rejected while the statement is built.
///
/// The subquery is rendered into the enclosing statement, so its parameters continue the
/// outer statement's numbering. It may be correlated with [`correlate`](crate::Select::correlate)
/// exactly like an `EXISTS` subquery.
///
/// A scalar subquery yields `NULL` when it returns no row, and this expression decodes into
/// `S::Rust` rather than `Option<S::Rust>`. If no row is possible, guard the value with
/// [`coalesce`] or make the subquery aggregate over an empty set (an aggregate always
/// returns a row).
pub fn scalar<RowSet, S: SqlType, Q, W>(query: Q, witness: W) -> Expr<RowSet, S>
where
    Q: IntoSubquery,
    W: TypeWitness<S>,
    (Q::Row, S::Rust): SameType,
{
    let _ = witness;
    Expr::from_kind(ExprKind::ScalarSubquery {
        query: Box::new(query.into_subquery()),
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

/// A one-argument function whose argument is text, so a string literal is accepted without a
/// typed expression being written out.
macro_rules! unary_text {
    ($( $(#[$attr:meta])* $name:ident = $sql:literal ),* $(,)?) => {
        $(
            $(#[$attr])*
            pub fn $name<RowSet, A, MA>(value: A) -> Expr<RowSet, Text>
            where
                A: IntoExpr<RowSet, Text, MA>,
            {
                call($sql, alloc::vec![value.into_expr().into_kind()])
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
    count -> Int8 = "count",
    /// `SQRT(expr)`.
    sqrt -> Float8 = "sqrt",
    /// `EXP(expr)`.
    exp -> Float8 = "exp",
    /// `LN(expr)`, the natural logarithm.
    ln -> Float8 = "ln",
    /// `LOG10(expr)`.
    log10 -> Float8 = "log10",
    /// `JSON_AGG(expr)`: the aggregate's values as a JSON array.
    json_agg -> Json = "json_agg",
    /// `JSONB_AGG(expr)`: the aggregate's values as a binary JSON array. On databases
    /// without a separate binary JSON type this is the same as [`json_agg`].
    jsonb_agg -> Jsonb = "jsonb_agg",
    /// `BOOL_AND(expr)`: whether every value is true.
    bool_and -> Bool = "bool_and",
    /// `BOOL_OR(expr)`: whether any value is true.
    bool_or -> Bool = "bool_or",
    /// `EVERY(expr)`, the standard's name for [`bool_and`].
    every -> Bool = "every",
}

unary_text! {
    /// `LOWER(expr)`.
    lower = "lower",
    /// `UPPER(expr)`.
    upper = "upper",
    /// `TRIM(expr)`.
    trim = "trim",
    /// `LTRIM(expr)`: remove leading spaces.
    ltrim = "ltrim",
    /// `RTRIM(expr)`: remove trailing spaces.
    rtrim = "rtrim",
    /// `BTRIM(expr)`: remove spaces from both ends, under whatever spelling the dialect has.
    btrim = "btrim",
    /// `INITCAP(expr)`: capitalise the first letter of every word. PostgreSQL only.
    initcap = "initcap",
    /// `REVERSE(expr)`.
    reverse = "reverse",
    /// `MD5(expr)`, as 32 hexadecimal characters.
    md5 = "md5",
    /// `SHA256(expr)`, as a hexadecimal digest.
    sha256 = "sha256",
    /// Base64-encode the bytes of `expr`.
    to_base64 = "to_base64",
    /// Decode base64 text back into bytes.
    from_base64 = "from_base64",
}

/// `STDDEV(expr)`, the sample standard deviation, as a double.
///
/// PostgreSQL returns `NUMERIC` for a statistical aggregate, which no driver decodes as
/// `f64`, so the result is cast in the database.
pub fn stddev<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "stddev",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `STDDEV_POP(expr)`, the population standard deviation, as a double.
pub fn stddev_pop<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "stddev_pop",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `VARIANCE(expr)`, the sample variance, as a double.
pub fn variance<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "variance",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `VAR_POP(expr)`, the population variance, as a double.
pub fn var_pop<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "var_pop",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `SIGN(expr)`: `-1`, `0` or `1`, as a whole number.
///
/// MySQL returns a `BIGINT` for `SIGN` whatever the argument is, so the result is cast to a
/// 64-bit integer on every dialect rather than following the argument's type.
pub fn sign<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_int(call::<_, Int8>(
        "sign",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `TRUNC(expr)`: round towards zero, as a double.
///
/// The result is a double on every dialect, because PostgreSQL returns `NUMERIC` for an
/// integer argument and MySQL's `TRUNCATE` always returns a decimal.
pub fn trunc<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "trunc",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `LENGTH(expr)`, the number of characters in the text form.
pub fn length<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_int(call::<_, Int8>(
        "length",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `CHAR_LENGTH(expr)`, the number of characters rather than bytes.
///
/// PostgreSQL spells it `char_length` and returns `integer`; SQLite spells it `length`. The
/// result is cast to a 64-bit integer on every dialect so one Rust type decodes all three.
pub fn char_length<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_int(call::<_, Int8>(
        "char_length",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `OCTET_LENGTH(expr)`, the number of bytes rather than characters.
///
/// SQLite has no such function, so it records `unsupported` there; MySQL spells it `LENGTH`,
/// which counts bytes for a text column.
pub fn octet_length<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_int(call::<_, Int8>(
        "octet_length",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

unary_same! {
    /// `MIN(expr)`.
    min = "min",
    /// `MAX(expr)`.
    max = "max",
    /// `ABS(expr)`.
    abs = "abs",
}

/// `ROUND(expr)`.
///
/// Cast to a double, because PostgreSQL returns `double precision` for an integer argument
/// (`pg_typeof(round(20::int4))`), so an `i64` column would decode as the wrong type.
pub fn round<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "round",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `CEIL(expr)`, cast for the same reason as [`round`].
pub fn ceil<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "ceil",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `FLOOR(expr)`, cast for the same reason as [`round`].
pub fn floor<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "floor",
        alloc::vec![value.into_typed().into_kind()],
    ))
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
        "sum",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// `AVG(expr)`, cast to a double for the same reason as [`sum`].
pub fn avg<RowSet, A, MA>(value: A) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_float(call::<_, Float8>(
        "avg",
        alloc::vec![value.into_typed().into_kind()],
    ))
}

/// Apply `wrap` to the *inside* of a cast, keeping the cast on the outside.
///
/// `sum` and friends cast their result — `CAST(SUM(x) AS double precision)` — because the
/// database's natural result type cannot be decoded. `FILTER` and `OVER` attach to the
/// aggregate, not to the cast: `CAST(SUM(x) AS double precision) FILTER (...)` is a syntax
/// error on every server, while `CAST(SUM(x) FILTER (...) AS double precision)` is what the
/// caller meant.
fn wrap_outside_cast(kind: ExprKind, wrap: impl FnOnce(Operand) -> ExprKind) -> ExprKind {
    match kind {
        ExprKind::Cast { expr, ty } => ExprKind::Cast {
            expr: Operand::new(wrap(expr)),
            ty,
        },
        other => wrap(Operand::new(other)),
    }
}

fn cast_to_float<T, S: SqlType>(expr: Expr<T, S>) -> Expr<T, Float8> {
    Expr::from_kind(ExprKind::Cast {
        expr: Operand::new(expr.into_kind()),
        ty: Float8::DEF,
    })
}

/// An exact `SUM`, choosing the result type at the call site.
///
/// The free function [`sum_as`] cannot be called with a turbofish, because Rust requires every
/// generic parameter of a function to be named once any is; this trait exists so that
/// `column.sum_as::<Decimal<20, 2>>()` names only the result type.
pub trait SumAs<RowSet, M>: TypedExpr<RowSet, M> + Sized {
    /// `SUM(self)`, decoded as `S`.
    fn sum_as<S: SqlType>(self) -> Expr<RowSet, S> {
        call("sum", alloc::vec![self.into_typed().into_kind()])
    }
}

impl<RowSet, M, A: TypedExpr<RowSet, M>> SumAs<RowSet, M> for A {}

/// An exact `AVG`; see [`SumAs`].
pub trait AvgAs<RowSet, M>: TypedExpr<RowSet, M> + Sized {
    /// `AVG(self)`, decoded as `S`.
    fn avg_as<S: SqlType>(self) -> Expr<RowSet, S> {
        call("avg", alloc::vec![self.into_typed().into_kind()])
    }
}

impl<RowSet, M, A: TypedExpr<RowSet, M>> AvgAs<RowSet, M> for A {}

/// `SUM(expr)`, decoded as `S` without a cast in the database.
///
/// Use this when a float would lose the value, which is what [`sum`] does deliberately:
/// `sum_as::<Decimal<20, 4>>(orders.total)` is an exact sum of a decimal column, while
/// `sum(orders.total)` is the same sum rounded through a double. The database's own return
/// type is what `S` has to describe; PostgreSQL's `SUM(numeric)` is an unconstrained
/// `numeric`, so any `Decimal` decodes it.
///
/// Call [`SumAs::sum_as`] on the column when a turbofish is more convenient: this function
/// has four generic parameters, and Rust requires all of them to be named together.
pub fn sum_as<S: SqlType, RowSet, A, MA>(value: A) -> Expr<RowSet, S>
where
    A: TypedExpr<RowSet, MA>,
{
    call("sum", alloc::vec![value.into_typed().into_kind()])
}

/// `AVG(expr)`, decoded as `S` without a cast in the database.
///
/// See [`sum_as`] for why an exact average needs its own entry point. PostgreSQL's
/// `AVG(numeric)` returns an unconstrained `numeric`, so `avg_as::<Decimal<20, 4>>` keeps
/// every digit the server computed.
pub fn avg_as<S: SqlType, RowSet, A, MA>(value: A) -> Expr<RowSet, S>
where
    A: TypedExpr<RowSet, MA>,
{
    call("avg", alloc::vec![value.into_typed().into_kind()])
}

/// Wrap an integer-valued expression in a cast to a 64-bit integer.
///
/// PostgreSQL returns `integer` for `length`, `strpos` and `extract`, which no driver can
/// decode as an `i64` without the cast; MySQL returns `BIGINT` for some of them and `INT` for
/// others. Casting makes the Rust type honest on every dialect.
/// Render an integer argument as a 32-bit value.
///
/// PostgreSQL declares the count parameters of `substr`, `lpad`, `left`, `split_part` and
/// friends as `integer`, and does not implicitly cast a `bigint` parameter down to it, so a
/// bound `i64` fails to resolve the function. Casting keeps one Rust argument type working
/// against all three databases.
fn narrow_int<T>(expr: Expr<T, Int8>) -> ExprKind {
    ExprKind::Cast {
        expr: Operand::new(expr.into_kind()),
        ty: TypeDef::Int4,
    }
}

fn cast_to_int<T, S: SqlType>(expr: Expr<T, S>) -> Expr<T, Int8> {
    Expr::from_kind(ExprKind::Cast {
        expr: Operand::new(expr.into_kind()),
        ty: Int8::DEF,
    })
}

/// `COUNT(*)`.
pub fn count_all<T>() -> Expr<T, Int8> {
    call("count", alloc::vec![ExprKind::Star])
}

/// `COUNT(DISTINCT expr)`.
pub fn count_distinct<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Function {
        name: "count",
        args: alloc::vec![value.into_typed().into_kind()],
        distinct: true,
    })
}

/// `CONCAT(a, b, ...)`.
///
/// Every argument is a SQL expression of type `TEXT`, which a string literal already is, so
/// `concat(users.first, " ")` compiles. Rust has no variadic functions, so the three- to
/// eight-argument forms are [`concat3`] and its siblings.
///
/// `CONCAT` ignores a `NULL` argument on PostgreSQL but returns `NULL` on MySQL and SQLite;
/// [`concat_ws`] behaves the same way everywhere and is the safer choice when a column may
/// be `NULL`.
pub fn concat<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    call(
        "concat",
        alloc::vec![a.into_expr().into_kind(), b.into_expr().into_kind()],
    )
}

/// `concat` with three arguments; see [`concat()`] for why this is a separate name.
pub fn concat3<RowSet, A, B, C, MA, MB, MC>(a: A, b: B, c: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind()
        ],
    )
}

/// `concat` with four arguments; see [`concat()`].
pub fn concat4<RowSet, A, B, C, D, MA, MB, MC, MD>(a: A, b: B, c: C, d: D) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind()
        ],
    )
}

/// `concat` with five arguments; see [`concat()`].
#[allow(clippy::too_many_arguments)]
pub fn concat5<RowSet, A, B, C, D, F, MA, MB, MC, MD, MF>(
    a: A,
    b: B,
    c: C,
    d: D,
    f: F,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
    F: IntoExpr<RowSet, Text, MF>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind(),
            f.into_expr().into_kind()
        ],
    )
}

/// `concat` with six arguments; see [`concat()`].
#[allow(clippy::too_many_arguments)]
pub fn concat6<RowSet, A, B, C, D, F, G, MA, MB, MC, MD, MF, MG>(
    a: A,
    b: B,
    c: C,
    d: D,
    f: F,
    g: G,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
    F: IntoExpr<RowSet, Text, MF>,
    G: IntoExpr<RowSet, Text, MG>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind(),
            f.into_expr().into_kind(),
            g.into_expr().into_kind()
        ],
    )
}

/// `concat` with seven arguments; see [`concat()`].
#[allow(clippy::too_many_arguments)]
pub fn concat7<RowSet, A, B, C, D, F, G, H, MA, MB, MC, MD, MF, MG, MH>(
    a: A,
    b: B,
    c: C,
    d: D,
    f: F,
    g: G,
    h: H,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
    F: IntoExpr<RowSet, Text, MF>,
    G: IntoExpr<RowSet, Text, MG>,
    H: IntoExpr<RowSet, Text, MH>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind(),
            f.into_expr().into_kind(),
            g.into_expr().into_kind(),
            h.into_expr().into_kind()
        ],
    )
}

/// `concat` with eight arguments; see [`concat()`].
#[allow(clippy::too_many_arguments)]
pub fn concat8<RowSet, A, B, C, D, F, G, H, I, MA, MB, MC, MD, MF, MG, MH, MI>(
    a: A,
    b: B,
    c: C,
    d: D,
    f: F,
    g: G,
    h: H,
    i: I,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
    F: IntoExpr<RowSet, Text, MF>,
    G: IntoExpr<RowSet, Text, MG>,
    H: IntoExpr<RowSet, Text, MH>,
    I: IntoExpr<RowSet, Text, MI>,
{
    call(
        "concat",
        alloc::vec![
            a.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind(),
            f.into_expr().into_kind(),
            g.into_expr().into_kind(),
            h.into_expr().into_kind(),
            i.into_expr().into_kind()
        ],
    )
}

/// `CONCAT_WS(separator, a, b, ...)`: join the arguments with `separator`, skipping `NULL`s.
///
/// Unlike [`concat()`], `CONCAT_WS` ignores `NULL` arguments on every database, which makes it
/// the portable way to join a group of columns that may be missing.
pub fn concat_ws<RowSet, A, B, C, MA, MB, MC>(separator: A, b: B, c: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "concat_ws",
        alloc::vec![
            separator.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind()
        ],
    )
}

/// `concat_ws` over three values; see [`concat_ws`].
pub fn concat_ws4<RowSet, A, B, C, D, MA, MB, MC, MD>(
    separator: A,
    b: B,
    c: C,
    d: D,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
    D: IntoExpr<RowSet, Text, MD>,
{
    call(
        "concat_ws",
        alloc::vec![
            separator.into_expr().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind()
        ],
    )
}

/// `COALESCE(a, b)`, falling back to `b` when `a` is `NULL`.
pub fn coalesce<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "coalesce",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// `coalesce` with three arguments; see [`coalesce`] for why this is a separate name.
pub fn coalesce3<RowSet, A, B, C, MA, MB, MC>(a: A, b: B, c: C) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
    C: IntoExpr<RowSet, A::SqlType, MC>,
{
    call(
        "coalesce",
        alloc::vec![
            a.into_typed().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind()
        ],
    )
}

/// `coalesce` with four arguments; see [`coalesce`].
pub fn coalesce4<RowSet, A, B, C, D, MA, MB, MC, MD>(
    a: A,
    b: B,
    c: C,
    d: D,
) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
    C: IntoExpr<RowSet, A::SqlType, MC>,
    D: IntoExpr<RowSet, A::SqlType, MD>,
{
    call(
        "coalesce",
        alloc::vec![
            a.into_typed().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind()
        ],
    )
}

/// `coalesce` with five arguments; see [`coalesce`].
#[allow(clippy::too_many_arguments)]
pub fn coalesce5<RowSet, A, B, C, D, F, MA, MB, MC, MD, MF>(
    a: A,
    b: B,
    c: C,
    d: D,
    f: F,
) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
    C: IntoExpr<RowSet, A::SqlType, MC>,
    D: IntoExpr<RowSet, A::SqlType, MD>,
    F: IntoExpr<RowSet, A::SqlType, MF>,
{
    call(
        "coalesce",
        alloc::vec![
            a.into_typed().into_kind(),
            b.into_expr().into_kind(),
            c.into_expr().into_kind(),
            d.into_expr().into_kind(),
            f.into_expr().into_kind()
        ],
    )
}

// ---------------------------------------------------------------------------------------
// String functions
// ---------------------------------------------------------------------------------------

/// `SUBSTR(value, start, length)`: the characters from `start` for `length`, both 1-based.
///
/// A negative `start` counts from the end on every dialect that has this function.
pub fn substr<RowSet, A, B, C, MA, MB, MC>(value: A, start: B, length: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
    C: IntoExpr<RowSet, Int8, MC>,
{
    call(
        "substr",
        alloc::vec![
            value.into_expr().into_kind(),
            narrow_int(start.into_expr()),
            narrow_int(length.into_expr())
        ],
    )
}

/// `SUBSTR(value, start)`: everything from `start` to the end of the string.
pub fn substring<RowSet, A, B, MA, MB>(value: A, start: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "substr",
        alloc::vec![value.into_expr().into_kind(), narrow_int(start.into_expr())],
    )
}

/// `REPLACE(value, from, to)`: every occurrence of `from` replaced by `to`.
pub fn replace<RowSet, A, B, C, MA, MB, MC>(value: A, from: B, to: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "replace",
        alloc::vec![
            value.into_expr().into_kind(),
            from.into_expr().into_kind(),
            to.into_expr().into_kind()
        ],
    )
}

/// `LEFT(value, n)`: the first `n` characters. SQLite has no such function.
pub fn left<RowSet, A, B, MA, MB>(value: A, n: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "left",
        alloc::vec![value.into_expr().into_kind(), narrow_int(n.into_expr())],
    )
}

/// `RIGHT(value, n)`: the last `n` characters. SQLite has no such function.
pub fn right<RowSet, A, B, MA, MB>(value: A, n: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "right",
        alloc::vec![value.into_expr().into_kind(), narrow_int(n.into_expr())],
    )
}

/// `LPAD(value, length, fill)`: pad on the left with `fill` until `length` characters.
pub fn lpad<RowSet, A, B, C, MA, MB, MC>(value: A, length: B, fill: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "lpad",
        alloc::vec![
            value.into_expr().into_kind(),
            narrow_int(length.into_expr()),
            fill.into_expr().into_kind()
        ],
    )
}

/// `RPAD(value, length, fill)`: pad on the right with `fill` until `length` characters.
pub fn rpad<RowSet, A, B, C, MA, MB, MC>(value: A, length: B, fill: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "rpad",
        alloc::vec![
            value.into_expr().into_kind(),
            narrow_int(length.into_expr()),
            fill.into_expr().into_kind()
        ],
    )
}

/// `strpos(haystack, needle)`: the 1-based position of `needle`, or `0` when absent.
///
/// The argument order is the same on every dialect — MySQL's `INSTR` and SQLite's `instr`
/// happen to agree with PostgreSQL's `strpos` — so this is one of the few string functions
/// that needs no reordering.
pub fn strpos<RowSet, A, B, MA, MB>(haystack: A, needle: B) -> Expr<RowSet, Int8>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    cast_to_int(call::<_, Int8>(
        "strpos",
        alloc::vec![
            haystack.into_expr().into_kind(),
            needle.into_expr().into_kind()
        ],
    ))
}

/// `SPLIT_PART(value, delimiter, n)`: the `n`th field, 1-based, or an empty string.
pub fn split_part<RowSet, A, B, C, MA, MB, MC>(value: A, delimiter: B, n: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Int8, MC>,
{
    call(
        "split_part",
        alloc::vec![
            value.into_expr().into_kind(),
            delimiter.into_expr().into_kind(),
            narrow_int(n.into_expr())
        ],
    )
}

/// `TRANSLATE(value, from, to)`: replace each character of `from` with the character of `to`
/// in the same position. PostgreSQL only.
pub fn translate<RowSet, A, B, C, MA, MB, MC>(value: A, from: B, to: C) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "translate",
        alloc::vec![
            value.into_expr().into_kind(),
            from.into_expr().into_kind(),
            to.into_expr().into_kind()
        ],
    )
}

/// `REPEAT(value, n)`: `value` repeated `n` times.
pub fn repeat<RowSet, A, B, MA, MB>(value: A, n: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "repeat",
        alloc::vec![value.into_expr().into_kind(), narrow_int(n.into_expr())],
    )
}

/// `REGEXP_REPLACE(value, pattern, replacement)`: replace every match of `pattern`.
pub fn regexp_replace<RowSet, A, B, C, MA, MB, MC>(
    value: A,
    pattern: B,
    replacement: C,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    C: IntoExpr<RowSet, Text, MC>,
{
    call(
        "regexp_replace",
        alloc::vec![
            value.into_expr().into_kind(),
            pattern.into_expr().into_kind(),
            replacement.into_expr().into_kind()
        ],
    )
}

/// `REGEXP_SUBSTR(value, pattern)`: the first part of `value` that matches `pattern`.
///
/// MySQL has this function; PostgreSQL reaches the same result through `substring(... from
/// ...)`, which is not a function call, so this records `unsupported` there.
pub fn regexp_substr<RowSet, A, B, MA, MB>(value: A, pattern: B) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    call(
        "regexp_substr",
        alloc::vec![
            value.into_expr().into_kind(),
            pattern.into_expr().into_kind()
        ],
    )
}

/// `DATE_FORMAT(value, format)`: format a date with MySQL's `%`-style codes.
pub fn date_format<RowSet, A, B, MA, MB>(value: A, format: B) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    call(
        "date_format",
        alloc::vec![
            value.into_typed().into_kind(),
            format.into_expr().into_kind()
        ],
    )
}

// ---------------------------------------------------------------------------------------
// Numeric functions
// ---------------------------------------------------------------------------------------

/// `NULLIF(a, b)`: `a`, or `NULL` when the two are equal.
pub fn nullif<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "nullif",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// `GREATEST(a, b)`: the larger of the two, ignoring `NULL`s in PostgreSQL.
pub fn greatest<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "greatest",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// `LEAST(a, b)`: the smaller of the two, ignoring `NULL`s in PostgreSQL.
pub fn least<RowSet, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, A::SqlType, MB>,
{
    call(
        "least",
        alloc::vec![a.into_typed().into_kind(), b.into_expr().into_kind()],
    )
}

/// `ROUND(value, digits)`: round to `digits` decimal places.
pub fn round_to<RowSet, A, B, MA, MB>(value: A, digits: B) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    // Always a double, because PostgreSQL's two-argument `round` is `numeric` and the renderer
    // casts its argument to get there; returning the argument's type would promise a type the
    // database does not produce. Use a `Decimal` column if you need exactness.
    cast_to_float(call::<_, Float8>(
        "round",
        alloc::vec![
            value.into_typed().into_kind(),
            narrow_int(digits.into_expr())
        ],
    ))
}

/// `POWER(value, exponent)`.
pub fn power<RowSet, A, B, MA, MB>(value: A, exponent: B) -> Expr<RowSet, Float8>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "power",
        alloc::vec![
            value.into_typed().into_kind(),
            exponent.into_expr().into_kind()
        ],
    )
}

/// The logarithm of `value` to `base`.
///
/// This is written as `LN(value) / LN(base)` rather than calling `LOG`, because there is no
/// portable spelling: PostgreSQL's two-argument `log` wants `numeric`, MySQL's `LOG(b, x)`
/// wants doubles, and the one-argument form means base 10 on PostgreSQL and the natural
/// logarithm on MySQL. Two natural logarithms and a division mean the same thing everywhere
/// and cost nothing worth measuring.
///
/// Use [`ln`] or [`log10`] where you mean one of those specifically.
pub fn log<RowSet, A, B, MA, MB>(base: A, value: B) -> Expr<RowSet, Float8>
where
    A: IntoExpr<RowSet, Float8, MA>,
    B: TypedExpr<RowSet, MB>,
{
    ln(value).divide(ln(base.into_expr()))
}

// ---------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------

/// `CURRENT_DATE`: the database's current date.
///
/// This is a keyword rather than a call, so the dialect writes it without parentheses.
pub fn current_date<T>() -> Expr<T, Date> {
    call("current_date", Vec::new())
}

/// `DATE_TRUNC(field, value)`: truncate a timestamp to the start of `field`.
///
/// This is a PostgreSQL function; MySQL and SQLite reach the same result through
/// `DATE_FORMAT` and `strftime`, which produce text, so this records `unsupported` there
/// rather than returning something of a different type.
pub fn date_trunc<RowSet, A, MA>(field: DateField, value: A) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::Function {
        name: "date_trunc",
        args: alloc::vec![
            ExprKind::Raw(alloc::format!("'{}'", field.date_trunc_name())),
            value.into_typed().into_kind()
        ],
        distinct: false,
    })
}

/// `EXTRACT(field FROM value)` as a whole number.
///
/// The fields that are not universally supported are spelled by each dialect: MySQL uses
/// `DAYOFWEEK`, `DAYOFYEAR` and `UNIX_TIMESTAMP`, and SQLite uses `strftime`.
///
/// `Week` is the ISO-8601 week everywhere: `EXTRACT(WEEK)` on PostgreSQL, `WEEK(x, 3)` on
/// MySQL and `%V` on SQLite, so the three agree at year boundaries.
///
/// `DayOfWeek` is the exception: the three databases number the days differently —
/// PostgreSQL counts from Sunday, MySQL from Sunday with `1` as the first day, and SQLite
/// from Sunday with `0` — so only compare a `DayOfWeek` with another of the same database.
pub fn extract<RowSet, A, MA>(field: DateField, value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    cast_to_int(Expr::<_, Int8>::from_kind(ExprKind::Extract {
        field,
        expr: Operand::new(value.into_typed().into_kind()),
    }))
}

/// `DATEDIFF(left, right)`: the whole days from `right` to `left`.
pub fn datediff<RowSet, A, B, MA, MB>(left: A, right: B) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
    B: TypedExpr<RowSet, MB>,
{
    cast_to_int(Expr::<_, Int8>::from_kind(ExprKind::DateDiff {
        left: Operand::new(left.into_typed().into_kind()),
        right: Operand::new(right.into_typed().into_kind()),
    }))
}

/// `value + interval`: shift a date or timestamp forward.
///
/// The result keeps the value's SQL type, because shifting a `DATE` by a whole number of
/// days is still a date. An interval with a time component added to a date truncates to the
/// date on SQLite, which is why it is safer to use a timestamp when the interval is not a
/// whole number of days.
pub fn date_add<RowSet, A, MA>(value: A, interval: Interval) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    date_shift(value, interval, false)
}

/// `value - interval`: shift a date or timestamp backward.
pub fn date_sub<RowSet, A, MA>(value: A, interval: Interval) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    date_shift(value, interval, true)
}

fn date_shift<RowSet, A, MA>(
    value: A,
    interval: Interval,
    subtract: bool,
) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    let date_only = matches!(A::SqlType::DEF, TypeDef::Date);
    Expr::from_kind(ExprKind::DateAdd {
        expr: Operand::new(value.into_typed().into_kind()),
        interval,
        subtract,
        date_only,
    })
}

/// `UNIX_TIMESTAMP(value)`: seconds since the Unix epoch. MySQL only.
pub fn unix_timestamp<RowSet, A, MA>(value: A) -> Expr<RowSet, Int8>
where
    A: TypedExpr<RowSet, MA>,
{
    call(
        "unix_timestamp",
        alloc::vec![value.into_typed().into_kind()],
    )
}

/// `FROM_UNIXTIME(seconds)`: the timestamp at a Unix time. MySQL only.
pub fn from_unixtime<RowSet, A, MA>(value: A) -> Expr<RowSet, Timestamp>
where
    A: TypedExpr<RowSet, MA>,
{
    call("from_unixtime", alloc::vec![value.into_typed().into_kind()])
}

// ---------------------------------------------------------------------------------------
// Aggregate functions
// ---------------------------------------------------------------------------------------

/// `string_agg(value, separator)`: the group's text joined by `separator`.
///
/// MySQL spells this `GROUP_CONCAT(value SEPARATOR separator)`, a different grammar for the
/// same operation, which is why it is one helper rather than two.
///
/// The separator is written into the statement rather than bound, because MySQL's
/// `SEPARATOR` takes a string literal and nothing else. It is a `&'static str`, so it is a
/// literal at the call site; a `'` inside it is doubled. A separator containing a backslash
/// is not portable: MySQL reads it as an escape and PostgreSQL does not.
pub fn string_agg<RowSet, A, MA>(value: A, separator: &'static str) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Text, MA>,
{
    call(
        "string_agg",
        alloc::vec![
            value.into_expr().into_kind(),
            ExprKind::Raw(alloc::format!("'{}'", separator.replace('\'', "''")))
        ],
    )
}

/// `JSON_OBJECT(key, value)`: a one-entry JSON object.
pub fn json_object<RowSet, A, B, MA, MB>(key: A, value: B) -> Expr<RowSet, Json>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: TypedExpr<RowSet, MB>,
{
    call(
        "json_object",
        alloc::vec![key.into_expr().into_kind(), value.into_typed().into_kind()],
    )
}

// ---------------------------------------------------------------------------------------
// Window functions
// ---------------------------------------------------------------------------------------

/// `ROW_NUMBER() OVER (...)`: the row's position in the window, starting at 1.
pub fn row_number<T>() -> Expr<T, Int8> {
    call("row_number", Vec::new())
}

/// `RANK() OVER (...)`: the row's rank, with ties sharing a rank and leaving gaps.
pub fn rank<T>() -> Expr<T, Int8> {
    call("rank", Vec::new())
}

/// `DENSE_RANK() OVER (...)`: the row's rank, with ties sharing a rank and no gaps.
pub fn dense_rank<T>() -> Expr<T, Int8> {
    call("dense_rank", Vec::new())
}

/// `LAG(value) OVER (...)`: the previous row's value in the window.
pub fn lag<RowSet, A, MA>(value: A) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    call("lag", alloc::vec![value.into_typed().into_kind()])
}

/// `LAG(value, n) OVER (...)`: the value `n` rows before this one.
pub fn lag_by<RowSet, A, B, MA, MB>(value: A, n: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "lag",
        alloc::vec![value.into_typed().into_kind(), narrow_int(n.into_expr())],
    )
}

/// `LEAD(value) OVER (...)`: the next row's value in the window.
pub fn lead<RowSet, A, MA>(value: A) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
{
    call("lead", alloc::vec![value.into_typed().into_kind()])
}

/// `LEAD(value, n) OVER (...)`: the value `n` rows after this one.
pub fn lead_by<RowSet, A, B, MA, MB>(value: A, n: B) -> Expr<RowSet, A::SqlType>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Int8, MB>,
{
    call(
        "lead",
        alloc::vec![value.into_typed().into_kind(), narrow_int(n.into_expr())],
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

/// Turn a dotted string into a structured path.
///
/// This is what the older [`json_get`] signature uses; a key that contains a dot cannot be
/// expressed this way, which is why [`JsonPath`] exists.
fn dotted_path(path: &str) -> JsonPath {
    let mut out = JsonPath::root();
    for key in path.split('.') {
        out = out.key(key);
    }
    out
}

/// The JSON value at `path` inside `document`, keeping its JSON type.
///
/// `path` is a dotted key path — `"a.b"` reaches `document["a"]["b"]` — and must be a
/// literal, because it is rendered into the SQL rather than bound. A key containing a dot is
/// not expressible this way; use [`json_at`] with a [`JsonPath`] for that.
pub fn json_get<RowSet, A, MA>(document: A, path: &'static str) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    json_at(document, dotted_path(path))
}

/// The text at `path` inside `document`, unquoted.
///
/// This is `->>` rather than `->`: a JSON string comes back without its quotes, which is
/// almost always what you want when comparing against a `String` column or parameter.
pub fn json_text<RowSet, A, MA>(document: A, path: &'static str) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
{
    json_at_text(document, dotted_path(path))
}

/// The JSON value at a structured path, keeping its JSON type.
///
/// Unlike [`json_get`], the path is built from segments, so a key that contains a dot, a
/// comma or a quote is expressible and each dialect renders the path in its own grammar.
pub fn json_at<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPath {
        expr: Operand::new(document.into_typed().into_kind()),
        path,
        as_text: false,
    })
}

/// The text at a structured path, unquoted.
pub fn json_at_text<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPath {
        expr: Operand::new(document.into_typed().into_kind()),
        path,
        as_text: true,
    })
}

/// `document -> key`: the JSON value under a key that is not known until run time.
///
/// The key is bound as a parameter, so it is never SQL text. Use [`json_at`] when the path is
/// a literal and contains several segments.
pub fn json_key<RowSet, A, MA>(document: A, key: impl Into<String>) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonKey {
        expr: Operand::new(document.into_typed().into_kind()),
        key: Value::Text(key.into()),
        as_text: false,
    })
}

/// `document ->> key`: the text under a bound key, unquoted.
pub fn json_key_text<RowSet, A, MA>(document: A, key: impl Into<String>) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonKey {
        expr: Operand::new(document.into_typed().into_kind()),
        key: Value::Text(key.into()),
        as_text: true,
    })
}

/// `a @> b` on PostgreSQL, `JSON_CONTAINS(a, b)` on MySQL: does `a` contain `b`?
pub fn contains_json<RowSet, A, B, MA, MB>(document: A, candidate: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Json, MB>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::Contains,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Operand(Operand::new(candidate.into_expr().into_kind())),
    })
}

/// `a <@ b` on PostgreSQL, `JSON_CONTAINS(b, a)` on MySQL: is `a` contained in `b`?
pub fn json_contained_by<RowSet, A, B, MA, MB>(document: A, container: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Json, MB>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::ContainedBy,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Operand(Operand::new(container.into_expr().into_kind())),
    })
}

/// `document ? key` on PostgreSQL, `JSON_CONTAINS_PATH` on MySQL: is `key` present?
///
/// The key is a name rather than a path, and it is not bound on MySQL because the function
/// needs a path expression; it is escaped into `$."key"`, so a key containing a dot or a
/// quote still means the one member.
pub fn json_has_key<RowSet, A, MA>(document: A, key: impl Into<String>) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::HasKey,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Key(key.into()),
    })
}

/// `document ?| keys` on PostgreSQL: is at least one of the keys present?
pub fn json_has_any<RowSet, A, I, K, MA>(document: A, keys: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    I: IntoIterator<Item = K>,
    K: Into<String>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::HasAnyKey,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Keys(keys.into_iter().map(Into::into).collect()),
    })
}

/// `document ?& keys` on PostgreSQL: are all of the keys present?
pub fn json_has_all<RowSet, A, I, K, MA>(document: A, keys: I) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    I: IntoIterator<Item = K>,
    K: Into<String>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::HasAllKey,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Keys(keys.into_iter().map(Into::into).collect()),
    })
}

/// `JSON_OVERLAPS(a, b)` on MySQL: do the documents share an element or member?
pub fn json_overlaps<RowSet, A, B, MA, MB>(document: A, other: B) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Json, MB>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::Overlaps,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Operand(Operand::new(other.into_expr().into_kind())),
    })
}

/// `document #- path` on PostgreSQL, `JSON_REMOVE` on MySQL: remove a path.
pub fn json_strip<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::Strip,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Path(path),
    })
}

/// `document @? predicate` on PostgreSQL: does the document match a JSON path predicate?
///
/// The predicate is bound as a parameter. MySQL has no equivalent, so the statement records
/// an `unsupported` error there rather than emitting PostgreSQL syntax.
pub fn json_matches_path<RowSet, A, MA>(
    document: A,
    predicate: impl Into<String>,
) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::MatchesPath,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Key(predicate.into()),
    })
}

/// `document @@ predicate` on PostgreSQL: does the document satisfy a `jsonpath` match?
///
/// The predicate is bound as a parameter; MySQL has no equivalent.
pub fn json_matches<RowSet, A, MA>(document: A, predicate: impl Into<String>) -> Expr<RowSet, Bool>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonOp {
        op: JsonOperator::Matches,
        left: Operand::new(document.into_typed().into_kind()),
        right: JsonRight::Key(predicate.into()),
    })
}

/// `JSON_SET(document, path, value)` on MySQL and SQLite, `JSONB_SET(...)` on PostgreSQL.
///
/// Every dialect creates a missing path, which is the behaviour that makes the helper useful
/// for building a document up over several updates.
pub fn json_set<RowSet, A, B, MA, MB>(document: A, path: JsonPath, value: B) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Json, MB>,
{
    Expr::from_kind(ExprKind::JsonPathFunction {
        name: "json_set",
        document: Operand::new(document.into_typed().into_kind()),
        path,
        extra: alloc::vec![Operand::new(value.into_expr().into_kind())],
    })
}

/// `JSON_REMOVE` on MySQL and SQLite: remove a path, without knowing its value.
///
/// This is [`json_strip`] under its MySQL spelling, so the same call works on both.
pub fn json_remove<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPathFunction {
        name: "json_remove",
        document: Operand::new(document.into_typed().into_kind()),
        path,
        extra: Vec::new(),
    })
}

/// `JSON_VALUE(document, path)` on MySQL: the scalar at a path, as text.
pub fn json_value<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Text>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPathFunction {
        name: "json_value",
        document: Operand::new(document.into_typed().into_kind()),
        path,
        extra: Vec::new(),
    })
}

/// `JSON_QUERY(document, path)` on MySQL: the JSON value or array at a path.
pub fn json_query<RowSet, A, MA>(document: A, path: JsonPath) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    Expr::from_kind(ExprKind::JsonPathFunction {
        name: "json_query",
        document: Operand::new(document.into_typed().into_kind()),
        path,
        extra: Vec::new(),
    })
}

/// `JSON_MERGE_PATCH(a, b)`: apply `b` to `a` as an RFC 7396 merge patch.
///
/// MySQL spells it as a function; PostgreSQL has no merge-patch operator, and SQLite calls
/// it `json_patch`, which is a different spelling of the same idea.
pub fn json_merge_patch<RowSet, A, B, MA, MB>(document: A, patch: B) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
    B: IntoExpr<RowSet, Json, MB>,
{
    call(
        "json_merge_patch",
        alloc::vec![
            document.into_typed().into_kind(),
            patch.into_expr().into_kind(),
        ],
    )
}

/// `JSON_ARRAYAGG(expr)`: the aggregate's values as one JSON array.
///
/// This is the same operation as [`json_agg`] and exists so that a query can name the MySQL
/// spelling; on PostgreSQL and SQLite both map onto the aggregate those servers provide.
pub fn json_array_agg<RowSet, A, MA>(value: A) -> Expr<RowSet, Json>
where
    A: TypedExpr<RowSet, MA>,
{
    call(
        "json_array_agg",
        alloc::vec![value.into_typed().into_kind()],
    )
}

/// `JSON_OBJECTAGG(key, value)`: the aggregate's key/value pairs as one JSON object.
pub fn json_object_agg<RowSet, A, B, MA, MB>(key: A, value: B) -> Expr<RowSet, Json>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: TypedExpr<RowSet, MB>,
{
    call(
        "json_object_agg",
        alloc::vec![key.into_expr().into_kind(), value.into_typed().into_kind()],
    )
}

// ---------------------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------------------

/// `a @> b`: does array `a` contain every element of `b`?
pub fn array_contains<RowSet, S: SqlType, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    Expr::from_kind(ExprKind::ArrayOp {
        op: "@>",
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
    })
}

/// `a <@ b`: is every element of array `a` also in `b`?
pub fn array_contained_by<RowSet, S: SqlType, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    Expr::from_kind(ExprKind::ArrayOp {
        op: "<@",
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
    })
}

/// `a && b`: do arrays `a` and `b` have any element in common?
pub fn array_overlaps<RowSet, S: SqlType, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Bool>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    Expr::from_kind(ExprKind::ArrayOp {
        op: "&&",
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
    })
}

/// `a || b`: concatenate two arrays.
pub fn array_concat<RowSet, S: SqlType, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    Expr::from_kind(ExprKind::ArrayOp {
        op: "||",
        left: Operand::new(a.into_expr().into_kind()),
        right: Operand::new(b.into_expr().into_kind()),
    })
}

/// `array[index]`: the element at a one-based position.
///
/// PostgreSQL arrays are one-based, and an out-of-range subscript is `NULL` rather than an
/// error. The expression's type does not model that `NULL`, exactly as `MIN` and a column
/// read from an optional join do not.
pub fn array_get<RowSet, S: SqlType, A, MA>(array: A, index: i32) -> Expr<RowSet, S>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    Array<S>: SqlType,
{
    Expr::from_kind(ExprKind::Subscript {
        array: Operand::new(array.into_expr().into_kind()),
        index: Operand::new(ExprKind::Value(Value::Int(i64::from(index)))),
    })
}

/// `array_length(array, dimension)`, or `NULL` when the array has no such dimension.
pub fn array_length<RowSet, S: SqlType, A, MA>(array: A, dimension: i32) -> Expr<RowSet, Int8>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    Array<S>: SqlType,
{
    // The dimension is written as a literal: PostgreSQL declares the argument `integer`, and
    // a bound `i64` parameter would not resolve the function without a cast.
    cast_to_int(call::<_, Int8>(
        "array_length",
        alloc::vec![
            array.into_expr().into_kind(),
            ExprKind::Raw(alloc::format!("{dimension}")),
        ],
    ))
}

/// `cardinality(array)`: the total number of elements across every dimension.
pub fn cardinality<RowSet, S: SqlType, A, MA>(array: A) -> Expr<RowSet, Int8>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    Array<S>: SqlType,
{
    cast_to_int(call::<_, Int8>(
        "cardinality",
        alloc::vec![array.into_expr().into_kind()],
    ))
}

/// `array_append(array, value)`.
pub fn array_append<RowSet, S: SqlType, A, B, MA, MB>(array: A, value: B) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, S, MB>,
    Array<S>: SqlType,
{
    call(
        "array_append",
        alloc::vec![array.into_expr().into_kind(), value.into_expr().into_kind(),],
    )
}

/// `array_prepend(value, array)`.
pub fn array_prepend<RowSet, S: SqlType, A, B, MA, MB>(value: A, array: B) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, S, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    call(
        "array_prepend",
        alloc::vec![value.into_expr().into_kind(), array.into_expr().into_kind(),],
    )
}

/// `array_cat(a, b)`: concatenate two arrays.
pub fn array_cat<RowSet, S: SqlType, A, B, MA, MB>(a: A, b: B) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Array<S>, MB>,
    Array<S>: SqlType,
{
    call(
        "array_cat",
        alloc::vec![a.into_expr().into_kind(), b.into_expr().into_kind()],
    )
}

/// `array_position(array, value)`: the one-based position of the first match, or `NULL`.
pub fn array_position<RowSet, S: SqlType, A, B, MA, MB>(array: A, value: B) -> Expr<RowSet, Int8>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, S, MB>,
    Array<S>: SqlType,
{
    cast_to_int(call::<_, Int8>(
        "array_position",
        alloc::vec![array.into_expr().into_kind(), value.into_expr().into_kind(),],
    ))
}

/// `array_remove(array, value)`: the array without every occurrence of `value`.
pub fn array_remove<RowSet, S: SqlType, A, B, MA, MB>(array: A, value: B) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, S, MB>,
    Array<S>: SqlType,
{
    call(
        "array_remove",
        alloc::vec![array.into_expr().into_kind(), value.into_expr().into_kind(),],
    )
}

/// `array_replace(array, from, to)`.
pub fn array_replace<RowSet, S: SqlType, A, B, C, MA, MB, MC>(
    array: A,
    from: B,
    to: C,
) -> Expr<RowSet, Array<S>>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, S, MB>,
    C: IntoExpr<RowSet, S, MC>,
    Array<S>: SqlType,
{
    call(
        "array_replace",
        alloc::vec![
            array.into_expr().into_kind(),
            from.into_expr().into_kind(),
            to.into_expr().into_kind(),
        ],
    )
}

/// `array_to_string(array, delimiter)`.
pub fn array_to_string<RowSet, S: SqlType, A, B, MA, MB>(
    array: A,
    delimiter: B,
) -> Expr<RowSet, Text>
where
    A: IntoExpr<RowSet, Array<S>, MA>,
    B: IntoExpr<RowSet, Text, MB>,
    Array<S>: SqlType,
{
    call(
        "array_to_string",
        alloc::vec![
            array.into_expr().into_kind(),
            delimiter.into_expr().into_kind(),
        ],
    )
}

/// `string_to_array(text, delimiter)`.
pub fn string_to_array<RowSet, A, B, MA, MB>(value: A, delimiter: B) -> Expr<RowSet, Array<Text>>
where
    A: IntoExpr<RowSet, Text, MA>,
    B: IntoExpr<RowSet, Text, MB>,
{
    call(
        "string_to_array",
        alloc::vec![
            value.into_expr().into_kind(),
            delimiter.into_expr().into_kind(),
        ],
    )
}

/// `array_agg(expr)`: the aggregate's values as an array.
pub fn array_agg<RowSet, A, MA>(value: A) -> Expr<RowSet, Array<A::SqlType>>
where
    A: TypedExpr<RowSet, MA>,
    Array<A::SqlType>: SqlType,
{
    call("array_agg", alloc::vec![value.into_typed().into_kind()])
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

    /// `IS DISTINCT FROM other`: `<>` that treats `NULL` as a value.
    pub fn is_distinct_from<RowSet, P, B, MB>(self, other: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, S, MB>,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::DistinctFrom {
            left: Operand::new(self.into_expr_kind()),
            right: Operand::new(other.into_expr().into_kind()),
            negated: true,
        })
    }

    /// `IS NOT DISTINCT FROM other`: `=` that treats `NULL` as a value.
    pub fn is_not_distinct_from<RowSet, P, B, MB>(self, other: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, S, MB>,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::DistinctFrom {
            left: Operand::new(self.into_expr_kind()),
            right: Operand::new(other.into_expr().into_kind()),
            negated: false,
        })
    }

    /// `REGEXP` match against `pattern`, in the dialect's spelling.
    pub fn regex_match<RowSet, P, B, MB>(self, pattern: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, Text, MB>,
    {
        let _ = PhantomData::<P>;
        crate::expr::regex_match(self.as_expr::<RowSet>(), pattern)
    }

    /// A case-insensitive `REGEXP` match against `pattern`.
    pub fn regex_match_ci<RowSet, P, B, MB>(self, pattern: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, Text, MB>,
    {
        let _ = PhantomData::<P>;
        crate::expr::regex_match_ci(self.as_expr::<RowSet>(), pattern)
    }

    /// A negated `REGEXP` match against `pattern`.
    pub fn not_regex_match<RowSet, P, B, MB>(self, pattern: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, Text, MB>,
    {
        let _ = PhantomData::<P>;
        crate::expr::not_regex_match(self.as_expr::<RowSet>(), pattern)
    }

    /// A negated, case-insensitive `REGEXP` match against `pattern`.
    pub fn not_regex_match_ci<RowSet, P, B, MB>(self, pattern: B) -> Expr<RowSet, Bool>
    where
        RowSet: Contains<P, T>,
        B: IntoExpr<RowSet, Text, MB>,
    {
        let _ = PhantomData::<P>;
        crate::expr::not_regex_match_ci(self.as_expr::<RowSet>(), pattern)
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
            name: "coalesce",
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
