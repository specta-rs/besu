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

/// The PostgreSQL name of the built-in range type over an element type.
///
/// PostgreSQL has six built-in range types and no generic `RANGE` spelling, so an element
/// outside these six has no type to name. A `Decimal` range is `numrange` whatever precision
/// and scale the Rust marker carries, because the subtype is an unconstrained `numeric`.
pub(crate) fn range_type_name(element: &TypeDef) -> Option<&'static str> {
    Some(match element {
        TypeDef::Int4 => "int4range",
        TypeDef::Int8 => "int8range",
        TypeDef::Decimal(_, _) => "numrange",
        TypeDef::Timestamp | TypeDef::TimestampP(_) => "tsrange",
        TypeDef::TimestampTz | TypeDef::TimestampTzP(_) => "tstzrange",
        TypeDef::Date => "daterange",
        // The fold lets a declared `Range(UInt(32))` render through its signed storage.
        TypeDef::UInt(16) => "int4range",
        TypeDef::UInt(32) => "int8range",
        _ => return None,
    })
}

/// The PostGIS spelling of a spatial column's type.
///
/// A plain `GEOMETRY`/`GEOGRAPHY` has no shape and no SRID, so it is written bare. Anything
/// narrowed is written with its typmod, exactly as `format_type` reports it back.
fn spatial_type_name(
    geography: bool,
    kind: &besu_core::GeometryKind,
    srid: Option<u32>,
) -> Cow<'static, str> {
    use besu_core::GeometryKind;

    let base = if geography { "GEOGRAPHY" } else { "GEOMETRY" };
    if *kind == GeometryKind::Geometry && srid.is_none() {
        return Cow::Borrowed(base);
    }
    let shape: Cow<'static, str> = match kind {
        GeometryKind::Geometry => Cow::Borrowed("GEOMETRY"),
        GeometryKind::Point => Cow::Borrowed("POINT"),
        GeometryKind::LineString => Cow::Borrowed("LINESTRING"),
        GeometryKind::Polygon => Cow::Borrowed("POLYGON"),
        GeometryKind::MultiPoint => Cow::Borrowed("MULTIPOINT"),
        GeometryKind::MultiLineString => Cow::Borrowed("MULTILINESTRING"),
        GeometryKind::MultiPolygon => Cow::Borrowed("MULTIPOLYGON"),
        GeometryKind::GeometryCollection => Cow::Borrowed("GEOMETRYCOLLECTION"),
        // A shape from a newer PostGIS is written exactly as the catalogue spelled it.
        GeometryKind::Other(name) => Cow::Owned(name.to_string()),
        // `GeometryKind` is `#[non_exhaustive]`: a shape added later still renders.
        _ => Cow::Owned(kind.name().to_string()),
    };
    Cow::Owned(match srid {
        Some(srid) => format!("{base}({shape},{srid})"),
        None => format!("{base}({shape})"),
    })
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
                // An enum's label is bound untyped, so it is a distinct parameter shape in
                // the prepared-statement cache: the same SQL text could otherwise resolve to
                // different enum types in different statements.
                Value::EnumLabel(_) => 'e',
                Value::Bytes(_) => 'x',
                Value::Uuid(_) => 'd',
                // A date is its own SQL type (`DATE`), just as an enum label is its own
                // untyped shape. They must not share a character: a cached statement
                // prepared for one would then be reused for the other.
                Value::Date(_) => 'y',
                Value::Time(_) => 'm',
                Value::Timestamp(_) => 's',
                Value::TimestampTz(_) => 'z',
                Value::Json(_) => 'j',
                Value::Decimal(_) => 'D',
                Value::Interval(_) => 'v',
                Value::Inet(_) => 'a',
                Value::Cidr(_) => 'c',
                Value::Bit(_) => 'B',
                Value::Range { element, .. } => {
                    // The subtype is part of the bytes: `int4range` and `int8range` share
                    // this SQL text but not their wire format.
                    suffix.push('R');
                    suffix.push_str(&format!("{element:?}"));
                    continue;
                }
                Value::Geometry(_) => 'G',
                Value::Vector(_) => 'V',
                Value::Array { element, .. } => {
                    // The element OID is part of the bytes, so it has to be part of the key:
                    // `int8[]` and `text[]` bind differently into the same SQL text.
                    suffix.push('A');
                    suffix.push_str(&format!("{element:?}"));
                    continue;
                }
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
            TypeDef::Interval => Cow::Borrowed("INTERVAL"),
            TypeDef::Inet => Cow::Borrowed("INET"),
            TypeDef::Cidr => Cow::Borrowed("CIDR"),
            // MySQL's `SET` has no PostgreSQL spelling; a label list is text here, and the
            // type fold does the same so a schema shared with MySQL converges.
            TypeDef::Set(_) => Cow::Borrowed("TEXT"),
            // A domain is declared with its own name; `CREATE DOMAIN` is what makes the name
            // exist, and the differ creates it before the tables that use it.
            TypeDef::Domain(def) => quote_type_name(&def.name),
            // An array is spelled by appending `[]` to the element type, which is exactly
            // how `format_type` reports it back.
            TypeDef::Array(inner) => Cow::Owned(format!(
                "{}[]",
                Postgres::type_name(inner.as_type().clone())
            )),
            // A range is one of PostgreSQL's six built-in range types, named after its
            // subtype. An element with no built-in range type is stored as text, matching
            // what `normalise_type` folds it to.
            TypeDef::Range(inner) => {
                Cow::Borrowed(range_type_name(inner.as_type()).unwrap_or("TEXT"))
            }
            // PostGIS spells a narrowed spatial column as `GEOMETRY(POINT,4326)`.
            TypeDef::Geometry { kind, srid } => spatial_type_name(false, &kind, srid),
            TypeDef::Geography { kind, srid } => spatial_type_name(true, &kind, srid),
            // `VECTOR` with no modifier accepts any dimension; `pgvector` reports it back the
            // same way, so a dimension-less declaration converges.
            TypeDef::Vector(0) => Cow::Borrowed("VECTOR"),
            TypeDef::Vector(n) => Cow::Owned(format!("VECTOR({n})")),
            // A collation is written after the type it applies to, which is legal both in
            // `CREATE TABLE` and after `ALTER COLUMN ... TYPE`.
            TypeDef::Collated(inner, collation) => Cow::Owned(format!(
                "{} COLLATE {}",
                Postgres::type_name(*inner),
                Postgres::quote(&collation)
            )),
            // A named enum is the type's name, exactly as a domain is. An enum with no name
            // is a label set with nothing for PostgreSQL to create, so it is stored as text.
            TypeDef::Enum(def) if def.name.is_empty() => Cow::Borrowed("TEXT"),
            TypeDef::Enum(def) => quote_type_name(&def.name),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The suffix is the prepared-statement cache key, so every parameter shape has to have
    /// its own spelling.
    ///
    /// `Date` and `EnumLabel` once shared the character `e`. The two are bound in different
    /// formats (four bytes for a `DATE`, the label's UTF-8 bytes for an enum), so a statement
    /// prepared for one and reused for the other sent bytes the server decoded as the wrong
    /// type.
    #[test]
    fn every_bound_value_gets_a_distinct_statement_shape() {
        let values = [
            Value::Null,
            Value::Bool(true),
            Value::Int(1),
            Value::UInt(1),
            Value::UInt128(1),
            Value::Int128(1),
            Value::Float(1.0),
            Value::Text("one".into()),
            Value::EnumLabel("one".into()),
            Value::Bytes(vec![1]),
            Value::Uuid(uuid::Uuid::nil()),
            Value::Date(chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
            Value::Time(chrono::NaiveTime::MIN),
            Value::Timestamp(
                chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_time(chrono::NaiveTime::MIN),
            ),
            Value::TimestampTz(chrono::DateTime::UNIX_EPOCH),
            Value::Json(br#"{"a":1}"#.to_vec()),
            Value::Decimal("1.0".into()),
            Value::Interval(besu_core::SqlInterval::zero()),
            Value::Inet(besu_core::Inet::new("127.0.0.1").unwrap()),
            Value::Cidr(besu_core::Cidr::new("10.0.0.0/8").unwrap()),
            Value::Bit(besu_core::BitString::new("101").unwrap()),
            Value::Range {
                element: TypeDef::Int4,
                range: Box::new(besu_core::Range::new(
                    besu_core::Bound::Included(Value::Int(1)),
                    besu_core::Bound::Excluded(Value::Int(2)),
                )),
            },
            Value::Geometry(
                besu_core::Geometry::from_hex("0101000000000000000000F03F0000000000000040")
                    .unwrap(),
            ),
            Value::Vector(besu_core::Vector::new(vec![1.0])),
            Value::Array {
                element: TypeDef::Int4,
                values: vec![Value::Int(1)],
            },
        ];

        let mut seen = std::collections::BTreeMap::new();
        for value in values {
            let mut suffix = String::new();
            Postgres::write_statement_suffix(&mut suffix, std::slice::from_ref(&value));
            if let Some(previous) = seen.insert(suffix.clone(), format!("{value:?}")) {
                panic!("`{previous}` and `{value:?}` share the statement shape `{suffix}`");
            }
        }

        // The type of an array is part of its shape, not just that it is an array.
        let mut integers = String::new();
        Postgres::write_statement_suffix(
            &mut integers,
            &[Value::Array {
                element: TypeDef::Int4,
                values: Vec::new(),
            }],
        );
        let mut texts = String::new();
        Postgres::write_statement_suffix(
            &mut texts,
            &[Value::Array {
                element: TypeDef::Text,
                values: Vec::new(),
            }],
        );
        assert_ne!(integers, texts);
    }
}
