#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Binary(Vec<u8>),
    // Sqlite doesn't have a dedicated `BOOL`
    Boolean(bool),
    Float(f64),
    Integer(i64),
    String(String),
    Null,
}

impl SqlValue {
    // TODO: Don't put user values in the query string. Use the `values` collection for it!
    pub(crate) fn to_sql(&self) -> String {
        // This function is 100% vulnerable to SQL injection
        match self {
            SqlValue::Binary(binary) => todo!(),
            SqlValue::Boolean(b) => b.to_string(),
            SqlValue::Float(f) => f.to_string(),
            SqlValue::Integer(i) => i.to_string(),
            SqlValue::String(s) => format!("'{}'", s),
            SqlValue::Null => "NULL".to_string(),
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
