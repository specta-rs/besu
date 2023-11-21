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
    #[deprecated(note = "This function is 100% vulnerable to SQL injection")]
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

impl From<bool> for SqlValue {
    fn from(value: bool) -> Self {
        SqlValue::Boolean(value)
    }
}
