use crate::{Error, SqlValue};

pub trait FromValue {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error>
    where
        Self: Sized;
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Null = value {
                Ok(None)
            } else {
                Ok(Some(T::from_value(vec![value])?))
            }
        } else {
            Ok(None)
        }
    }
}

impl FromValue for String {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::String(s) = value {
                Ok(s)
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

impl FromValue for u8 {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Integer(i) = value {
                Ok(i as u8)
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

impl FromValue for i32 {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Integer(i) = value {
                Ok(i as i32)
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

impl FromValue for u64 {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Integer(i) = value {
                Ok(i as u64)
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

impl FromValue for Option<Vec<u8>> {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Binary(b) = value {
                Ok(Some(b))
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

#[cfg(feature = "uuid")]
impl FromValue for uuid::Uuid {
    fn from_value(values: Vec<SqlValue>) -> Result<Self, Error> {
        if let Some(value) = values.into_iter().next() {
            if let SqlValue::Binary(b) = value {
                Ok(uuid::Uuid::from_bytes_le(b.try_into().unwrap()))
            } else {
                todo!();
            }
        } else {
            todo!();
        }
    }
}

// TODO: All standard Rust types!!!
