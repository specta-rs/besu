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
        queries::InsertDefinition,
        schema::{ColumnDefinition, TableDefinition},
    };

    pub const fn table_def(
        name: &'static str,
        columns: &'static [&'static str],
        column_types: &'static [&'static str],
    ) -> TableDefinition {
        TableDefinition {
            name,
            columns,
            column_types,
        }
    }

    pub const fn column_def(table_name: &'static str, name: &'static str) -> ColumnDefinition {
        ColumnDefinition { table_name, name }
    }

    pub const fn insert_def(
        table_name: &'static str,
        columns: &'static [&'static str],
    ) -> InsertDefinition {
        InsertDefinition {
            table_name,
            columns,
        }
    }
}
