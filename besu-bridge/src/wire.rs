//! The JSON exchanged between the browser and the server.
//!
//! Every value is a tagged object, `{"t": "<tag>", "v": <payload>}`, so the type travels with
//! the value. A plain JSON number cannot be told apart from a JSON string that happens to
//! contain digits, and nothing about `{"v": 1}` says whether the column was a boolean, an
//! integer or a float.
//!
//! The 128-bit integers are decimal **text** because no widely used JSON number type is
//! guaranteed to hold them: JavaScript's `JSON.parse` returns a `number`, and the top half of
//! `u128` is not representable in one. The 64-bit `int` and `uint` stay numbers, which `serde`
//! reads and writes exactly; a peer written in JavaScript has to parse those with `BigInt`
//! when it expects magnitudes above `2^53`.
//!
//! See the [crate documentation](crate) for the complete table.

use std::fmt;

use besu_core::{Bound, Range, Sql, TypeDef, Value};
use serde::{Deserialize, Serialize};

use crate::BridgeError;

/// One value on the wire.
///
/// The tags are lowercase snake case: `null`, `bool`, `int`, `uint`, `uint128`, `int128`,
/// `float`, `text`, `enum_label`, `bytes`, `uuid`, `date`, `time`, `timestamp`,
/// `timestamp_tz`, `decimal`, `json`, `array`, `interval`, `inet`, `cidr`, `bit`, `range`,
/// `geometry` and `vector`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeValue {
    /// SQL `NULL`. The only tag with no payload: `{"t":"null"}`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer, as a JSON number.
    Int(i64),
    /// An unsigned 64-bit integer, as a JSON number.
    UInt(u64),
    /// An unsigned 128-bit integer, as decimal text inside `v`.
    ///
    /// The decimal text is what keeps every value exact: a JSON number above `2^53` is not
    /// representable in a JavaScript `number`, and a server written in JavaScript would
    /// silently round it.
    UInt128(#[serde(with = "u128_text")] u128),
    /// A signed 128-bit integer, as decimal text inside `v`. See [`BridgeValue::UInt128`].
    Int128(#[serde(with = "i128_text")] i128),
    /// A 64-bit float.
    ///
    /// A finite value is a JSON number. `NaN` and the infinities have no JSON spelling, so
    /// they are the strings `"nan"`, `"inf"` and `"-inf"`.
    Float(#[serde(with = "float_wire")] f64),
    /// UTF-8 text.
    Text(String),
    /// The label of a SQL enum, kept distinct from [`BridgeValue::Text`] so a server can
    /// bind it as the enum's own type rather than as `TEXT`.
    EnumLabel(String),
    /// Binary data, as standard base64 inside `v`.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// A UUID, as its hyphenated lowercase spelling.
    Uuid(uuid::Uuid),
    /// A date with no time, as `YYYY-MM-DD`.
    Date(chrono::NaiveDate),
    /// A time with no date, as `HH:MM:SS[.fraction]`.
    Time(chrono::NaiveTime),
    /// A timestamp with no timezone, as `YYYY-MM-DDTHH:MM:SS[.fraction]`.
    Timestamp(chrono::NaiveDateTime),
    /// A timestamp with a timezone, as RFC 3339.
    TimestampTz(chrono::DateTime<chrono::Utc>),
    /// A fixed-precision decimal, as its exact decimal text. It is never a float.
    Decimal(String),
    /// A JSON document, carried as JSON inside `v` rather than double-encoded as text.
    Json(serde_json::Value),
    /// An array of one element type.
    Array(BridgeArray),
    /// A SQL interval, as its canonical text.
    Interval(String),
    /// A network address, as its canonical text.
    Inet(String),
    /// A network, as its canonical text.
    Cidr(String),
    /// A bit string, as a string of `0` and `1`.
    Bit(String),
    /// A SQL range.
    ///
    /// Boxed because a range holds bound values, and a value that contains a range that
    /// contains a value needs one indirection to have a size.
    Range(Box<BridgeRange>),
    /// Spatial data, as EWKB in base64 inside `v`.
    Geometry(#[serde(with = "base64_bytes")] Vec<u8>),
    /// A `pgvector` vector, as its `[1,2,3]` text.
    Vector(String),
}

impl BridgeValue {
    /// The wire tag for this value, for error messages.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::UInt(_) => "uint",
            Self::UInt128(_) => "uint128",
            Self::Int128(_) => "int128",
            Self::Float(_) => "float",
            Self::Text(_) => "text",
            Self::EnumLabel(_) => "enum_label",
            Self::Bytes(_) => "bytes",
            Self::Uuid(_) => "uuid",
            Self::Date(_) => "date",
            Self::Time(_) => "time",
            Self::Timestamp(_) => "timestamp",
            Self::TimestampTz(_) => "timestamp_tz",
            Self::Decimal(_) => "decimal",
            Self::Json(_) => "json",
            Self::Array(_) => "array",
            Self::Interval(_) => "interval",
            Self::Inet(_) => "inet",
            Self::Cidr(_) => "cidr",
            Self::Bit(_) => "bit",
            Self::Range(_) => "range",
            Self::Geometry(_) => "geometry",
            Self::Vector(_) => "vector",
        }
    }

    /// Whether this value is SQL `NULL`.
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Encode a bound value for the wire.
    ///
    /// The conversion is total for every value a dialect can bind: a value that has no wire
    /// representation is an encode error rather than a value silently dropped.
    pub fn from_value(value: Value) -> Result<Self, besu_core::Error> {
        use Value::*;

        Ok(match value {
            Null => Self::Null,
            Bool(value) => Self::Bool(value),
            Int(value) => Self::Int(value),
            UInt(value) => Self::UInt(value),
            UInt128(value) => Self::UInt128(value),
            Int128(value) => Self::Int128(value),
            Float(value) => Self::Float(value),
            Text(value) => Self::Text(value),
            EnumLabel(value) => Self::EnumLabel(value),
            Bytes(value) => Self::Bytes(value),
            Uuid(value) => Self::Uuid(value),
            Date(value) => Self::Date(value),
            Time(value) => Self::Time(value),
            Timestamp(value) => Self::Timestamp(value),
            TimestampTz(value) => Self::TimestampTz(value),
            Decimal(value) => Self::Decimal(value),
            Json(text) => Self::Json(serde_json::from_slice(&text).map_err(|err| {
                besu_core::Error::encode(
                    String::from_utf8_lossy(&text),
                    format!("the JSON value is not valid JSON: {err}"),
                )
            })?),
            Array { element, values } => Self::Array(BridgeArray {
                element,
                values: values
                    .into_iter()
                    .map(Self::from_value)
                    .collect::<Result<_, _>>()?,
            }),
            Interval(value) => Self::Interval(value.to_string()),
            Inet(value) => Self::Inet(value.to_string()),
            Cidr(value) => Self::Cidr(value.to_string()),
            Bit(value) => Self::Bit(value.to_string()),
            Range { element, range } => Self::Range(Box::new(BridgeRange {
                element,
                bounds: match range.bounds() {
                    None => None,
                    Some((start, end)) => Some((
                        BridgeBound::from_bound(start)?,
                        BridgeBound::from_bound(end)?,
                    )),
                },
            })),
            Geometry(value) => Self::Geometry(value.into_ewkb()),
            Vector(value) => Self::Vector(value.to_string()),
            // `Value` is `#[non_exhaustive]`: a variant added after this crate was written
            // reaches this arm until the bridge learns how to carry it. Refusing is better
            // than inventing a spelling the server would misinterpret.
            other => {
                return Err(besu_core::Error::unsupported(format!(
                    "the bridge has no wire encoding for `{}`",
                    other.display()
                )));
            }
        })
    }

    /// Decode a wire value back into the core [`Value`].
    ///
    /// This is the server's half of [`BridgeValue::from_value`]: a parameter that arrived
    /// over HTTP is rebuilt exactly, including a range's bounds and an array's element type.
    pub fn into_value(self) -> Result<Value, besu_core::Error> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Int(value) => Value::Int(value),
            Self::UInt(value) => Value::UInt(value),
            Self::UInt128(value) => Value::UInt128(value),
            Self::Int128(value) => Value::Int128(value),
            Self::Float(value) => Value::Float(value),
            Self::Text(value) => Value::Text(value),
            Self::EnumLabel(value) => Value::EnumLabel(value),
            Self::Bytes(value) => Value::Bytes(value),
            Self::Uuid(value) => Value::Uuid(value),
            Self::Date(value) => Value::Date(value),
            Self::Time(value) => Value::Time(value),
            Self::Timestamp(value) => Value::Timestamp(value),
            Self::TimestampTz(value) => Value::TimestampTz(value),
            Self::Decimal(value) => Value::Decimal(value),
            Self::Json(value) => Value::Json(
                serde_json::to_vec(&value)
                    .map_err(|err| besu_core::Error::encode("<json>", err.to_string()))?,
            ),
            Self::Array(array) => Value::Array {
                element: array.element,
                values: array
                    .values
                    .into_iter()
                    .map(Self::into_value)
                    .collect::<Result<_, _>>()?,
            },
            Self::Interval(text) => Value::Interval(parse(&text, "an interval")?),
            Self::Inet(text) => Value::Inet(parse(&text, "an inet address")?),
            Self::Cidr(text) => Value::Cidr(parse(&text, "a cidr network")?),
            Self::Bit(text) => Value::Bit(parse(&text, "a bit string")?),
            Self::Range(range) => {
                let BridgeRange { element, bounds } = *range;
                Value::Range {
                    element,
                    range: Box::new(match bounds {
                        None => Range::empty(),
                        Some((start, end)) => Range::new(start.into_bound()?, end.into_bound()?),
                    }),
                }
            }
            Self::Geometry(bytes) => Value::Geometry(
                besu_core::Geometry::from_ewkb(bytes)
                    .map_err(|err| besu_core::Error::encode("<geometry>", err.to_string()))?,
            ),
            Self::Vector(text) => Value::Vector(parse(&text, "a vector")?),
        })
    }
}

