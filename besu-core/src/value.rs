use alloc::{string::String, vec::Vec};
use core::fmt;

use crate::scalars::{BitString, Bound, Cidr, Geometry, Inet, Range, SqlInterval, Vector};

/// An owned, dialect-independent SQL value.
///
/// [`Sql`](crate::Sql) stores its parameters as `Vec<D::Value>` where a dialect decides how
/// (and whether) to translate them. For every driver shipped with Besu that is this type
/// itself, which means a parameter is encoded exactly once and never copied again.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer that fits in `i64`.
    Int(i64),
    /// An unsigned integer that fits in `u64`.
    ///
    /// This is always used for a `u64`, even when the value happens to fit in an `i64`: a
    /// column has one SQL type, and a prepared statement must not receive two different
    /// parameter types for the same SQL.
    UInt(u64),
    /// An unsigned 128-bit integer.
    ///
    /// This is always used for a `u128`, even when the value fits in a `u64`, for the same
    /// reason [`Value::UInt`] is always used for a `u64`: a column has one SQL type, and a
    /// prepared statement must not receive two different parameter types for the same SQL.
    UInt128(u128),
    /// A signed 128-bit integer.
    ///
    /// This is always used for an `i128`, even when the value fits in an `i64`, so that a
    /// prepared statement sees one parameter type for the column.
    Int128(i128),
    /// A 64-bit float.
    Float(f64),
    /// UTF-8 text.
    Text(String),
    /// A label of a SQL enum, carried as text.
    ///
    /// The SQL type of a label is the enum type of the column it is written to or compared
    /// with, and nothing at the binding site names that type. MySQL and SQLite bind it as
    /// ordinary text; PostgreSQL declares the parameter untyped so the server infers the
    /// enum from the column, because an explicitly `TEXT` parameter is not assignable to an
    /// enum without a cast.
    EnumLabel(String),
    /// Binary data.
    Bytes(Vec<u8>),
    /// A UUID.
    Uuid(uuid::Uuid),
    /// A date with no time.
    Date(chrono::NaiveDate),
    /// A time with no date.
    Time(chrono::NaiveTime),
    /// A timestamp with no timezone.
    Timestamp(chrono::NaiveDateTime),
    /// A timestamp with a timezone.
    TimestampTz(chrono::DateTime<chrono::Utc>),
    /// A fixed-precision decimal, as its exact decimal text.
    ///
    /// The text is the canonical spelling — an optional `-`, digits, an optional `.` and the
    /// scale's digits — and is never parsed into a floating-point number, because the whole
    /// point of a decimal column is that the value is exact. PostgreSQL encodes it into the
    /// binary `NUMERIC` format; MySQL binds the text and lets the server coerce it.
    Decimal(String),
    /// An array of another type.
    ///
    /// The element's [`TypeDef`](crate::TypeDef) travels with the values because PostgreSQL's binary array
    /// format carries the element's OID in its header, and the server rejects a parameter
    /// whose element type does not match the column exactly. Encoding an `INTEGER[]` column
    /// with an inferred `BIGINT` element is not a widening cast in that protocol; it is an
    /// error. Carrying the declared type is what makes the encoder right the first time.
    Array {
        /// The declared type of every element.
        element: crate::TypeDef,
        /// The elements, in order.
        values: Vec<Value>,
    },
    /// JSON, kept as text so dialects can render it as `json` or `jsonb`.
    Json(Vec<u8>),
    /// A SQL interval.
    ///
    /// PostgreSQL has a native `INTERVAL` and a three-field binary format; MySQL and SQLite
    /// have no interval type at all, so they store the value's canonical text.
    Interval(SqlInterval),
    /// A PostgreSQL `INET`, as its canonical textual spelling.
    Inet(Inet),
    /// A PostgreSQL `CIDR`, as its canonical textual spelling.
    Cidr(Cidr),
    /// A SQL bit string, as `0` and `1` characters.
    ///
    /// PostgreSQL's `BIT(n)` and `BIT VARYING(n)` send a length and the packed bytes; MySQL's
    /// `BIT(n)` sends the packed bytes alone; SQLite stores the text.
    Bit(BitString),
    /// A SQL range.
    ///
    /// The element's [`TypeDef`](crate::TypeDef) travels with the bounds for the same reason an
    /// array carries its element's: PostgreSQL's binary range format stores each bound as the
    /// element's own encoding, with no type name on the wire, so the encoder has to know what
    /// it is writing. [`Value::Range`] stores each bound as a [`Value`], which is the form the
    /// element encoder already accepts.
    Range {
        /// The declared type of the range's subtype.
        element: crate::TypeDef,
        /// The bounds, or the empty range.
        ///
        /// Boxed because a [`Value`] is what a bound holds, and a `Value` that contains a
        /// range that contains a `Value` needs one indirection to have a size.
        range: alloc::boxed::Box<Range<Value>>,
    },
    /// A spatial value, as EWKB bytes.
    ///
    /// PostgreSQL's PostGIS sends and accepts EWKB; MySQL sends an SRID prefix followed by
    /// WKB, which the driver converts. The value itself is always the EWKB form.
    Geometry(Geometry),
    /// A `pgvector` vector.
    ///
    /// PostgreSQL binds it untyped in the extension's binary form so the server infers
    /// `vector(n)` from the column, and reads that form back; the `[1,2,3]` text is parsed if a
    /// result arrives that way. SQLite and MySQL store the text.
    Vector(Vector),
}

