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
