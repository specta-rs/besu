use alloc::{borrow::Cow, string::String};

use crate::{Error, TypeDef, Value};

/// How a dialect spells positional placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    /// A bare `?` for every parameter, as in SQLite and MySQL.
    Question,
    /// `?NNN` with an explicit index, as in SQLite.
    QuestionNumbered,
    /// `$NNN` with a 1-based index, as in PostgreSQL.
    Dollar,
}

/// How a dialect spells access to a value inside a JSON document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonPathStyle {
    /// `doc #> '{a,b}'` for JSON and `doc #>> '{a,b}'` for text, as in PostgreSQL.
    Postgres,
    /// `json_extract(doc, '$.a.b')`, which returns text for a scalar, as in SQLite.
    Sqlite,
    /// `JSON_EXTRACT(doc, '$.a.b')`, wrapped in `JSON_UNQUOTE` for text, as in MySQL.
    Mysql,
}

/// How a dialect spells a full-text match.
///
/// There is no portable syntax for this at all, so the styles differ in more than wording:
/// PostgreSQL matches a `tsquery` against a `tsvector`, MySQL needs a `FULLTEXT` index over
/// exactly the matched columns, and SQLite matches an FTS5 virtual table. The expression
/// layer hides the difference, but creating whatever the database needs is the caller's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullTextStyle {
    /// `to_tsvector('english', col) @@ plainto_tsquery('english', $1)`, as in PostgreSQL.
    Postgres,
    /// `MATCH(col) AGAINST ($1 IN BOOLEAN MODE)`, as in MySQL.
    Mysql,
    /// `col MATCH $1`, as in SQLite against an FTS5 virtual table.
    Sqlite,
    /// A dialect that has not said how it spells a match.
    ///
    /// Using a match on such a dialect records an `unsupported` error rather than emitting
    /// another database's syntax, which would fail in a way that points at the wrong place.
    Unsupported,
}

/// How a dialect spells a `NULL`-safe comparison.
///
/// `IS DISTINCT FROM` is `true` when the two sides differ and `false` when they are equal,
/// even when one side is `NULL` — unlike `<>`, which is `UNKNOWN` for any `NULL` and
/// therefore neither matches nor rejects the row. It is the comparison to reach for when a
/// nullable column has to be compared exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistinctFromStyle {
    /// `a IS DISTINCT FROM b` and `a IS NOT DISTINCT FROM b`, the SQL standard spelling,
    /// available on PostgreSQL and on SQLite 3.39 and later.
    Standard,
    /// MySQL has no `IS DISTINCT FROM` keyword, but its null-safe equality operator `<=>` is
    /// exactly `IS NOT DISTINCT FROM`, so the negated form is written around it.
    MySql,
    /// A dialect that has not said how it spells the comparison.
    ///
    /// Rendering records an `unsupported` error rather than emitting another database's
    /// syntax, which would fail inside the server with a message that points at the wrong
    /// place.
    Unsupported,
}

/// How a dialect spells a regular-expression match.
///
/// The three databases do not agree even on the operator, so this is a style rather than a
/// flag: PostgreSQL's `~` family takes modifiers in the operator itself, MySQL has a keyword
/// and a three-argument function, and SQLite has nothing unless an extension is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexStyle {
    /// `a ~ b`, `a ~* b`, `a !~ b` and `a !~* b`, as in PostgreSQL.
    Postgres,
    /// `a REGEXP b` for a match and `REGEXP_LIKE(a, b, 'i')` for a case-insensitive one, as
    /// in MySQL. There is no operator for the negated forms, so they are wrapped in `NOT`.
    MySql,
    /// A dialect with no regular-expression matching at all, as in SQLite without an
    /// extension. Using one records an `unsupported` error.
    Unsupported,
}