impl Value {
    /// Whether this value is SQL `NULL`.
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Render a range in PostgreSQL's canonical text spelling.
    ///
    /// A dialect without a range type stores this text, and a value read back is parsed with
    /// [`Range::parse_with`], so the two directions agree. The
    /// spelling uses each bound's diagnostic form, which quotes a text value exactly as the
    /// parser expects it.
    pub fn range_text(range: &Range<Value>) -> String {
        let mut out = String::new();
        let Some((start, end)) = range.bounds() else {
            out.push_str("empty");
            return out;
        };
        out.push_str(match start {
            Bound::Included(_) => "[",
            Bound::Excluded(_) | Bound::Unbounded => "(",
        });
        if let Some(value) = start.value() {
            out.push_str(&range_bound_text(value));
        }
        out.push(',');
        if let Some(value) = end.value() {
            out.push_str(&range_bound_text(value));
        }
        out.push_str(match end {
            Bound::Included(_) => "]",
            Bound::Excluded(_) | Bound::Unbounded => ")",
        });
        out
    }

    /// Render the value for diagnostics. Quoting is *not* SQL-escaped, so never feed the
    /// result back into a query.
    pub fn display(&self) -> impl fmt::Display + '_ {
        struct Display<'a>(&'a Value);

        impl fmt::Display for Display<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.0 {
                    Value::Null => f.write_str("NULL"),
                    Value::Bool(v) => write!(f, "{}", if *v { "TRUE" } else { "FALSE" }),
                    Value::Int(v) => write!(f, "{v}"),
                    Value::UInt(v) => write!(f, "{v}"),
                    Value::UInt128(v) => write!(f, "{v}"),
                    Value::Int128(v) => write!(f, "{v}"),
                    Value::Float(v) => write!(f, "{v}"),
                    Value::Text(v) => write!(f, "'{}'", v.escape_debug()),
                    Value::EnumLabel(v) => write!(f, "'{}'", v.escape_debug()),
                    Value::Bytes(v) => {
                        f.write_str("x'")?;
                        for b in v {
                            write!(f, "{b:02x}")?;
                        }
                        f.write_str("'")
                    }
                    Value::Uuid(v) => write!(f, "'{v}'"),
                    Value::Date(v) => write!(f, "'{v}'"),
                    Value::Time(v) => write!(f, "'{v}'"),
                    Value::Timestamp(v) => write!(f, "'{v}'"),
                    Value::TimestampTz(v) => write!(f, "'{v}'"),
                    Value::Decimal(v) => write!(f, "{v}"),
                    Value::Array { values, .. } => {
                        f.write_str("ARRAY[")?;
                        for (i, value) in values.iter().enumerate() {
                            if i != 0 {
                                f.write_str(", ")?;
                            }
                            write!(f, "{}", value.display())?;
                        }
                        f.write_str("]")
                    }
                    Value::Json(v) => write!(f, "'{}'", String::from_utf8_lossy(v).escape_debug()),
                    Value::Interval(v) => write!(f, "'{v}'"),
                    Value::Inet(v) => write!(f, "'{v}'"),
                    Value::Cidr(v) => write!(f, "'{v}'"),
                    Value::Bit(v) => write!(f, "B'{}'", v.as_str()),
                    Value::Range { range, .. } => {
                        f.write_str("'")?;
                        match range.bounds() {
                            None => f.write_str("empty")?,
                            Some((start, end)) => {
                                f.write_str(match start {
                                    Bound::Included(_) => "[",
                                    Bound::Excluded(_) | Bound::Unbounded => "(",
                                })?;
                                if let Some(value) = start.value() {
                                    write!(f, "{}", value.display())?;
                                }
                                f.write_str(",")?;
                                if let Some(value) = end.value() {
                                    write!(f, "{}", value.display())?;
                                }
                                f.write_str(match end {
                                    Bound::Included(_) => "]",
                                    Bound::Excluded(_) | Bound::Unbounded => ")",
                                })?;
                            }
                        }
                        f.write_str("'")
                    }
                    Value::Geometry(v) => write!(f, "'{v}'"),
                    Value::Vector(v) => write!(f, "'{v}'"),
                }
            }
        }

        Display(self)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.display(), f)
    }
}

