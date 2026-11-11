use std::borrow::Cow;

use besu_core::dialect::FullTextStyle;
use besu_core::{Dialect, Error, JsonPathStyle, Placeholder, TypeDef, UpsertStyle, Value};

/// The PostgreSQL dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Postgres;

impl Dialect for Postgres {
    type Value = Value;

    const NAME: &'static str = "postgres";
    const PLACEHOLDER: Placeholder = Placeholder::Dollar;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = true;
    const SUPPORTS_FULL_JOIN: bool = true;
    const SUPPORTS_RIGHT_JOIN: bool = true;
    const SUPPORTS_ROW_LOCKING: bool = true;
    const SUPPORTS_DELETE_LIMIT: bool = false;
    const REQUIRES_LIMIT_WITH_OFFSET: bool = false;
    const SUPPORTS_NULLS_ORDER: bool = true;
    const SUPPORTS_DEFAULT_IN_VALUES: bool = true;
    const MAX_PARAMS: usize = 65_535;
    const JSON_PATH: JsonPathStyle = JsonPathStyle::Postgres;

    /// A match is a `tsquery` against a `tsvector`, and both are built for the same text
    /// search configuration so the document and the query are normalised identically.
    const FULL_TEXT: FullTextStyle = FullTextStyle::Postgres;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    /// Encode the bound values' shapes so the prepared-statement cache cannot confuse two
    /// statements that share text but not parameter types.
    ///
    /// PostgreSQL fixes parameter types at prepare time, and sqlx caches by SQL string alone.
    /// Without this, `sql!("SELECT ?::numeric", x)` executed once with an integer and once
    /// with a numeric would reach the same cached statement with the wrong bytes.
    fn statement_suffix(args: &[Self::Value]) -> Option<String> {
        if args.is_empty() {
            return None;
        }

        let mut suffix = String::with_capacity(args.len() + 14);
        // A newline first, so a trailing `--` comment cannot swallow the signature.
        suffix.push_str("\n/* besu:");
        for value in args {
            suffix.push(match value {
                Value::Null => 'n',
                Value::Bool(_) => 'b',
                Value::Int(_) => 'i',
                Value::UInt(_) => 'u',
                Value::UInt128(_) => 'U',
                Value::Int128(_) => 'I',
                Value::Float(_) => 'f',
                Value::Text(_) => 't',
                Value::Bytes(_) => 'x',
                Value::Uuid(_) => 'd',
                Value::Date(_) => 'e',
                Value::Time(_) => 'm',
                Value::Timestamp(_) => 's',
                Value::TimestampTz(_) => 'z',
                Value::Json(_) => 'j',
                _ => '?',
            });
        }
        suffix.push_str(" */");
        Some(suffix)
    }

    fn bind(value: Value) -> Result<Self::Value, Error> {
        // PostgreSQL has a native type for every value Besu can represent, and `NUMERIC`
        // covers integers that do not fit in an `i64`, so nothing is lossy.
        Ok(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&(index + 1).to_string());
    }

    fn type_name(ty: TypeDef) -> Cow<'static, str> {
        match ty {
            TypeDef::Bool => Cow::Borrowed("BOOLEAN"),
            TypeDef::Int2 => Cow::Borrowed("SMALLINT"),
            TypeDef::Int4 => Cow::Borrowed("INTEGER"),
            TypeDef::Int8 => Cow::Borrowed("BIGINT"),
            TypeDef::UInt(8) => Cow::Borrowed("SMALLINT"),
            TypeDef::UInt(16) => Cow::Borrowed("INTEGER"),
            TypeDef::UInt(32) => Cow::Borrowed("BIGINT"),
            // `NUMERIC(20,0)` is the smallest native type that holds every `u64`, and
            // `NUMERIC(39,0)` every `u128`.
            TypeDef::UInt(64) => Cow::Borrowed("NUMERIC(20,0)"),
            TypeDef::UInt(128) => Cow::Borrowed("NUMERIC(39,0)"),
            TypeDef::UInt(_) => Cow::Borrowed("NUMERIC(20,0)"),
            TypeDef::Float4 => Cow::Borrowed("REAL"),
            TypeDef::Float8 => Cow::Borrowed("DOUBLE PRECISION"),
            TypeDef::Text => Cow::Borrowed("TEXT"),
            TypeDef::Char(n) => Cow::Owned(format!("CHAR({n})")),
            TypeDef::VarChar(n) => Cow::Owned(format!("VARCHAR({n})")),
            TypeDef::Bytes => Cow::Borrowed("BYTEA"),
            TypeDef::Uuid => Cow::Borrowed("UUID"),
            TypeDef::Json => Cow::Borrowed("JSON"),
            TypeDef::Jsonb => Cow::Borrowed("JSONB"),
            TypeDef::Date => Cow::Borrowed("DATE"),
            TypeDef::Time => Cow::Borrowed("TIME"),
            TypeDef::TimeTz => Cow::Borrowed("TIMETZ"),
            TypeDef::Timestamp => Cow::Borrowed("TIMESTAMP"),
            TypeDef::TimestampTz => Cow::Borrowed("TIMESTAMPTZ"),
            TypeDef::Decimal(p, s) => Cow::Owned(format!("NUMERIC({p},{s})")),
            TypeDef::Custom(name) => name,
            _ => Cow::Borrowed("TEXT"),
        }
    }
}
