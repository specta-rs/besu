use std::borrow::Cow;

use besu_core::dialect::{
    DistinctFromStyle, FullTextStyle, FunctionStyle, RegexStyle, portable_function,
};
use besu_core::{Dialect, Error, JsonPathStyle, Placeholder, TypeDef, UpsertStyle, Value};

/// The PostgreSQL dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Postgres;

/// Quote each dotted part of a type name, when every part is a plain identifier.
///
/// A `Custom` type is resolved through `search_path` when it is unquoted, so a name such as
/// `mySchema.Mood` would fold to lowercase and reach a type that does not exist. Quoting
/// each part preserves the case, which is the whole point of storing the name exactly.
///
/// A name that is not a chain of identifiers — an inline `enum('a','b')` from another
/// dialect, or a type with arguments — is returned verbatim, because quoting it would turn
/// something the server accepts into a syntax error.
fn quote_type_name(name: &str) -> Cow<'static, str> {
    let plain = |part: &str| {
        part.chars()
            .next()
            .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
            && part
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
    };
    if name.is_empty() || !name.split('.').all(plain) {
        return Cow::Owned(name.to_owned());
    }
    Cow::Owned(
        name.split('.')
            .map(Postgres::quote)
            .collect::<Vec<_>>()
            .join("."),
    )
}

impl Dialect for Postgres {
    type Value = Value;

    const NAME: &'static str = "postgres";
    const PLACEHOLDER: Placeholder = Placeholder::Dollar;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = true;
    const SUPPORTS_ARRAYS: bool = true;
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
    const ROUND_TO_NEEDS_NUMERIC: bool = true;

