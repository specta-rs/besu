//! Decoding wire values into Rust types.
//!
//! A [`BridgeRow`] already holds parsed values, so decoding never touches JSON: each
//! implementation reads one [`BridgeValue`] and converts it. The conversions are strict about
//! the wire tag — a `date` is not quietly read as a `timestamp` — because a server that
//! answers with the wrong tag is a server bug, and an error naming the column is easier to
//! act on than a plausible-looking wrong value.

use besu_core::{Decode, Dialect};

use crate::{Bridge, BridgeError, BridgeRow, BridgeValue, Transport};

fn column(row: &BridgeRow, index: usize) -> Result<&BridgeValue, BridgeError> {
    row.value(index)
        .ok_or_else(|| BridgeError::decode(index, "the row has no value at this column"))
}

fn wrong(value: &BridgeValue, index: usize, expected: &str) -> BridgeError {
    BridgeError::decode(
        index,
        format!(
            "expected {expected} from the bridge server, found a `{}` value",
            value.tag()
        ),
    )
}

/// A Rust type that a wire value can be read as.
///
/// Kept private because it exists to serve two public traits: [`Decode`] for a scalar, and
/// the array impls built on the same conversions.
trait FromWire: Sized {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError>;
}

macro_rules! impl_integers {
    ($($ty:ty),* $(,)?) => {
        $(
            impl FromWire for $ty {
                fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
                    let number: i128 = match value {
                        BridgeValue::Int(value) => i128::from(*value),
                        BridgeValue::UInt(value) => i128::try_from(*value).map_err(|_| {
                            BridgeError::decode(
                                index,
                                "the unsigned integer is too large for a signed value",
                            )
                        })?,
                        BridgeValue::Int128(value) => *value,
                        BridgeValue::UInt128(value) => i128::try_from(*value).map_err(|_| {
                            BridgeError::decode(
                                index,
                                "the unsigned integer is too large for a signed value",
                            )
                        })?,
                        other => return Err(wrong(other, index, "an integer")),
                    };
                    <$ty>::try_from(number).map_err(|_| {
                        BridgeError::decode(
                            index,
                            concat!(
                                "the integer on the wire does not fit in `",
                                stringify!($ty),
                                "`"
                            ),
                        )
                    })
                }
            }
        )*
    };
}

impl_integers!(i8, i16, i32, i64, u8, u16, u32, u64, i128);

impl FromWire for u128 {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            // The wide unsigned variant is kept as itself rather than going through `i128`,
            // because the top half of `u128` has no signed representation.
            BridgeValue::UInt128(value) => Ok(*value),
            BridgeValue::Int(value) => u128::try_from(*value).map_err(|_| {
                BridgeError::decode(index, "a negative integer cannot be decoded as `u128`")
            }),
            BridgeValue::UInt(value) => Ok(u128::from(*value)),
            BridgeValue::Int128(value) => u128::try_from(*value).map_err(|_| {
                BridgeError::decode(index, "a negative integer cannot be decoded as `u128`")
            }),
            other => Err(wrong(other, index, "an integer")),
        }
    }
}

impl FromWire for bool {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Bool(value) => Ok(*value),
            other => Err(wrong(other, index, "a boolean")),
        }
    }
}

impl FromWire for f64 {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Float(value) => Ok(*value),
            other => Err(wrong(other, index, "a float")),
        }
    }
}

impl FromWire for f32 {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        // A `REAL` column and a `DOUBLE PRECISION` column both arrive as `float`; the width
        // is the Rust type's decision, exactly as it is in the sqlx drivers.
        f64::from_wire(value, index).map(|value| value as f32)
    }
}

impl FromWire for String {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            // A `NUMERIC` column is exact decimal text, and an enum column is a label;
            // neither has a Rust type of its own here, and both are legitimately a `String`.
            BridgeValue::Text(value)
            | BridgeValue::EnumLabel(value)
            | BridgeValue::Decimal(value) => Ok(value.clone()),
            other => Err(wrong(other, index, "text")),
        }
    }
}

impl FromWire for Vec<u8> {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Bytes(value) => Ok(value.clone()),
            other => Err(wrong(other, index, "binary data")),
        }
    }
}

impl FromWire for uuid::Uuid {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Uuid(value) => Ok(*value),
            other => Err(wrong(other, index, "a UUID")),
        }
    }
}

macro_rules! impl_chrono {
    ($($ty:ty => $variant:ident, $what:literal;)*) => {
        $(
            impl FromWire for $ty {
                fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
                    match value {
                        BridgeValue::$variant(value) => Ok(*value),
                        other => Err(wrong(other, index, $what)),
                    }
                }
            }
        )*
    };
}

