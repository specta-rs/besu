use std::marker::PhantomData;

use super::Model;

pub struct Column<M, T> {
    pub(crate) table_name: &'static str,
    pub(crate) name: &'static str,
    pub(crate) phantom: PhantomData<(M, T)>,
}

impl<M: Model, T> Column<M, T> {}