    /// Encode the bound values' shapes so the prepared-statement cache cannot confuse two
    /// statements that share text but not parameter types.
    ///
    /// PostgreSQL fixes parameter types at prepare time, and sqlx caches by SQL string alone.
    /// Without this, `sql!("SELECT ?::numeric", x)` executed once with an integer and once
    /// with a numeric would reach the same cached statement with the wrong bytes.
    fn write_statement_suffix(suffix: &mut String, args: &[Self::Value]) {
        if args.is_empty() {
            return;
        }

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
                Value::Decimal(_) => 'D',
                Value::Array { .. } => 'A',
                _ => '?',
            });
        }
        suffix.push_str(" */");
    }

    fn bind(value: Value) -> Result<Self::Value, Error> {
        // PostgreSQL has a native type for every value Besu can represent, and `NUMERIC`
        // covers integers that do not fit in an `i64`, so nothing is lossy.
        Ok(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        out.push('$');
        besu_core::dialect::write_index(out, index + 1);
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
            TypeDef::TimeP(p) => Cow::Owned(format!("TIME({p})")),
            TypeDef::TimeTz => Cow::Borrowed("TIMETZ"),
            TypeDef::TimeTzP(p) => Cow::Owned(format!("TIMETZ({p})")),
            TypeDef::Timestamp => Cow::Borrowed("TIMESTAMP"),
            TypeDef::TimestampP(p) => Cow::Owned(format!("TIMESTAMP({p})")),
            TypeDef::TimestampTz => Cow::Borrowed("TIMESTAMPTZ"),
            TypeDef::TimestampTzP(p) => Cow::Owned(format!("TIMESTAMPTZ({p})")),
            TypeDef::Decimal(p, s) => Cow::Owned(format!("NUMERIC({p},{s})")),
            TypeDef::Bit(n) => Cow::Owned(format!("BIT({n})")),
            // A bare `BIT VARYING` has no limit, which is what `VarBit(0)` records.
            TypeDef::VarBit(0) => Cow::Borrowed("BIT VARYING"),
            TypeDef::VarBit(n) => Cow::Owned(format!("BIT VARYING({n})")),
            // An array is spelled by appending `[]` to the element type, which is exactly
            // how `format_type` reports it back.
            TypeDef::Array(inner) => Cow::Owned(format!(
                "{}[]",
                Postgres::type_name(inner.as_type().clone())
            )),
            // A collation is written after the type it applies to, which is legal both in
            // `CREATE TABLE` and after `ALTER COLUMN ... TYPE`.
            TypeDef::Collated(inner, collation) => Cow::Owned(format!(
                "{} COLLATE {}",
                Postgres::type_name(*inner),
                Postgres::quote(&collation)
            )),
            TypeDef::Custom(name) => quote_type_name(&name),
            _ => Cow::Borrowed("TEXT"),
        }
    }

    /// PostgreSQL is the one dialect that spells both null-safe comparisons as keywords, and
    /// the one that has the whole `~` family of regular-expression operators.
    const DISTINCT_FROM: DistinctFromStyle = DistinctFromStyle::Standard;
    const REGEX: RegexStyle = RegexStyle::Postgres;

    fn function(name: &str) -> Option<FunctionStyle> {
        use FunctionStyle::{Bare, Named};
        Some(match name {
            "char_length" => Named("CHAR_LENGTH"),
            "octet_length" => Named("OCTET_LENGTH"),
            "substr" => Named("SUBSTR"),
            "replace" => Named("REPLACE"),
            "left" => Named("LEFT"),
            "right" => Named("RIGHT"),
            "lpad" => Named("LPAD"),
            "rpad" => Named("RPAD"),
            "ltrim" => Named("LTRIM"),
            "rtrim" => Named("RTRIM"),
            "btrim" => Named("BTRIM"),
            "initcap" => Named("INITCAP"),
            "reverse" => Named("REVERSE"),
            "repeat" => Named("REPEAT"),
            "strpos" => Named("STRPOS"),
            "split_part" => Named("SPLIT_PART"),
            "translate" => Named("TRANSLATE"),
            "concat" => Named("CONCAT"),
            "concat_ws" => Named("CONCAT_WS"),
            "md5" => Named("MD5"),
            "regexp_replace" => Named("REGEXP_REPLACE"),
            "greatest" => Named("GREATEST"),
            "least" => Named("LEAST"),
            "ceil" => Named("CEIL"),
            "floor" => Named("FLOOR"),
            "trunc" => Named("TRUNC"),
            "sign" => Named("SIGN"),
            "sqrt" => Named("SQRT"),
            "power" => Named("POWER"),
            "exp" => Named("EXP"),
            "ln" => Named("LN"),
            "log10" => Named("LOG10"),
            "current_date" => Bare("CURRENT_DATE"),
            "date_trunc" => Named("DATE_TRUNC"),
            "string_agg" => Named("STRING_AGG"),
            "json_agg" => Named("JSON_AGG"),
            "jsonb_agg" => Named("JSONB_AGG"),
            "json_object" => Named("JSONB_BUILD_OBJECT"),
            "json_array_agg" => Named("JSON_AGG"),
            "json_object_agg" => Named("JSONB_OBJECT_AGG"),
            // Arrays. PostgreSQL is the only supported database with a real array type, so
            // these are exactly the spellings and every other dialect reports the function
            // as missing instead of inventing one.
            "array_length" => Named("ARRAY_LENGTH"),
            "cardinality" => Named("CARDINALITY"),
            "array_append" => Named("ARRAY_APPEND"),
            "array_prepend" => Named("ARRAY_PREPEND"),
            "array_cat" => Named("ARRAY_CAT"),
            "array_position" => Named("ARRAY_POSITION"),
            "array_remove" => Named("ARRAY_REMOVE"),
            "array_replace" => Named("ARRAY_REPLACE"),
            "array_to_string" => Named("ARRAY_TO_STRING"),
            "string_to_array" => Named("STRING_TO_ARRAY"),
            "array_agg" => Named("ARRAY_AGG"),
            "bool_and" => Named("BOOL_AND"),
            "bool_or" => Named("BOOL_OR"),
            "every" => Named("EVERY"),
            "stddev" => Named("STDDEV"),
            "stddev_pop" => Named("STDDEV_POP"),
            "variance" => Named("VARIANCE"),
            "var_pop" => Named("VAR_POP"),
            "row_number" => Named("ROW_NUMBER"),
            "rank" => Named("RANK"),
            "dense_rank" => Named("DENSE_RANK"),
            "lag" => Named("LAG"),
            "lead" => Named("LEAD"),
            other => return portable_function(other),
        })
    }
}
