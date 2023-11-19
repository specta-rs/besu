use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{schema::Table, WhereCondition};

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

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        // TODO
        self
    }

    // pub fn order(&mut self) {}

    // pub fn limit(&mut self) {}

    // pub fn distinct(mut self) -> Self {
    //     // TODO
    //     self
    // }

    // TODO: Where condition type - `M2` is not correct
    pub fn left_join<M2>(
        self,
        table: Table<M2>,
        condition: WhereCondition<(M, M2)>,
    ) -> Query<(M, M2)> {
        todo!();
    }

    // TODO: Joins?
}

impl<M> Future for Query<M> {
    type Output = Result<Vec<M>, ()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}
