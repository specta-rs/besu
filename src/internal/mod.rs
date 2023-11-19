mod contains;
mod equals;

// pub(crate) use contains::Contains;
pub(crate) use equals::TyEq;

// TODO: Remove
pub use contains::*;

pub mod construct {
    use std::marker::PhantomData;

    use crate::schema::{Column, Table};

    pub const fn table<M>(
        name: &'static str,
        fields: &'static [(&'static str, &'static str)],
    ) -> Table<M> {
        Table {
            name,
            fields,
            phantom: PhantomData,
        }
    }

    pub const fn column<M, T>(name: &'static str) -> Column<M, T> {
        Column {
            name,
            phantom: core::marker::PhantomData,
        }
    }
}
