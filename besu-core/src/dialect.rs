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

    /// How the dialect spells access to a value inside a JSON document.
    const JSON_PATH: JsonPathStyle = JsonPathStyle::Sqlite;

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

    /// The dialect's name for a canonical type, used by `CAST` and as the basis of DDL.
    fn type_name(ty: TypeDef) -> Cow<'static, str>;
}