/// How a dialect spells a canonical function name.
///
/// Function spellings are not portable in the way operators are: the same operation can be
/// called `char_length` here and `LENGTH` there, can need an extra literal argument, and can
/// be spelled as a bare keyword. The variants cover exactly those differences, so a helper
/// names one canonical function and the dialect decides what reaches the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionStyle {
    /// `NAME(args...)`, with the arguments in the order the caller wrote them.
    Named(&'static str),
    /// `NAME` with no parentheses, for keywords such as `CURRENT_DATE` that share a
    /// function's meaning but not its syntax.
    Bare(&'static str),
    /// `NAME(args..., extra...)`, where the extras are raw SQL literals the caller has no
    /// business supplying: MySQL's `SHA2(x, 256)` and PostgreSQL's `encode(x, 'base64')`.
    WithSuffix(&'static str, &'static [&'static str]),
    /// `NAME(a SEPARATOR b)`, which is the only way MySQL spells the two-argument
    /// `string_agg`.
    Separator(&'static str),
}

impl FunctionStyle {
    /// The name written before the parentheses.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Named(name) | Self::Bare(name) | Self::Separator(name) => name,
            Self::WithSuffix(name, _) => name,
        }
    }
}

/// The functions every dialect spells the same way, as the default for
/// [`Dialect::function`].
///
/// Only a spelling that is identical on PostgreSQL, MySQL and SQLite belongs here. A
/// function that one of them lacks, renames or orders differently is deliberately absent, so
/// that a dialect has to say what it does with it and a missing capability becomes an
/// `unsupported` error instead of a server-side syntax error.
pub fn portable_function(name: &str) -> Option<FunctionStyle> {
    let spelling = match name {
        "count" => "COUNT",
        "sum" => "SUM",
        "avg" => "AVG",
        "min" => "MIN",
        "max" => "MAX",
        "lower" => "LOWER",
        "upper" => "UPPER",
        "length" => "LENGTH",
        "trim" => "TRIM",
        "abs" => "ABS",
        "round" => "ROUND",
        "coalesce" => "COALESCE",
        "nullif" => "NULLIF",
        "row_number" => "ROW_NUMBER",
        "rank" => "RANK",
        "dense_rank" => "DENSE_RANK",
        "lag" => "LAG",
        "lead" => "LEAD",
        _ => return None,
    };
    Some(FunctionStyle::Named(spelling))
}

/// How a dialect spells an upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertStyle {
    /// `INSERT ... ON CONFLICT (...) DO UPDATE SET ...`, as in PostgreSQL and SQLite.
    OnConflict,
    /// `INSERT ... ON DUPLICATE KEY UPDATE ...`, as in MySQL.
    OnDuplicateKey,
}

impl UpsertStyle {
    /// Whether this is MySQL's upsert spelling.
    pub const fn is_mysql(&self) -> bool {
        matches!(self, Self::OnDuplicateKey)
    }
}

/// A dialect implements the SQL syntax for a specific kind of database.
///
/// A dialect is a *type*, not a value, and always has exactly one instance of the empty
/// set: it is a compile-time tag. Crucially a dialect can render a complete query with no
/// database connection, which is what allows Besu to produce SQL in tests, in build
/// scripts and in the CLI.
///
/// Every method on this trait is meant to be total: dialects never fail because they cannot
/// express something. When they genuinely cannot (MySQL has no `RETURNING`) the capability
/// consts let the query builder reject it with a good error before a query is sent.
pub trait Dialect: 'static + Sized {
    /// The dialect's owned representation of a bound parameter.
    ///
    /// Every driver shipped with Besu uses [`Value`] directly, so encoding is a no-op.
    /// Another driver may use its own type and pay a conversion here instead.
    type Value: Clone + PartialEq + Send + Sync + core::fmt::Debug + 'static;

    /// The dialect's human name, for diagnostics.
    const NAME: &'static str;

    /// How the dialect spells parameters.
    const PLACEHOLDER: Placeholder;

    /// Whether `INSERT`/`UPDATE`/`DELETE` support a `RETURNING` clause.
    const SUPPORTS_RETURNING: bool;

    /// Whether `ILIKE` is available (PostgreSQL only).
    const SUPPORTS_ILIKE: bool;

    /// Whether the dialect has a native array type.
    ///
    /// Only PostgreSQL does. The expression layer uses this to decide whether an `IN` list
    /// can be one array parameter and whether an array operator is expressible at all; a
    /// dialect that leaves it off records an `unsupported` error rather than emitting
    /// PostgreSQL's syntax.
    const SUPPORTS_ARRAYS: bool = false;

    /// Whether `FULL OUTER JOIN` is available.
    const SUPPORTS_FULL_JOIN: bool;

    /// Whether `ALTER TABLE ... ALTER COLUMN ... TYPE` is supported.
    const SUPPORTS_ALTER_COLUMN_TYPE: bool = true;

    /// Whether `LIMIT` is mandatory when `OFFSET` is used. MySQL requires it because its
    /// grammar is `LIMIT <offset>, <count>`.
    const REQUIRES_LIMIT_WITH_OFFSET: bool = false;

    /// Whether `NULLS FIRST` / `NULLS LAST` can be written in `ORDER BY`. MySQL cannot.
    const SUPPORTS_NULLS_ORDER: bool = true;

    /// Whether `DEFAULT` may appear as a value in an `INSERT ... VALUES` list. SQLite
    /// cannot, which is why Besu omits never-provided columns instead.
    const SUPPORTS_DEFAULT_IN_VALUES: bool = true;

    /// The placeholder used when `LIMIT` is required but the user only set an `OFFSET`.
    const UNBOUNDED_LIMIT: i64 = -1;

    /// How the dialect spells an upsert.
    const UPSERT: UpsertStyle;

    /// Whether `ON CONFLICT DO UPDATE` may omit the conflict target.
    ///
    /// PostgreSQL requires the inference specification — `ON CONFLICT DO UPDATE` on its own
    /// is an error — while SQLite allows it on the last `ON CONFLICT` clause of the
    /// statement, where it applies to any unique constraint. The default is the stricter
    /// PostgreSQL rule, because a dialect that has not said otherwise is more likely to
    /// reject the statement than to accept it.
    const SUPPORTS_TARGETLESS_DO_UPDATE: bool = false;

    /// How the dialect spells access to a value inside a JSON document.
    const JSON_PATH: JsonPathStyle = JsonPathStyle::Sqlite;

    /// How the dialect spells a full-text match.
    ///
    /// Defaulted to [`FullTextStyle::Unsupported`] so that a dialect only emits a match once
    /// it has stated its spelling: each spelling is specific enough that emitting another
    /// database's would be silently wrong.
    const FULL_TEXT: FullTextStyle = FullTextStyle::Unsupported;

    /// Whether `DELETE ... LIMIT n` is available. PostgreSQL has no such statement.
    const SUPPORTS_DELETE_LIMIT: bool = true;

    /// The largest number of bound parameters a single statement may carry.
    ///
    /// Every one of these databases has a limit, and hitting it produces a confusing driver
    /// error, so Besu checks before sending the statement.
    const MAX_PARAMS: usize = 65_535;

    /// Whether `RIGHT JOIN` is available. SQLite only gained it in 3.39.
    const SUPPORTS_RIGHT_JOIN: bool = true;

    /// Whether `SELECT ... FOR UPDATE` is available. SQLite has no row locking.
    const SUPPORTS_ROW_LOCKING: bool = true;

    /// Whether `round(value, digits)` exists only for `numeric`, as on PostgreSQL.
    ///
    /// PostgreSQL has `round(numeric, int)` and no `round(double precision, int)`, so the
    /// portable two-argument call has to cast its value first. Rendering the cast here keeps
    /// the helper's promise that it works on any number.
    const ROUND_TO_NEEDS_NUMERIC: bool = false;

    /// Whether `INTERSECT ALL` is available.
    ///
    /// `INTERSECT` keeps a row once however many times both sides contain it; the `ALL`
    /// modifier keeps it as many times as the smaller of the two counts, which is the
    /// multiset intersection. SQLite has no such spelling, so it is off there.
    const SUPPORTS_INTERSECT_ALL: bool = true;

    /// Whether `EXCEPT ALL` is available.
    ///
    /// `EXCEPT` removes every copy of a row that the right-hand side contains; the `ALL`
    /// modifier subtracts the counts instead, which is the multiset difference. SQLite has
    /// no such spelling, so it is off there.
    const SUPPORTS_EXCEPT_ALL: bool = true;

    /// How the dialect spells `IS DISTINCT FROM`.
    ///
    /// Defaulted to [`DistinctFromStyle::Standard`] because the standard spelling is what
    /// most databases that implement the operation at all accept.
    const DISTINCT_FROM: DistinctFromStyle = DistinctFromStyle::Standard;

    /// How the dialect spells a regular-expression match.
    ///
    /// Defaulted to [`RegexStyle::Unsupported`], for the same reason as
    /// [`Dialect::FULL_TEXT`]: emitting one database's operator on another produces a
    /// syntax error a long way from the query that caused it.
    const REGEX: RegexStyle = RegexStyle::Unsupported;

    /// The dialect's spelling of a canonical function name.
    ///
    /// The helpers in the expression layer name a function once — `char_length`,
    /// `string_agg`, `json_agg` — and this hook turns that name into the text the server
    /// parses, or `None` when the dialect has no such function. `None` makes rendering
    /// record an [`Error::unsupported`] rather than write another database's syntax.
    ///
    /// The default is [`portable_function`], which knows only the handful of spellings that
    /// PostgreSQL, MySQL and SQLite genuinely share; a dialect overrides it to add the rest.
    fn function(name: &str) -> Option<FunctionStyle> {
        portable_function(name)
    }

    /// Convert a generic value into the dialect's bound representation.
    fn bind(value: Value) -> Result<Self::Value, Error>;

    /// Write a placeholder for the parameter at `index` (zero based) to `out`.
    fn write_placeholder(out: &mut String, index: usize);

    /// Quote an identifier, returning an owned string.
    fn quote(ident: &str) -> String {
        let mut out = String::new();
        Self::write_ident(&mut out, ident);
        out
    }

    /// Write an identifier, quoted so reserved words and odd characters work.
    fn write_ident(out: &mut String, ident: &str) {
        out.push('"');
        for ch in ident.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    }

    /// The dialect's name for a canonical type, as DDL spells it.
    ///
    /// This is the type a column is declared with, not necessarily the type a `CAST` may
    /// name: see [`Dialect::cast_type_name`].
    fn type_name(ty: TypeDef) -> Cow<'static, str>;

    /// The dialect's name for a canonical type inside `CAST`.
    ///
    /// DDL and `CAST` are two separate grammars, and a dialect may accept a type in one but
    /// not the other. MySQL is the clear case: its `CAST` only names `BINARY`, `CHAR`,
    /// `DATE`, `DATETIME`, `DECIMAL`, `DOUBLE`, `FLOAT`, `JSON`, `NCHAR`, `REAL`, `SIGNED`,
    /// `TIME`, `UNSIGNED`, `YEAR` and the spatial types, so the DDL spelling `INTEGER` or
    /// `VARCHAR(n)` there is a syntax error.
    ///
    /// The default is the DDL spelling, because PostgreSQL and SQLite accept the names they
    /// declare columns with in `CAST` too.
    fn cast_type_name(ty: TypeDef) -> Cow<'static, str> {
        Self::type_name(ty)
    }

    /// Append a comment to a finished statement that carries no meaning to the database.
    ///
    /// This exists for one reason: a driver caches prepared statements per SQL *string*, and
    /// some databases fix the parameter types at prepare time. Two statements with identical
    /// text but different bound types would then share a cache entry and the second would send
    /// bytes for the wrong type. A suffix that encodes the type shape makes the cache key
    /// match what is actually being bound.
    ///
    /// It writes into `out` rather than returning a `String` so that sealing a statement
    /// costs nothing when the dialect has nothing to add, and one growable buffer when it
    /// does.
    ///
    /// PostgreSQL needs this. Nothing else does, and the default writes nothing.
    fn write_statement_suffix(_out: &mut String, _args: &[Self::Value]) {}
}

/// Write `value` as decimal digits, without allocating.
///
/// Placeholders are written once per bound parameter, which makes this one of the hottest
/// few instructions in query building. `value.to_string()` would allocate a `String` for
/// every parameter purely to copy a handful of bytes into a buffer that already exists.
pub fn write_index(out: &mut String, mut value: usize) {
    let mut digits = [0_u8; 20];
    let mut len = digits.len();
    loop {
        len -= 1;
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    // `digits` holds ASCII decimal digits only, so this is always valid UTF-8. The fallback
    // cannot be reached, and writing nothing would be worse than a compile-time panic.
    for digit in &digits[len..] {
        out.push(char::from(*digit));
    }
}
