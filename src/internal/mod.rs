pub mod construct {
    use crate::schema::{Column, TableDef};

    pub const fn table_def(
        name: &'static str,
        fields: &'static [(&'static str, &'static str)],
    ) -> TableDef {
        TableDef { name, fields }
    }

    pub const fn column<T>(name: &'static str) -> Column<T> {
        Column {
            name,
            phantom: core::marker::PhantomData,
        }
    }
}
