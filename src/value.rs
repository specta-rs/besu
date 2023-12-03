#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Binary(Vec<u8>),
    // Sqlite doesn't have a dedicated `BOOL`
    Boolean(bool),
    Float(f64),
    Integer(i64),
    String(String),
    Default, // TODO: I don't thing I like this
    Null,
}

impl<T: Into<SqlValue>> Into<SqlValue> for Option<T> {
    fn into(self) -> SqlValue {
        self.map(Into::into).unwrap_or(SqlValue::Null)
    }
}

impl From<&'static str> for SqlValue {
    fn from(value: &'static str) -> Self {
        SqlValue::String(value.to_string()) // TODO: Maybe use `Cow` so it's no alloc?
    }
}

impl From<String> for SqlValue {
    fn from(value: String) -> Self {
        SqlValue::String(value)
    }
}

impl From<u8> for SqlValue {
    fn from(value: u8) -> Self {
        SqlValue::Integer(value as i64) // TODO: I don't think this is safe.
    }
}

impl From<u64> for SqlValue {
    fn from(value: u64) -> Self {
        SqlValue::Integer(value as i64) // TODO: I don't think this is safe.
    }
}

impl From<i32> for SqlValue {
    fn from(value: i32) -> Self {
        SqlValue::Integer(value as i64) // TODO: I don't think this is safe.
    }
}

impl From<bool> for SqlValue {
    fn from(value: bool) -> Self {
        SqlValue::Boolean(value)
    }
}

impl From<Vec<u8>> for SqlValue {
    fn from(value: Vec<u8>) -> Self {
        SqlValue::Binary(value)
    }
}

#[cfg(feature = "uuid")]
impl From<uuid::Uuid> for SqlValue {
    fn from(value: uuid::Uuid) -> Self {
        SqlValue::Binary(value.to_bytes_le().to_vec())
    }
}
