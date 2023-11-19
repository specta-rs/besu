#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    Array(Vec<SqlValue>),
}

impl SqlValue {
    // TODO: Don't put user values in the query string. Use the `values` collection for it!
    pub(crate) fn to_sql(&self) -> String {
        // This function is 100% vunerable to SQL injection
        match self {
            SqlValue::Null => "NULL".to_string(),
            SqlValue::Boolean(b) => b.to_string(),
            SqlValue::Integer(i) => i.to_string(),
            SqlValue::Float(f) => f.to_string(),
            SqlValue::Text(s) => format!("'{}'", s),
            SqlValue::Array(a) => todo!(), // TODO: I don't think Sqlite supports this
        }
    }
}

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
            if let SqlValue::Text(s) = value {
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
