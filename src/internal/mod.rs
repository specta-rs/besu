mod contains;
mod either;
mod equals;
mod intersperse;

pub(crate) use contains::Contains;
pub(crate) use either::Either;
pub(crate) use equals::TyEq;
pub(crate) use intersperse::IntersperseIter;

pub mod construct {
    use crate::{
        queries::InsertDef,
        schema::{ColumnDef, TableDef},
    };

    pub const fn table_def(
        name: &'static str,
        columns: &'static [&'static str],
        column_types: &'static [&'static str],
    ) -> TableDef {
        TableDef {
            name,
            columns,
            column_types,
        }
    }

    pub const fn column_def(table_name: &'static str, name: &'static str) -> ColumnDef {
        ColumnDef { table_name, name }
    }

    pub const fn insert_def(
        table_name: &'static str,
        columns: &'static [&'static str],
    ) -> InsertDef {
        InsertDef {
            table_name,
            columns,
        }
    }
}
