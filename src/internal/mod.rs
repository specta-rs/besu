mod contains;
mod equals;

// pub(crate) use contains::Contains;
pub(crate) use equals::TyEq;

// TODO: Remove
pub use contains::*;

pub mod construct {
    use std::marker::PhantomData;

    use crate::schema::{ColumnDef, ErasedTable, Table};

    pub const fn table<M>(
        name: &'static str,
        fields: &'static [(&'static str, &'static str)],
    ) -> Table<M> {
        Table {
            table: ErasedTable { name, fields },
            phantom: PhantomData,
        }
    }

    pub const fn column_def(table_name: &'static str, name: &'static str) -> ColumnDef {
        ColumnDef { table_name, name }
    }
}
