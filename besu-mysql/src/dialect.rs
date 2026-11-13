use std::borrow::Cow;

use besu_core::dialect::{
    DistinctFromStyle, FullTextStyle, FunctionStyle, RegexStyle, portable_function,
};
use besu_core::{Dialect, Error, JsonPathStyle, Placeholder, TypeDef, UpsertStyle, Value};

/// The MySQL dialect.
///
/// MySQL is close enough to standard SQL that most queries are identical to what you would
/// write by hand, with a handful of sharp edges: identifiers are quoted with backticks, an
/// `OFFSET` is only valid alongside a `LIMIT`, there is no `RETURNING`, and an upsert is
/// spelled `ON DUPLICATE KEY UPDATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MySql;

/// MySQL's spelling of a spatial column: `POINT`, optionally narrowed by `SRID n`.
///
/// MySQL has no `GEOGRAPHY`, and no `GEOMETRY(...)` typmod wrapper: the shape *is* the type
/// name, and the SRID is a column attribute.
fn spatial_type_name(kind: &besu_core::GeometryKind, srid: Option<u32>) -> Cow<'static, str> {
    use besu_core::GeometryKind;

    let base: Cow<'static, str> = match kind {
        GeometryKind::Geometry => Cow::Borrowed("GEOMETRY"),
        GeometryKind::Point => Cow::Borrowed("POINT"),
        GeometryKind::LineString => Cow::Borrowed("LINESTRING"),
        GeometryKind::Polygon => Cow::Borrowed("POLYGON"),
        GeometryKind::MultiPoint => Cow::Borrowed("MULTIPOINT"),
        GeometryKind::MultiLineString => Cow::Borrowed("MULTILINESTRING"),
        GeometryKind::MultiPolygon => Cow::Borrowed("MULTIPOLYGON"),
        GeometryKind::GeometryCollection => Cow::Borrowed("GEOMETRYCOLLECTION"),
        GeometryKind::Other(name) => Cow::Owned(name.to_uppercase()),
        // `GeometryKind` is `#[non_exhaustive]`: a shape added later still renders.
        _ => Cow::Owned(kind.name().to_uppercase()),
    };
    match srid {
        Some(srid) if srid != 0 => Cow::Owned(format!("{base} SRID {srid}")),
        _ => base,
    }
}

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
            // An enum is inline: MySQL has no named type, so only the labels are rendered.
            // A quote inside a label is escaped by doubling it, exactly as the server spells
            // it back.
            TypeDef::Enum(def) => {
                let mut out = String::from("ENUM(");
                for (i, label) in def.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    out.push('\'');
                    out.push_str(&label.replace('\'', "''"));
                    out.push('\'');
                }
                out.push(')');
                Cow::Owned(out)
            }
            TypeDef::Custom(name) => name,
            // A `SET` is inline, exactly like an inline enum: the labels are the type.
            TypeDef::Set(labels) => {
                let mut out = String::from("SET(");
                for (i, label) in labels.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    out.push('\'');
                    out.push_str(&label.replace('\'', "''"));
                    out.push('\'');
                }
                out.push(')');
                Cow::Owned(out)
            }
            // MySQL has no domain. A schema pulled from PostgreSQL still has to render, so the
            // base type is the closest safe spelling; `normalise_type` folds it back.
            TypeDef::Domain(def) => MySql::type_name(def.base.as_type().clone()),
            // MySQL has no range type. The canonical `[a,b)` text is exact and survives a
            // round trip, and `normalise_type` folds a declared range onto the `TEXT` column
            // introspection reports.
            TypeDef::Range(_) => Cow::Borrowed("TEXT"),
            // MySQL has real spatial types, spelled like PostGIS's shapes without the
            // `GEOMETRY(...)` wrapper. `GEOGRAPHY` has no MySQL counterpart, so it renders as
            // the equivalent `GEOMETRY` and the fold says so.
            TypeDef::Geometry { kind, srid } | TypeDef::Geography { kind, srid } => {
                spatial_type_name(&kind, srid)
            }
            // MySQL 8.4 has no vector type, so the `[1,2,3]` text is the storage.
            TypeDef::Vector(_) => Cow::Borrowed("TEXT"),
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
                // `TIME(6)` keeps the nanoseconds a `Value::Time` carries; a bare `TIME`
                // would silently truncate them to whole seconds. `TimeP` was added for a
                // column that really has another precision.
                TypeDef::Time => "TIME(6)",
                TypeDef::TimeP(p) => return Cow::Owned(format!("TIME({p})")),
                // MySQL has no timezone-aware time; the offset is dropped.
                TypeDef::TimeTz => "TIME(6)",
                TypeDef::TimeTzP(p) => return Cow::Owned(format!("TIME({p})")),
                // `DATETIME` rather than `TIMESTAMP`: MySQL's `TIMESTAMP` is range-limited
                // to 2038 and silently converts through the session time zone.
                TypeDef::Timestamp | TypeDef::TimestampTz => "DATETIME(6)",
                TypeDef::TimestampP(p) | TypeDef::TimestampTzP(p) => {
                    return Cow::Owned(format!("DATETIME({p})"));
                }
                // MySQL's bit field tops out at 64 bits; a wider one keeps the width in the
                // name so the server's own error says which column is at fault. A `VarBit`
                // with no width is a fixed 64-bit column, because MySQL has no variable one.
                TypeDef::Bit(n) => return Cow::Owned(format!("BIT({n})")),
                TypeDef::VarBit(0) => return Cow::Owned(String::from("BIT(64)")),
                TypeDef::VarBit(n) => return Cow::Owned(format!("BIT({n})")),
                // MySQL has no interval and no address type; the canonical text is exact and
                // survives a round trip, so `TEXT` is the honest storage.
                TypeDef::Interval | TypeDef::Inet | TypeDef::Cidr => "TEXT",
                TypeDef::Collated(inner, collation) => {
                    return Cow::Owned(format!(
                        "{} COLLATE {}",
                        MySql::type_name(*inner),
                        MySql::quote(&collation)
                    ));
                }
                // Widened unsigned types, and anything Besu does not model yet, fall back
                // to a type every MySQL understands.
                _ => "TEXT",
            }),
        }
    }

    /// MySQL's `CAST` grammar is a short, closed list, and most of the DDL spellings above
    /// are not on it: `INT`, `BIGINT`, `VARCHAR(n)`, `TEXT`, `BLOB` and `BOOLEAN` are all
    /// syntax errors after `AS`. The target here is chosen from that list, so a cast that
    /// maps several canonically distinct types onto one MySQL storage type (a `UUID` and a
    /// blob are both `BINARY`, for instance) still renders.
    ///
    /// The spelling is about syntax, not conversion: `CAST(text AS BINARY(16))` truncates
    /// the text's bytes rather than turning a UUID string into its 16-byte form. That
    /// conversion is `UUID_TO_BIN`, a function rather than a cast, so write it with
    /// [`raw`](https://docs.rs/besu/latest/besu/fn.raw.html) instead.
    fn cast_type_name(ty: TypeDef) -> Cow<'static, str> {
        match ty {
            TypeDef::Char(n) | TypeDef::VarChar(n) => Cow::Owned(format!("CHAR({n})")),
            TypeDef::Decimal(precision, scale) => {
                Cow::Owned(format!("DECIMAL({precision},{scale})"))
            }
            // A user-declared type is the caller's to know: MySQL accepts an enum or a
            // spatial type in `CAST`, and Besu cannot tell which it is, so the name is
            // passed through exactly as the DDL would write it.
            TypeDef::Custom(name) => name,
            // A domain is not spelled in MySQL; the base type decides what a cast means.
            TypeDef::Domain(def) => MySql::cast_type_name(def.base.as_type().clone()),
            other => Cow::Borrowed(match other {
                // `SIGNED` is MySQL's integer cast; a boolean is the integer `0` or `1`
                // there, so this is the same value, not a different one.
                TypeDef::Bool => "SIGNED",
                TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 => "SIGNED",
                TypeDef::UInt(128) => "DECIMAL(39,0)",
                TypeDef::UInt(_) => "UNSIGNED",
                // `DOUBLE` widens a `FLOAT` value without changing it, and `FLOAT` is
                // single precision: the wider target never loses digits the column held.
                TypeDef::Float4 | TypeDef::Float8 => "DOUBLE",
                // `CHAR` with no length keeps the whole string, which is what `TEXT` means.
                TypeDef::Text => "CHAR",
                // `BINARY` with no length keeps every byte, and `BINARY(16)` matches what a
                // UUID column is stored as.
                TypeDef::Bytes => "BINARY",
                TypeDef::Uuid => "BINARY(16)",
                TypeDef::Json | TypeDef::Jsonb => "JSON",
                TypeDef::Date => "DATE",
                TypeDef::Time | TypeDef::TimeTz => "TIME",
                TypeDef::Timestamp | TypeDef::TimestampTz => "DATETIME(6)",
                _ => "CHAR",
            }),
        }
    }

    /// MySQL has no `IS DISTINCT FROM` keyword; `<=>` is the null-safe equality the negated
    /// form is built on. Its regular expressions are `REGEXP` and `REGEXP_LIKE`.
    const DISTINCT_FROM: DistinctFromStyle = DistinctFromStyle::MySql;
    const REGEX: RegexStyle = RegexStyle::MySql;

    fn function(name: &str) -> Option<FunctionStyle> {
        use FunctionStyle::{Bare, Named, Separator, WithSuffix};
        Some(match name {
            // `CHAR_LENGTH` counts characters and `LENGTH` counts bytes, which is the
            // opposite of what the names suggest on `TEXT`.
            "char_length" => Named("CHAR_LENGTH"),
            "octet_length" => Named("LENGTH"),
            "substr" => Named("SUBSTR"),
            "replace" => Named("REPLACE"),
            "left" => Named("LEFT"),
            "right" => Named("RIGHT"),
            "lpad" => Named("LPAD"),
            "rpad" => Named("RPAD"),
            "ltrim" => Named("LTRIM"),
            "rtrim" => Named("RTRIM"),
            "btrim" => Named("TRIM"),
            "reverse" => Named("REVERSE"),
            "repeat" => Named("REPEAT"),
            // `INSTR` takes the haystack first, exactly like PostgreSQL's `strpos`.
            "strpos" => Named("INSTR"),
            "concat" => Named("CONCAT"),
            "concat_ws" => Named("CONCAT_WS"),
            "md5" => Named("MD5"),
            // MySQL's `SHA2` always names the digest size.
            "sha256" => WithSuffix("SHA2", &["256"]),
            "to_base64" => Named("TO_BASE64"),
            "from_base64" => Named("FROM_BASE64"),
            "regexp_replace" => Named("REGEXP_REPLACE"),
            "regexp_substr" => Named("REGEXP_SUBSTR"),
            "date_format" => Named("DATE_FORMAT"),
            "greatest" => Named("GREATEST"),
            "least" => Named("LEAST"),
            // MySQL's `TRUNCATE` always takes the number of decimal places.
            "ceil" => Named("CEIL"),
            "floor" => Named("FLOOR"),
            "trunc" => WithSuffix("TRUNCATE", &["0"]),
            "sign" => Named("SIGN"),
            "sqrt" => Named("SQRT"),
            "power" => Named("POWER"),
            "exp" => Named("EXP"),
            "ln" => Named("LN"),
            "log10" => Named("LOG10"),
            "current_date" => Bare("CURRENT_DATE"),
            "unix_timestamp" => Named("UNIX_TIMESTAMP"),
            "from_unixtime" => Named("FROM_UNIXTIME"),
            "string_agg" => Separator("GROUP_CONCAT"),
            "json_agg" => Named("JSON_ARRAYAGG"),
            // MySQL has one JSON type, so the binary variant is the same aggregate.
            "jsonb_agg" => Named("JSON_ARRAYAGG"),
            "json_object" => Named("JSON_OBJECT"),
            "json_array_agg" => Named("JSON_ARRAYAGG"),
            "json_object_agg" => Named("JSON_OBJECTAGG"),
            "json_merge_patch" => Named("JSON_MERGE_PATCH"),
            // The path-taking functions, whose path argument is rendered in MySQL's grammar
            // by the expression layer rather than being an ordinary argument here.
            "json_set" => Named("JSON_SET"),
            "json_remove" => Named("JSON_REMOVE"),
            "json_value" => Named("JSON_VALUE"),
            "json_query" => Named("JSON_QUERY"),
            // `MIN` and `MAX` over a boolean column return the column's own `TINYINT`, so
            // the value decodes as a boolean. The bitwise aggregates would return a
            // `BIGINT`, which does not.
            "bool_and" => Named("MIN"),
            "bool_or" => Named("MAX"),
            "every" => Named("MIN"),
            "stddev" => Named("STDDEV_SAMP"),
            "stddev_pop" => Named("STDDEV_POP"),
            "variance" => Named("VAR_SAMP"),
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

    #[test]
    fn an_enum_is_inline_with_its_labels() {
        // MySQL has no named type, so only the labels reach the DDL. A quote inside a label
        // is doubled, which is how the server spells it back.
        let def = besu_core::EnumDef::new("mood", &["happy", "it's ok"]);
        assert_eq!(
            MySql::type_name(TypeDef::Enum(def)),
            "ENUM('happy','it''s ok')"
        );
    }

    #[test]
    fn a_set_is_inline_with_its_labels() {
        let labels = besu_core::EnumLabels::from_static(&["read", "write"]);
        assert_eq!(
            MySql::type_name(TypeDef::Set(labels)),
            "SET('read','write')"
        );
    }

    #[test]
    fn bit_interval_and_address_types_take_their_closest_storage() {
        assert_eq!(MySql::type_name(TypeDef::Bit(8)), "BIT(8)");
        // MySQL has one fixed-width bit type, so an unbounded `VARBIT` is a full 64 bits.
        assert_eq!(MySql::type_name(TypeDef::VarBit(0)), "BIT(64)");
        assert_eq!(MySql::type_name(TypeDef::VarBit(5)), "BIT(5)");
        assert_eq!(MySql::type_name(TypeDef::Interval), "TEXT");
        assert_eq!(MySql::type_name(TypeDef::Inet), "TEXT");
        assert_eq!(MySql::type_name(TypeDef::Cidr), "TEXT");
    }

    #[test]
    fn a_domain_renders_its_base_type() {
        let domain = TypeDef::Domain(besu_core::DomainDef::new("positive", &TypeDef::Int4));
        assert_eq!(MySql::type_name(domain), "INT");
    }

    /// The DDL spelling is mostly not legal after `CAST(... AS ...)`, which accepts only
    /// `BINARY`, `CHAR`, `DATE`, `DATETIME`, `DECIMAL`, `DOUBLE`, `FLOAT`, `JSON`, `NCHAR`,
    /// `REAL`, `SIGNED`, `TIME`, `UNSIGNED`, `YEAR` and the spatial types.
    #[test]
    fn casts_are_spelled_from_the_cast_grammar() {
        assert_eq!(MySql::cast_type_name(TypeDef::Bool), "SIGNED");
        assert_eq!(MySql::cast_type_name(TypeDef::Int4), "SIGNED");
        assert_eq!(MySql::cast_type_name(TypeDef::UInt(32)), "UNSIGNED");
        assert_eq!(MySql::cast_type_name(TypeDef::UInt(128)), "DECIMAL(39,0)");
        assert_eq!(MySql::cast_type_name(TypeDef::Float8), "DOUBLE");
        assert_eq!(MySql::cast_type_name(TypeDef::Text), "CHAR");
        assert_eq!(MySql::cast_type_name(TypeDef::VarChar(255)), "CHAR(255)");
        assert_eq!(MySql::cast_type_name(TypeDef::Bytes), "BINARY");
        assert_eq!(MySql::cast_type_name(TypeDef::Uuid), "BINARY(16)");
        assert_eq!(MySql::cast_type_name(TypeDef::Json), "JSON");
        assert_eq!(MySql::cast_type_name(TypeDef::Time), "TIME");
        assert_eq!(MySql::cast_type_name(TypeDef::TimestampTz), "DATETIME(6)");
        assert_eq!(
            MySql::cast_type_name(TypeDef::Decimal(10, 2)),
            "DECIMAL(10,2)"
        );
    }
}
