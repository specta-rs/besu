use alloc::{string::String, vec::Vec};
use core::fmt;

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
    /// An unsigned integer that fits in `u64`. Only used when the value does not fit in
    /// `i64`; smaller unsigned values are normalised to [`Value::Int`] so that dialects
    /// with a single integer type behave predictably.
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
}

impl Value {
    /// Whether this value is SQL `NULL`.
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
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
    bool => crate::TypeDef::Bool,
    i8 => crate::TypeDef::Int2,
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
}
