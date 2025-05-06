use std::marker::PhantomData;

use crate::{
    sql::{Inner, Inner2},
    Sql,
};

#[derive(Clone, Copy)] // TODO: Debug
pub struct Column<T> {
    name: &'static str,
    phantom: std::marker::PhantomData<T>,
}

impl<T> Column<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            phantom: PhantomData,
        }
    }
}

// TODO: `FromSql` bound good or not?
// impl<T: FromSql> IntoSql for Column<T> {
//     type Type = T;

//     fn into_sql(self) -> Sql<Self::Type> {
//         Sql {
//             parts: Inner2::Owned(vec![Inner::Literal(self.name)]),
//             marker: std::marker::PhantomData,
//         }
//     }
// }
