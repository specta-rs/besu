use std::marker::PhantomData;

use crate::{Driver, Sql};

// TODO: Break this out
pub struct SelectOperation<D: Driver> {
    // TODO
    phantom: PhantomData<D>,
}

pub trait Dialect {
    // TODO: Rename
    fn to_query<D: Driver, T>(sql: Sql<D, T>) -> (String, Vec<D::Value>);

    // fn select<D: Driver>(&self, operation: SelectOperation<D>) -> Sql<D, ()>;
}
