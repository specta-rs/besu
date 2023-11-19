mod contains;
mod equals;

// pub(crate) use contains::Contains;
pub(crate) use equals::TyEq;

// TODO: Remove
pub use contains::*;

pub mod construct {
    use crate::schema::{ColumnDef, TableDef};

    pub const fn table_def(
        name: &'static str,
        fields: &'static [(&'static str, &'static str)],
    ) -> TableDef {
        TableDef { name, fields }
    }

    pub const fn column_def(table_name: &'static str, name: &'static str) -> ColumnDef {
        ColumnDef { table_name, name }
    }
}