/// Parse canonical text into a core scalar, turning the parse error into a core error.
fn parse<T>(text: &str, what: &str) -> Result<T, besu_core::Error>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    text.trim()
        .parse()
        .map_err(|err: T::Err| besu_core::Error::encode(text, format!("{what}: {err}")))
}

/// An array on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeArray {
    /// The declared type of every element.
    ///
    /// It travels with the values because the server's dialect needs it to bind the array
    /// again: PostgreSQL's binary array format carries the element's OID, and a mismatch is
    /// an error rather than a widening cast.
    pub element: TypeDef,
    /// The elements, in order.
    pub values: Vec<BridgeValue>,
}

/// A range on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeRange {
    /// The declared type of the range's subtype.
    pub element: TypeDef,
    /// The two bounds, or `None` for the empty range.
    pub bounds: Option<(BridgeBound, BridgeBound)>,
}

impl BridgeRange {
    /// Build a range from a core range whose bounds have already been encoded.
    pub fn new(element: TypeDef, bounds: Option<(BridgeBound, BridgeBound)>) -> Self {
        Self { element, bounds }
    }
}

/// One end of a [`BridgeRange`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "bound", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeBound {
    /// The bound is part of the range.
    Included(BridgeValue),
    /// The bound is not part of the range.
    Excluded(BridgeValue),
    /// There is no bound.
    Unbounded,
}

