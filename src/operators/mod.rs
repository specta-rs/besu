use std::marker::PhantomData;

use crate::{
    internal::{Contains, TyEq},
    schema::Column,
};

pub struct WhereCondition<M> {
    phantom: PhantomData<M>,
}

// TODO: Seal this trait
pub trait ColumnValue {
    type Model<M>; // TODO: Avoiding this GAT would be nice but it makes `Contains` fucked.
    type Type;
}

impl ColumnValue for i32 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for u32 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for i64 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for u64 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for f32 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for f64 {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for String {
    type Model<M> = M;
    type Type = Self;
}
impl ColumnValue for bool {
    type Model<M> = M;
    type Type = Self;
}

impl<M, T> ColumnValue for Column<M, T> {
    type Model<MM> = M;
    type Type = T;
}

pub fn eq<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way. // TODO: I suspect this is breaking inference so tradeoffs lol
    (A::Type, B::Type): TyEq,
{
    todo!();
}

// pub fn eq<T, M>(a: impl ColumnValue, b: impl ColumnValue) -> WhereCondition<M> {
//     todo!();
// }

// pub fn eq<A, B>(column: A, value: B) -> WhereCondition<A, B> {
//     todo!();
// }

// TODO: Enforce only integer types
// pub fn gt<A, B>(column: ) -> WhereCondition<A, B> {
//     todo!();
// }

// TODO: Enforce only integer types
// pub fn lt<M, T>(column: Column<M, T>, value: T) -> WhereCondition<M> {
//     todo!();
// }

// pub fn _in<M, T>(column: Column<M, T>, value: Vec<T>) -> WhereCondition<M> {
//     todo!();
// }

// TODO: Allow custom where parameters using unchecked raw SQL -> Allow these to be defined publically!
