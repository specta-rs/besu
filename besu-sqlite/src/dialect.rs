use std::borrow::Cow;

use besu_core::dialect::{
    DistinctFromStyle, FullTextStyle, FunctionStyle, RegexStyle, portable_function,
};
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
            "SQLite integers are signed 64-bit, so this value does not fit; use u128 for a \
             value this large",
        )),
        // A 128-bit value is stored as decimal text rather than a SQLite integer, so it is
        // not range-checked against the signed 64-bit limit.
        Value::UInt128(_) | Value::Int128(_) => Ok(value),
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

    /// SQLite allows `ON CONFLICT DO UPDATE` without a target: the clause applies to any
    /// unique constraint, which PostgreSQL does not permit.
    const SUPPORTS_TARGETLESS_DO_UPDATE: bool = true;

    /// SQLite's `MATCH` is provided by the table, not the expression: it only works against
    /// an FTS5 virtual table, which the schema layer cannot create.
    const FULL_TEXT: FullTextStyle = FullTextStyle::Sqlite;
    const SUPPORTS_ROW_LOCKING: bool = false;
    // SQLITE_MAX_VARIABLE_NUMBER, the default since 3.32.
    const MAX_PARAMS: usize = 32_766;

    /// SQLite has `INTERSECT` and `EXCEPT` but neither `ALL` form; both operators always
    /// remove duplicates.
    const SUPPORTS_INTERSECT_ALL: bool = false;

    /// SQLite has no `EXCEPT ALL`; see [`Self::SUPPORTS_INTERSECT_ALL`].
    const SUPPORTS_EXCEPT_ALL: bool = false;

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
            // SQLite has no integer wider than `i64`, so a 128-bit value is stored as its
            // decimal digits. `TEXT` affinity keeps the digits exact; comparisons and
            // ordering are lexicographic unless the column is cast to a number.
            TypeDef::UInt(128) => "TEXT",
            TypeDef::UInt(_) => "INTEGER",
            TypeDef::Float4 | TypeDef::Float8 => "REAL",
            TypeDef::Text | TypeDef::Char(_) | TypeDef::VarChar(_) => "TEXT",
            TypeDef::Bytes => "BLOB",
            // SQLite gives these NUMERIC affinity, so a value bound as text stays text while
            // the declared name round-trips through `PRAGMA table_info`.
            TypeDef::Uuid => "UUID",
            TypeDef::Json | TypeDef::Jsonb => "JSON",
            TypeDef::Date => "DATE",
            TypeDef::Time => "TIME",
            // SQLite stores a time as text and has no fractional precision in the type name,
            // so every precision collapses onto the same declared type.
            TypeDef::TimeP(_) => "TIME",
            // SQLite has no timezone-aware time, so the offset is dropped.
            TypeDef::TimeTz | TypeDef::TimeTzP(_) => "TIME",
            TypeDef::Timestamp | TypeDef::TimestampP(_) => "TIMESTAMP",
            TypeDef::TimestampTz | TypeDef::TimestampTzP(_) => "TIMESTAMP",
            // SQLite has no bit string; an integer is the closest storage, and `BLOB` keeps
            // no width. The value is the caller's to write with `sql!`.
            TypeDef::Bit(_) | TypeDef::VarBit(_) => "INTEGER",
            // An array is stored as its JSON text, which is the only collection SQLite has.
            TypeDef::Array(_) => "JSON",
            // SQLite has no enum: the label is stored as text, and normalisation folds the
            // declared enum onto the `TEXT` introspection reports.
            TypeDef::Enum(_) => "TEXT",
            TypeDef::Collated(inner, collation) => {
                return Cow::Owned(format!(
                    "{} COLLATE {}",
                    SQLite::type_name(*inner),
                    SQLite::quote(&collation)
                ));
            }
            // A decimal in SQLite is text: `NUMERIC` affinity would try to turn the stored
            // string into a float and lose the exact value, which is the whole point of a
            // decimal. `Decimal`'s Rust type is a string, so text is the honest mapping.
            TypeDef::Decimal(_, _) => "TEXT",
            TypeDef::Custom(ref name) => return name.clone(),
            _ => "TEXT",
        })
    }

    /// SQLite has had `IS DISTINCT FROM` since 3.39, and no regular-expression matching at
    /// all without an extension, so a regex records `unsupported`.
    const DISTINCT_FROM: DistinctFromStyle = DistinctFromStyle::Standard;
    const REGEX: RegexStyle = RegexStyle::Unsupported;

    /// SQLite's function library is smaller and differently named than the other two, so
    /// this table is mostly a translation: `char_length` is `length`, `btrim` is `trim`,
    /// `strpos` is `instr`, and `string_agg` is `group_concat`. Anything absent is a
    /// function SQLite does not have, which becomes an `unsupported` error rather than a
    /// server-side syntax error.
    fn function(name: &str) -> Option<FunctionStyle> {
        use FunctionStyle::{Bare, Named};
        Some(match name {
            "char_length" => Named("LENGTH"),
            "substr" => Named("SUBSTR"),
            "replace" => Named("REPLACE"),
            "ltrim" => Named("LTRIM"),
            "rtrim" => Named("RTRIM"),
            "btrim" => Named("TRIM"),
            "concat" => Named("CONCAT"),
            "concat_ws" => Named("CONCAT_WS"),
            "strpos" => Named("INSTR"),
            "greatest" => Named("MAX"),
            "least" => Named("MIN"),
            "current_date" => Bare("CURRENT_DATE"),
            "string_agg" => Named("GROUP_CONCAT"),
            "json_agg" => Named("JSON_GROUP_ARRAY"),
            // SQLite has one JSON type, so the binary variant is the same aggregate.
            "jsonb_agg" => Named("JSON_GROUP_ARRAY"),
            "json_object" => Named("JSON_OBJECT"),
            "json_array_agg" => Named("JSON_GROUP_ARRAY"),
            "json_object_agg" => Named("JSON_GROUP_OBJECT"),
            "json_merge_patch" => Named("JSON_PATCH"),
            "json_set" => Named("JSON_SET"),
            "json_remove" => Named("JSON_REMOVE"),
            "bool_and" => Named("MIN"),
            "bool_or" => Named("MAX"),
            "every" => Named("MIN"),
            "row_number" => Named("ROW_NUMBER"),
            "rank" => Named("RANK"),
            "dense_rank" => Named("DENSE_RANK"),
            "lag" => Named("LAG"),
            "lead" => Named("LEAD"),
            other => return portable_function(other),
        })
    }
}
