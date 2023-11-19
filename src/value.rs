#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl SqlValue {
    // TODO: Don't put user values in the query string. Use the `values` collection for it!
    pub(crate) fn to_sql(&self) -> String {
        // This function is 100% vunerable to SQL injection
        match self {
            SqlValue::Null => "NULL".to_string(),
            SqlValue::Bool(b) => b.to_string(),
            SqlValue::Integer(i) => i.to_string(),
            SqlValue::Float(f) => f.to_string(),
            SqlValue::Text(s) => format!("'{}'", s),
        }
    }
}
