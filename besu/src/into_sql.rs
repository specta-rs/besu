use crate::{value, Sql};

pub trait IntoSql<To, M> {
    fn into_sql(self) -> Sql<To>;
}

pub enum FromSqlTToU {}
// impl<T, U: From<T>> IntoSql<U, FromSqlTToU> for Sql<T> {
//     fn into_sql(self) -> Sql<U> {
//         todo!();
//     }
// }

pub enum FromTForU {}
impl<T, U: From<T>> IntoSql<U, FromTForU> for T {
    fn into_sql(self) -> Sql<U> {
        value(self.into())
    }
}

pub enum NeverIntoT {}
// impl<T> IntoSql<T, NeverIntoT> for Sql<Never> {
//     fn into_sql(self) -> Sql<T> {
//         todo!();
//     }
// }