impl BridgeBound {
    fn from_bound(bound: &Bound<Value>) -> Result<Self, besu_core::Error> {
        Ok(match bound {
            Bound::Included(value) => Self::Included(BridgeValue::from_value(value.clone())?),
            Bound::Excluded(value) => Self::Excluded(BridgeValue::from_value(value.clone())?),
            Bound::Unbounded => Self::Unbounded,
        })
    }

    fn into_bound(self) -> Result<Bound<Value>, besu_core::Error> {
        Ok(match self {
            Self::Included(value) => Bound::Included(value.into_value()?),
            Self::Excluded(value) => Bound::Excluded(value.into_value()?),
            Self::Unbounded => Bound::Unbounded,
        })
    }
}

/// What the server should do with a [`Request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequestKind {
    /// Run the statement and return its rows.
    #[default]
    Query,
    /// Run the statement and return the number of affected rows.
    Execute,
}

/// A statement sent to the bridge server.
///
/// The SQL is already rendered for the dialect the bridge was built with — the placeholders
/// match the server's database — and the parameters are already encoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The SQL text to run.
    pub sql: String,
    /// The bound parameters, in placeholder order.
    pub params: Vec<BridgeValue>,
    /// Whether the caller wants rows or an affected-row count.
    #[serde(default)]
    pub kind: RequestKind,
}

impl Request {
    /// A request that expects rows.
    pub fn query(sql: impl Into<String>, params: Vec<BridgeValue>) -> Self {
        Self {
            sql: sql.into(),
            params,
            kind: RequestKind::Query,
        }
    }

    /// A request that expects an affected-row count.
    pub fn execute(sql: impl Into<String>, params: Vec<BridgeValue>) -> Self {
        Self {
            sql: sql.into(),
            params,
            kind: RequestKind::Execute,
        }
    }

    /// Build the core statement this request describes, binding every parameter through
    /// `D`'s dialect.
    ///
    /// This is what a server calls before handing the statement to its driver. A parameter
    /// that cannot be rebuilt, or a dialect that rejects one of the values, is an error
    /// rather than a statement with a hole in it.
    pub fn to_sql<D: besu_core::Dialect>(&self) -> Result<Sql<D>, besu_core::Error> {
        let mut sql = Sql::<D>::raw(self.sql.clone());
        for param in &self.params {
            sql.push_arg(param.clone().into_value()?);
        }
        sql.finish()
    }
}