macro_rules! from_int {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for Value {
                fn from(value: $ty) -> Self {
                    Self::Int(value as i64)
                }
            }
        )*
    };
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        // Always `UInt`, even when the value happens to fit in an `i64`. A `u64` column has
        // one SQL type, and a database caches a prepared statement per SQL string: binding
        // `INT8` for small values and `NUMERIC` for large ones would hand the same statement
        // two different parameter types.
        Self::UInt(value)
    }
}

impl From<u128> for Value {
    fn from(value: u128) -> Self {
        // Always `UInt128`, for the same prepared-statement reason as `u64`.
        Self::UInt128(value)
    }
}

impl From<i128> for Value {
    fn from(value: i128) -> Self {
        // Always `Int128`, for the same prepared-statement reason as `u64`.
        Self::Int128(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::Float(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<char> for Value {
    fn from(value: char) -> Self {
        Self::Text(value.into())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.into())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(v) => v.into(),
            None => Self::Null,
        }
    }
}

impl From<uuid::Uuid> for Value {
    fn from(value: uuid::Uuid) -> Self {
        Self::Uuid(value)
    }
}

impl From<chrono::NaiveDate> for Value {
    fn from(value: chrono::NaiveDate) -> Self {
        Self::Date(value)
    }
}

impl From<chrono::NaiveTime> for Value {
    fn from(value: chrono::NaiveTime) -> Self {
        Self::Time(value)
    }
}

impl From<chrono::NaiveDateTime> for Value {
    fn from(value: chrono::NaiveDateTime) -> Self {
        Self::Timestamp(value)
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Value {
    fn from(value: chrono::DateTime<chrono::Utc>) -> Self {
        Self::TimestampTz(value)
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Self::Json(value.to_string().into_bytes())
    }
}

impl From<SqlInterval> for Value {
    fn from(value: SqlInterval) -> Self {
        Self::Interval(value)
    }
}

impl From<Inet> for Value {
    fn from(value: Inet) -> Self {
        Self::Inet(value)
    }
}

impl From<Cidr> for Value {
    fn from(value: Cidr) -> Self {
        Self::Cidr(value)
    }
}

impl From<BitString> for Value {
    fn from(value: BitString) -> Self {
        Self::Bit(value)
    }
}

impl From<Geometry> for Value {
    fn from(value: Geometry) -> Self {
        Self::Geometry(value)
    }
}

impl From<Vector> for Value {
    fn from(value: Vector) -> Self {
        Self::Vector(value)
    }
}

/// A `Vec` of a scalar is an array of that scalar's canonical type.
///
/// The element type has to be recovered from the Rust type, because `From` is the only
/// conversion `#[derive(Table)]` uses for a whole-model insert. `Vec<u8>` is deliberately
/// absent: it is [`Value::Bytes`], and a byte vector means binary data rather than an array
/// of one-byte integers. A column with a narrower declared element type — `VARCHAR(20)[]`
/// whose Rust type is also `Vec<String>` — binds its elements as `TEXT[]` here; the typed
/// expression path carries the declared element type instead.
macro_rules! from_vec {
    ($($ty:ty => $element:expr),* $(,)?) => {
        $(
            impl From<Vec<$ty>> for Value {
                fn from(values: Vec<$ty>) -> Self {
                    Self::Array {
                        element: $element,
                        values: values.into_iter().map(Value::from).collect(),
                    }
                }
            }
        )*
    };
}

from_vec! {
    bool => crate::TypeDef::Bool,    i8 => crate::TypeDef::Int2,
    i16 => crate::TypeDef::Int2,
    i32 => crate::TypeDef::Int4,
    i64 => crate::TypeDef::Int8,
    u16 => crate::TypeDef::UInt(16),
    u32 => crate::TypeDef::UInt(32),
    u64 => crate::TypeDef::UInt(64),
    u128 => crate::TypeDef::UInt(128),
    i128 => crate::TypeDef::Decimal(39, 0),
    f32 => crate::TypeDef::Float4,
    f64 => crate::TypeDef::Float8,
    String => crate::TypeDef::Text,
    Vec<u8> => crate::TypeDef::Bytes,
    uuid::Uuid => crate::TypeDef::Uuid,
    chrono::NaiveDate => crate::TypeDef::Date,
    chrono::NaiveTime => crate::TypeDef::Time,
    chrono::NaiveDateTime => crate::TypeDef::Timestamp,
    chrono::DateTime<chrono::Utc> => crate::TypeDef::TimestampTz,
    serde_json::Value => crate::TypeDef::Json,
    SqlInterval => crate::TypeDef::Interval,
    Inet => crate::TypeDef::Inet,
    Cidr => crate::TypeDef::Cidr,
}

/// A range is bound with its subtype, because PostgreSQL's binary range format stores each
/// bound as the subtype's own encoding.
///
/// The subtype has to be named per element type, since `From` is the only conversion a
/// `SqlType::Rust` has to provide and a generic `Range<T>` cannot know its SQL type.
macro_rules! from_range {
    ($($rust:ty => $element:expr, $convert:expr;)*) => {
        $(
            impl From<Range<$rust>> for Value {
                fn from(range: Range<$rust>) -> Self {
                    Self::Range {
                        element: $element,
                        range: alloc::boxed::Box::new(range.map($convert)),
                    }
                }
            }

            impl From<Vec<Range<$rust>>> for Value {
                fn from(ranges: Vec<Range<$rust>>) -> Self {
                    Self::Array {
                        element: crate::TypeDef::Range(crate::types::TypeRef::from_static(
                            &$element,
                        )),
                        values: ranges.into_iter().map(Value::from).collect(),
                    }
                }
            }
        )*
    };
}

from_range! {
    i32 => crate::TypeDef::Int4, |value| Value::Int(i64::from(value));
    i64 => crate::TypeDef::Int8, Value::Int;
    chrono::NaiveDateTime => crate::TypeDef::Timestamp, Value::Timestamp;
    chrono::DateTime<chrono::Utc> => crate::TypeDef::TimestampTz, Value::TimestampTz;
    chrono::NaiveDate => crate::TypeDef::Date, Value::Date;
    // The only text-shaped range Besu models is `numrange`, whose subtype is `numeric`.
    String => crate::TypeDef::Decimal(0, 0), Value::Decimal;
}

/// One range bound in a spelling [`Range::parse_with`] reads back.
///
/// A date, a timestamp and a decimal are written in a form their own `FromStr` accepts, which
/// is what lets a dialect without ranges store the text and decode it again. Text is quoted
/// whenever the bracket syntax could take one of its characters, or when it is empty — an
/// empty unquoted bound is an infinite one — and inside the quotes a `"` or a `\` is escaped
/// with a backslash, exactly as PostgreSQL writes it.
fn range_bound_text(value: &Value) -> String {
    match value {
        Value::Text(text) => {
            let needs_quotes = text.is_empty()
                || text.chars().any(|ch| {
                    matches!(ch, ',' | '"' | '\\' | '(' | ')' | '[' | ']') || ch.is_whitespace()
                });
            if needs_quotes {
                let mut escaped = String::with_capacity(text.len());
                for ch in text.chars() {
                    if matches!(ch, '"' | '\\') {
                        escaped.push('\\');
                    }
                    escaped.push(ch);
                }
                alloc::format!("\"{escaped}\"")
            } else {
                text.clone()
            }
        }
        Value::Date(date) => date.format("%Y-%m-%d").to_string(),
        Value::Time(time) => time.format("%H:%M:%S%.f").to_string(),
        Value::Timestamp(timestamp) => timestamp.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),
        // `DateTime<Utc>`'s `FromStr` is RFC 3339, so that is the spelling to write.
        Value::TimestampTz(timestamp) => timestamp.to_rfc3339(),
        Value::Decimal(text) => text.clone(),
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::UInt128(value) => value.to_string(),
        Value::Int128(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        // Every value a range can hold is covered above; the diagnostic form is the honest
        // fallback for anything else.
        other => other.display().to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::scalars::{Bound, ParseValueError, Range};

    /// Read a rendered range back as text, which is what a dialect without ranges does.
    fn parse_text_range(text: &str) -> Result<Range<Value>, ParseValueError> {
        Range::parse_with(text, |bound| {
            Ok::<_, core::convert::Infallible>(Value::Text(bound.to_owned()))
        })
    }

    #[test]
    fn a_text_range_bound_survives_the_text_spelling() {
        // A bound is text the bracket syntax has to be told apart from; an empty bound is the
        // one that used to be written bare and then read back as an unbounded range.
        for bound in ["", "a,b", "a\"b", "a\\b", "a\\", "a b", "[x]", "a\\\\b"] {
            let range = Range::new(
                Bound::Included(Value::Text(bound.to_owned())),
                Bound::Unbounded,
            );
            let rendered = Value::range_text(&range);
            assert_eq!(
                parse_text_range(&rendered).as_ref(),
                Ok(&range),
                "`{bound}` rendered as `{rendered}`"
            );
        }
    }

    #[test]
    fn a_numeric_range_survives_the_text_spelling() {
        let range = Range::closed(Value::Int(1), Value::Int(5));
        let rendered = Value::range_text(&range);
        assert_eq!(rendered, "[1,5]");
        let parsed = Range::parse_with(&rendered, |bound| {
            bound
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| ParseValueError::new(bound, "an integer"))
        });
        assert_eq!(parsed, Ok(range));
        assert_eq!(Value::range_text(&Range::<Value>::empty()), "empty");
    }
}
