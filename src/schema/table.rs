use std::marker::PhantomData;

use crate::{queries::Query, WhereCondition};

use super::model::Model;

pub struct Table<M> {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [(&'static str, &'static str)],
    pub(crate) phantom: PhantomData<M>,
}

impl<M: Model> Table<M> {
    pub fn _where(self, condition: WhereCondition<M>) -> Query<M> {
        todo!();
    }

    // pub fn _as(self, name: &'static str) -> Self {
    //     todo!();
    // }
}