/// What the bridge server sends back.
///
/// A successful query fills `rows` and, when the driver reports them, `columns`. A successful
/// execute fills `affected`. A failure fills `error` and nothing else.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Response {
    /// The column names, in the order each row's values are in.
    ///
    /// Absent when the driver cannot name its columns or the result set is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    /// The rows, each a list of values in column order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<BridgeValue>>>,
    /// The number of rows a statement changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected: Option<u64>,
    /// The server's error, if the statement did not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ServerError>,
}

impl Response {
    /// A response carrying rows and their column names.
    pub fn rows(columns: Option<Vec<String>>, rows: Vec<Vec<BridgeValue>>) -> Self {
        Self {
            columns,
            rows: Some(rows),
            affected: None,
            error: None,
        }
    }

    /// A response carrying an affected-row count.
    pub fn affected(count: u64) -> Self {
        Self {
            columns: None,
            rows: None,
            affected: Some(count),
            error: None,
        }
    }

    /// A response carrying an error.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            error: Some(ServerError {
                message: message.into(),
                code: None,
            }),
            ..Self::default()
        }
    }

    /// Read this response as a result set.
    ///
    /// A server error becomes [`BridgeError::Server`]; a response with neither rows nor an
    /// error is a protocol error rather than an empty result, because an empty result is
    /// `{"rows": []}`.
    pub fn into_rows(self) -> Result<(Vec<String>, Vec<Vec<BridgeValue>>), BridgeError> {
        if let Some(error) = self.error {
            return Err(error.into());
        }
        match self.rows {
            Some(rows) => Ok((self.columns.unwrap_or_default(), rows)),
            None => Err(BridgeError::Protocol(
                "the response has no `rows`; a query returns `{\"rows\": [...]}`".to_owned(),
            )),
        }
    }

    /// Read this response as an affected-row count.
    pub fn into_affected(self) -> Result<u64, BridgeError> {
        if let Some(error) = self.error {
            return Err(error.into());
        }
        self.affected.ok_or_else(|| {
            BridgeError::Protocol(
                "the response has no `affected`; an execute returns `{\"affected\": n}`".to_owned(),
            )
        })
    }
}

/// An error reported by the bridge server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerError {
    /// The human-readable message.
    pub message: String,
    /// An optional machine-readable code, such as a SQLSTATE, when the server has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl ServerError {
    /// Build an error with no code.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(code) => write!(f, "{} ({code})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<String> for ServerError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ServerError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Serialise an unsigned 128-bit integer as decimal text.
mod u128_text {
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<u128, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Serialise a signed 128-bit integer as decimal text.
mod i128_text {
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(value: &i128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<i128, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Serialise binary data as base64.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de};

    use crate::base64;

    pub(super) fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::encode(value))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::decode(&text).map_err(de::Error::custom)
    }
}

/// Serialise a float, spelling the values JSON cannot represent.
mod float_wire {
    use std::fmt;

