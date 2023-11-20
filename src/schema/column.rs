use crate::SqlValue;

use super::Model;

// This type is sealed!
pub struct ColumnDef {
    pub(crate) table_name: &'static str,
    pub(crate) name: &'static str,
}

pub trait Column {
    /// The Rust struct which represents the model this column belongs to.
    type Model: Model;

    /// The Rust type of the column.
    type Type: Into<SqlValue> + 'static;

    fn def() -> ColumnDef;
}
