use std::borrow::Cow;

use besu_core::dialect::FullTextStyle;
use besu_core::{Dialect, Error, JsonPathStyle, Placeholder, TypeDef, UpsertStyle, Value};

/// The MySQL dialect.
///
/// MySQL is close enough to standard SQL that most queries are identical to what you would
/// write by hand, with a handful of sharp edges: identifiers are quoted with backticks, an
/// `OFFSET` is only valid alongside a `LIMIT`, there is no `RETURNING`, and an upsert is
/// spelled `ON DUPLICATE KEY UPDATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MySql;

impl Dialect for MySql {
    type Value = Value;

    const NAME: &'static str = "mysql";
    const PLACEHOLDER: Placeholder = Placeholder::Question;

    /// MySQL has no `RETURNING` clause, so inserts and updates cannot hand back the rows
    /// they touched. Select them afterwards instead.
    const SUPPORTS_RETURNING: bool = false;

    /// MySQL's `LIKE` is case-insensitive under the default `utf8mb4_unicode_ci`
    /// collation, and there is no `ILIKE` keyword.
    const SUPPORTS_ILIKE: bool = false;

    /// MySQL has no `FULL OUTER JOIN`; emulate it by unioning a left and a right join.
    const SUPPORTS_FULL_JOIN: bool = false;

    /// `OFFSET` is only grammatical after a `LIMIT`.
    const REQUIRES_LIMIT_WITH_OFFSET: bool = true;

    /// MySQL has no `NULLS FIRST` / `NULLS LAST`; the query layer emulates it by ordering
    /// on `expr IS NULL` first.
    const SUPPORTS_NULLS_ORDER: bool = false;

    /// `DEFAULT` is legal in a `VALUES` list, which is what makes a bulk insert with holes
    /// work.
    const SUPPORTS_DEFAULT_IN_VALUES: bool = true;

    /// MySQL's "no limit" sentinel is `18446744073709551615` (`u64::MAX`), which does not
    /// fit in the `i64` this constant is declared as. `i64::MAX` is an equally valid
    /// `LIMIT` and is just as unbounded in practice.
    const UNBOUNDED_LIMIT: i64 = i64::MAX;

    const JSON_PATH: JsonPathStyle = JsonPathStyle::Mysql;

    /// MySQL has exactly one match spelling, and it needs a `FULLTEXT` index over the column
    /// it names; the schema is the caller's to declare.
    const FULL_TEXT: FullTextStyle = FullTextStyle::Mysql;
    const UPSERT: UpsertStyle = UpsertStyle::OnDuplicateKey;

    /// MySQL 8.4 supports the `ALL` modifiers on every set operator, and the tests run them
    /// against a real server. A server that has `INTERSECT` and `EXCEPT` but not their `ALL`
    /// forms rejects the statement with its own syntax error, rather than the query quietly
    /// losing duplicates.
    const SUPPORTS_INTERSECT_ALL: bool = true;

    /// See [`Self::SUPPORTS_INTERSECT_ALL`].
    const SUPPORTS_EXCEPT_ALL: bool = true;

    /// MySQL can hold every value Besu models, including unsigned 64-bit integers, so
    /// binding never fails.
    fn bind(value: Value) -> Result<Self::Value, Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }

    /// MySQL quotes identifiers with backticks, and escapes a backtick inside one by
    /// doubling it.
    fn write_ident(out: &mut String, ident: &str) {
        out.push('`');
        for ch in ident.chars() {
            if ch == '`' {
                out.push('`');
            }
            out.push(ch);
        }
        out.push('`');
    }

    fn type_name(ty: TypeDef) -> Cow<'static, str> {
        match ty {
            TypeDef::Char(n) => Cow::Owned(format!("CHAR({n})")),
            TypeDef::VarChar(n) => Cow::Owned(format!("VARCHAR({n})")),
            TypeDef::Decimal(precision, scale) => {
                Cow::Owned(format!("DECIMAL({precision},{scale})"))
            }
            TypeDef::Custom(name) => name,
            other => Cow::Borrowed(match other {
                TypeDef::Bool => "BOOLEAN",
                TypeDef::Int2 => "SMALLINT",
                TypeDef::Int4 => "INT",
                TypeDef::Int8 => "BIGINT",
                TypeDef::UInt(bits) => match bits {
                    8 => "TINYINT UNSIGNED",
                    16 => "SMALLINT UNSIGNED",
                    32 => "INT UNSIGNED",
                    // `BIGINT UNSIGNED` stops at `u64::MAX`; a 128-bit value needs an exact
                    // decimal, and `DECIMAL(39,0)` holds every `u128`.
                    128 => "DECIMAL(39,0)",
                    _ => "BIGINT UNSIGNED",
                },
                TypeDef::Float4 => "FLOAT",
                TypeDef::Float8 => "DOUBLE",
                TypeDef::Text => "TEXT",
                TypeDef::Bytes => "BLOB",
                TypeDef::Uuid => "BINARY(16)",
                TypeDef::Json | TypeDef::Jsonb => "JSON",
                TypeDef::Date => "DATE",
                TypeDef::Time => "TIME",
                // MySQL has no timezone-aware time; the offset is dropped.
                TypeDef::TimeTz => "TIME",
                // `DATETIME` rather than `TIMESTAMP`: MySQL's `TIMESTAMP` is range-limited
                // to 2038 and silently converts through the session time zone.
                TypeDef::Timestamp | TypeDef::TimestampTz => "DATETIME(6)",
                // Widened unsigned types, and anything Besu does not model yet, fall back
                // to a type every MySQL understands.
                _ => "TEXT",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_with_backticks() {
        assert_eq!(MySql::quote("users"), "`users`");
        // A backtick inside the name is escaped by doubling it.
        assert_eq!(MySql::quote("odd`name"), "`odd``name`");
    }

    #[test]
    fn types_are_spelled_the_mysql_way() {
        assert_eq!(MySql::type_name(TypeDef::Int8), "BIGINT");
        assert_eq!(MySql::type_name(TypeDef::UInt(8)), "TINYINT UNSIGNED");
        assert_eq!(MySql::type_name(TypeDef::VarChar(255)), "VARCHAR(255)");
        assert_eq!(MySql::type_name(TypeDef::Decimal(10, 2)), "DECIMAL(10,2)");
        assert_eq!(MySql::type_name(TypeDef::Uuid), "BINARY(16)");
        assert_eq!(MySql::type_name(TypeDef::TimestampTz), "DATETIME(6)");
    }
}