    use serde::{Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() {
            serializer.serialize_f64(*value)
        } else if value.is_nan() {
            serializer.serialize_str("nan")
        } else if *value > 0.0 {
            serializer.serialize_str("inf")
        } else {
            serializer.serialize_str("-inf")
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        struct FloatVisitor;

        impl<'de> de::Visitor<'de> for FloatVisitor {
            type Value = f64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a finite JSON number or one of `nan`, `inf` and `-inf`")
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<f64, E> {
                Ok(value)
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<f64, E> {
                Ok(value as f64)
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<f64, E> {
                Ok(value as f64)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<f64, E> {
                match value {
                    "nan" => Ok(f64::NAN),
                    "inf" => Ok(f64::INFINITY),
                    "-inf" => Ok(f64::NEG_INFINITY),
                    other => Err(E::custom(format!("`{other}` is not a bridge float"))),
                }
            }
        }

        deserializer.deserialize_any(FloatVisitor)
    }
}

#[cfg(test)]
mod tests {
    use besu_core::{BitString, Cidr, Geometry, Inet, Range, SqlInterval, TypeDef, Value};

    use super::{BridgeValue, Request, RequestKind};

    #[test]
    fn a_request_is_the_documented_shape() {
        let request = Request::query(
            "SELECT $1, $2",
            vec![BridgeValue::Int(7), BridgeValue::Text("x".to_owned())],
        );

        assert_eq!(
            serde_json::to_value(&request).expect("a request serialises"),
            serde_json::json!({
                "sql": "SELECT $1, $2",
                "params": [
                    {"t": "int", "v": 7},
                    {"t": "text", "v": "x"},
                ],
                "kind": "query",
            })
        );

        let execute = Request::execute("DELETE FROM users", Vec::new());
        assert_eq!(execute.kind, RequestKind::Execute);
        assert_eq!(
            serde_json::to_value(&execute).expect("a request serialises")["kind"],
            serde_json::json!("execute")
        );
    }

    #[test]
    fn a_response_is_the_documented_shape() {
        let rows =
            super::Response::rows(Some(vec!["id".to_owned()]), vec![vec![BridgeValue::Int(1)]]);
        assert_eq!(
            serde_json::to_value(&rows).expect("a response serialises"),
            serde_json::json!({
                "columns": ["id"],
                "rows": [[{"t": "int", "v": 1}]],
            })
        );

        assert_eq!(
            serde_json::to_value(super::Response::affected(3)).expect("a response serialises"),
            serde_json::json!({"affected": 3})
        );

        assert_eq!(
            serde_json::to_value(super::Response::error("no such table"))
                .expect("a response serialises"),
            serde_json::json!({"error": {"message": "no such table"}})
        );
    }

    #[test]
    fn non_finite_floats_have_a_spelling() {
        assert_eq!(
            serde_json::to_value(BridgeValue::Float(f64::NAN)).expect("a float serialises"),
            serde_json::json!({"t": "float", "v": "nan"})
        );
        assert_eq!(
            serde_json::to_value(BridgeValue::Float(f64::NEG_INFINITY))
                .expect("a float serialises"),
            serde_json::json!({"t": "float", "v": "-inf"})
        );

        let decoded: BridgeValue =
            serde_json::from_str(r#"{"t":"float","v":"inf"}"#).expect("a float deserialises");
        assert_eq!(decoded, BridgeValue::Float(f64::INFINITY));
    }

    #[test]
    fn every_value_round_trips_through_json_and_back_to_a_value() {
        let date = chrono::NaiveDate::from_ymd_opt(2024, 2, 29).expect("a real date");
        let time = chrono::NaiveTime::from_hms_milli_opt(12, 34, 56, 789).expect("a real time");
        let timestamp = date.and_time(time);
        let timestamp_tz =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).expect("a real time");

        // A little-endian WKB point at the origin, which `Geometry::from_ewkb` accepts.
        let mut wkb = vec![1, 1, 0, 0, 0];
        wkb.extend_from_slice(&0_f64.to_le_bytes());
        wkb.extend_from_slice(&0_f64.to_le_bytes());

        let values = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(-7),
            Value::UInt(u64::MAX),
            Value::UInt128(u128::MAX),
            Value::Int128(i128::MIN),
            Value::Float(f64::INFINITY),
            Value::Text("héllo".to_owned()),
            Value::EnumLabel("happy".to_owned()),
            Value::Bytes(vec![0, 1, 254, 255]),
            Value::Uuid(uuid::Uuid::nil()),
            Value::Date(date),
            Value::Time(time),
            Value::Timestamp(timestamp),
            Value::TimestampTz(timestamp_tz),
            Value::Decimal("1234.5000".to_owned()),
            Value::Json(br#"{"a":1}"#.to_vec()),
            Value::Array {
                element: TypeDef::Int4,
                values: vec![Value::Int(1), Value::Null, Value::Int(3)],
            },
            Value::Interval(SqlInterval::new(1, 2, 3)),
            Value::Inet("10.0.0.1".parse::<Inet>().expect("an address")),
            Value::Cidr("10.0.0.0/24".parse::<Cidr>().expect("a network")),
            Value::Bit(BitString::new("1010").expect("a bit string")),
            Value::Range {
                element: TypeDef::Int4,
                range: Box::new(Range::new(
                    besu_core::Bound::Included(Value::Int(1)),
                    besu_core::Bound::Excluded(Value::Int(5)),
                )),
            },
            Value::Geometry(Geometry::from_ewkb(wkb).expect("a point")),
            Value::Vector(besu_core::Vector::new(vec![1.0_f32, 2.5])),
        ];

        for value in values {
            let wire = BridgeValue::from_value(value.clone()).expect("the value encodes");
            let json = serde_json::to_string(&wire).expect("the wire value serialises");
            let decoded: BridgeValue =
                serde_json::from_str(&json).expect("the wire value deserialises");
            assert_eq!(decoded, wire, "{json}");
            assert_eq!(
                decoded.into_value().expect("the value decodes"),
                value,
                "{json}"
            );
        }
    }
}
