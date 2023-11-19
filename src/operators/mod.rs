use std::marker::PhantomData;

use crate::schema::Column;

pub struct WhereCondition<M> {
    phantom: PhantomData<M>,
}

pub fn eq<M, T>(column: Column<M, T>, value: T) -> WhereCondition<M> {
    todo!();
}

// TODO: Enforce only integer types
pub fn gt<M, T>(column: Column<M, T>, value: T) -> WhereCondition<M> {
    todo!();
}

// TODO: Enforce only integer types
pub fn lt<M, T>(column: Column<M, T>, value: T) -> WhereCondition<M> {
    todo!();
}

pub fn _in<M, T>(column: Column<M, T>, value: Vec<T>) -> WhereCondition<M> {
    todo!();
}
