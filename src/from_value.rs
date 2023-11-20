use crate::SqlValue;

pub trait FromValue {
    fn from_value(value: Vec<SqlValue>) -> Result<Self, ()>
    where
        Self: Sized;
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: Vec<SqlValue>) -> Result<Self, ()> {
        if let Some(value) = value.into_iter().next() {
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
    fn from_value(value: Vec<SqlValue>) -> Result<Self, ()> {
        if let Some(value) = value.into_iter().next() {
            if let SqlValue::String(s) = value {
                Ok(s)
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }
}

impl FromValue for u8 {
    fn from_value(value: Vec<SqlValue>) -> Result<Self, ()> {
        if let Some(value) = value.into_iter().next() {
            if let SqlValue::Integer(i) = value {
                Ok(i as u8)
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }
}

impl FromValue for u64 {
    fn from_value(value: Vec<SqlValue>) -> Result<Self, ()> {
        if let Some(value) = value.into_iter().next() {
            if let SqlValue::Integer(i) = value {
                Ok(i as u64)
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }
}
