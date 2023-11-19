use std::marker::PhantomData;

pub(crate) struct ErasedTable {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [(&'static str, &'static str)],
}

pub struct Table<M> {
    pub(crate) table: ErasedTable,
    pub(crate) phantom: PhantomData<M>,
}
