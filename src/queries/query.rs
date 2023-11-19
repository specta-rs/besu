use std::marker::PhantomData;

use crate::WhereCondition;

pub struct Query<M> {
    db: (),
    phantom: PhantomData<M>,
}

impl<M> Query<M> {
    pub(crate) fn new(db: ()) -> Self {
        Self {
            db,
            phantom: PhantomData,
        }
    }

    // TODO: Joins?
}

pub struct QueryBuilder<M> {
    phantom: PhantomData<M>,
}

impl<M> QueryBuilder<M> {
    pub(crate) fn new() -> Self {
        Self {
            phantom: PhantomData,
        }
    }

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        // TODO
        self
    }

    // TODO: Joins?
}