impl_chrono! {
    chrono::NaiveDate => Date, "a date";
    chrono::NaiveTime => Time, "a time";
    chrono::NaiveDateTime => Timestamp, "a timestamp";
    chrono::DateTime<chrono::Utc> => TimestampTz, "a timestamp with a timezone";
}

impl FromWire for serde_json::Value {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Json(value) => Ok(value.clone()),
            other => Err(wrong(other, index, "a JSON document")),
        }
    }
}

macro_rules! impl_text_variant {
    ($($ty:ty => $variant:ident, $tag:literal;)*) => {
        $(
            impl FromWire for $ty {
                fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
                    let text = match value {
                        BridgeValue::$variant(text) => text,
                        other => return Err(wrong(other, index, $tag)),
                    };
                    text.trim().parse().map_err(|err: besu_core::ParseValueError| {
                        BridgeError::decode(index, format!("not a valid {}: {err}", $tag))
                    })
                }
            }
        )*
    };
}

impl_text_variant! {
    besu_core::SqlInterval => Interval, "interval";
    besu_core::Inet => Inet, "inet address";
    besu_core::Cidr => Cidr, "cidr network";
    besu_core::BitString => Bit, "bit string";
    besu_core::Vector => Vector, "vector";
}

impl FromWire for besu_core::Geometry {
    fn from_wire(value: &BridgeValue, index: usize) -> Result<Self, BridgeError> {
        match value {
            BridgeValue::Geometry(bytes) => Self::from_ewkb(bytes.clone())
                .map_err(|err| BridgeError::decode(index, err.to_string())),
            other => Err(wrong(other, index, "spatial data")),
        }
    }
}

macro_rules! impl_decode {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<T: Transport, D: Dialect> Decode<Bridge<T, D>> for $ty {
                fn decode(row: &BridgeRow, offset: &mut usize) -> Result<Self, BridgeError> {
                    let index = *offset;
                    *offset += 1;
                    <$ty as FromWire>::from_wire(column(row, index)?, index)
                }
            }
        )*
    };
}

impl_decode!(
    bool,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
    i128,
    u128,
    f32,
    f64,
    String,
    Vec<u8>,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    serde_json::Value,
    besu_core::SqlInterval,
    besu_core::Inet,
    besu_core::Cidr,
    besu_core::BitString,
    besu_core::Geometry,
    besu_core::Vector,
);

/// An array is one wire value, so it consumes one column.
///
/// `Vec<u8>` is deliberately absent: it is binary data, exactly as it is in
/// [`besu_core::Value`]. Every other `Vec` of a wire scalar is an array of that scalar.
macro_rules! impl_arrays {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<T: Transport, D: Dialect> Decode<Bridge<T, D>> for Vec<$ty> {
                fn decode(row: &BridgeRow, offset: &mut usize) -> Result<Self, BridgeError> {
                    let index = *offset;
                    *offset += 1;
                    match column(row, index)? {
                        BridgeValue::Array(array) => array
                            .values
                            .iter()
                            .map(|value| <$ty as FromWire>::from_wire(value, index))
                            .collect(),
                        other => Err(wrong(other, index, "an array")),
                    }
                }
            }
        )*
    };
}

impl_arrays!(
    bool,
    i8,
    i16,
    i32,
    i64,
    u16,
    u32,
    u64,
    i128,
    u128,
    f32,
    f64,
    String,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    serde_json::Value,
);

/// Decode one end of a wire range.
fn range_bound<E: FromWire>(
    bound: &crate::BridgeBound,
    index: usize,
) -> Result<besu_core::Bound<E>, BridgeError> {
    Ok(match bound {
        crate::BridgeBound::Included(value) => {
            besu_core::Bound::Included(E::from_wire(value, index)?)
        }
        crate::BridgeBound::Excluded(value) => {
            besu_core::Bound::Excluded(E::from_wire(value, index)?)
        }
        crate::BridgeBound::Unbounded => besu_core::Bound::Unbounded,
    })
}

/// A range is one wire value, so it consumes one column.
///
/// The element types match the ones [`besu_core::Value`] can build a range from.
macro_rules! impl_ranges {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<T: Transport, D: Dialect> Decode<Bridge<T, D>> for besu_core::Range<$ty> {
                fn decode(row: &BridgeRow, offset: &mut usize) -> Result<Self, BridgeError> {
                    let index = *offset;
                    *offset += 1;
                    match column(row, index)? {
                        BridgeValue::Range(range) => match &range.bounds {
                            None => Ok(besu_core::Range::empty()),
                            Some((start, end)) => Ok(besu_core::Range::new(
                                range_bound::<$ty>(start, index)?,
                                range_bound::<$ty>(end, index)?,
                            )),
                        },
                        other => Err(wrong(other, index, "a range")),
                    }
                }
            }
        )*
    };
}

impl_ranges!(
    i32,
    i64,
    chrono::NaiveDate,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    String,
);
