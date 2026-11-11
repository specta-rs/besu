use std::borrow::Cow;

use besu_core::{Dialect, Error, Placeholder, TypeDef, UpsertStyle, Value};

/// The SQLite dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SQLite;

/// SQLite has a single integer type, so anything that does not fit in an `i64` cannot be
/// stored losslessly.
pub(crate) fn sqlite_bind(value: Value) -> Result<Value, Error> {
    match &value {
        Value::UInt(v) if i64::try_from(*v).is_err() => Err(Error::encode(
            v,
            "SQLite integers are signed 64-bit, so this value does not fit",
        )),
        _ => Ok(value),
    }
}

impl Dialect for SQLite {
    type Value = Value;

    const NAME: &'static str = "sqlite";
    const PLACEHOLDER: Placeholder = Placeholder::Question;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = true;
    const REQUIRES_LIMIT_WITH_OFFSET: bool = false;
    const SUPPORTS_NULLS_ORDER: bool = true;
    const SUPPORTS_DEFAULT_IN_VALUES: bool = false;
    const UNBOUNDED_LIMIT: i64 = -1;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    fn bind(value: Value) -> Result<Self::Value, Error> {
        sqlite_bind(value)
    }

    fn write_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }

    fn type_name(ty: TypeDef) -> Cow<'static, str> {
        Cow::Borrowed(match ty {
            TypeDef::Bool => "BOOLEAN",
            TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 => "INTEGER",
            TypeDef::UInt(_) => "INTEGER",
            TypeDef::Float4 | TypeDef::Float8 => "REAL",
            TypeDef::Text | TypeDef::Char(_) | TypeDef::VarChar(_) => "TEXT",
            TypeDef::Bytes | TypeDef::Uuid => "BLOB",
            TypeDef::Json | TypeDef::Jsonb => "TEXT",
            TypeDef::Date | TypeDef::Time | TypeDef::Timestamp | TypeDef::TimestampTz => "TEXT",
            TypeDef::Decimal(_, _) => "NUMERIC",
            TypeDef::Custom(ref name) => return name.clone(),
            _ => "TEXT",
        })
    }
}
