use std::marker::PhantomData;

pub struct Query<M> {
    phantom: PhantomData<M>,
}

impl<M> Query<M> {
    pub(crate) fn new() -> Self {
        Self {
            phantom: PhantomData,
        }
    }

    pub fn _where(mut self, condition: ()) -> Self {
        // TODO
        self
    }
}
