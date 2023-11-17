use std::marker::PhantomData;

pub struct Column<T> {
    pub(crate) name: &'static str,
    pub(crate) phantom: PhantomData<T>,
}

impl<T> Column<T> {}
