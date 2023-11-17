pub mod construct {
    use crate::schema::{Column, Table};

    pub const fn table(
        name: &'static str,
        fields: &'static [(&'static str, &'static str)],
    ) -> Table {
        Table { name, fields }
    }

    pub const fn column<T>(name: &'static str) -> Column<T> {
        Column {
            name,
            phantom: core::marker::PhantomData,
        }
    }
}
