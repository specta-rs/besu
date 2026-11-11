use std::borrow::Cow;

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
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

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
            // `NUMERIC(20,0)` is the smallest native type that holds every `u64`.
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
            TypeDef::Timestamp => Cow::Borrowed("TIMESTAMP"),
            TypeDef::TimestampTz => Cow::Borrowed("TIMESTAMPTZ"),
            TypeDef::Decimal(p, s) => Cow::Owned(format!("NUMERIC({p},{s})")),
            TypeDef::Custom(name) => name,
            _ => Cow::Borrowed("TEXT"),
        }
    }
}
