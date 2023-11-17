use std::{future::Future, marker::PhantomData};

pub struct UpdateQuery<M> {
    pub(crate) phantom: PhantomData<M>,
}

impl<M> UpdateQuery<M> {
    // TODO
    // pub fn edit(&mut self) -> UpdateQueryMut {
    //     UpdateQueryMut(self)
    // }

    // TODO: Fix up types
    pub fn set(self, field: (), value: impl std::fmt::Debug) -> Self {
        // TODO

        self
    }

    // TODO: `WHERE` clauses
}

impl<M> Future for UpdateQuery<M> {
    type Output = Result<(), ()>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        todo!()
    }
}
