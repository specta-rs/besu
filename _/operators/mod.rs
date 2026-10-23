use std::borrow::Cow;

use crate::internal::{Contains, TyEq};

mod column_value;
mod order;
mod where_condition;

pub use column_value::ColumnValue;
pub use order::{asc, desc, OrderClause};
pub use where_condition::WhereCondition;
pub(crate) use where_condition::{Condition, ErasedWhereCondition};

/// A custom SQL where condition. This is great for when you need to do something that can't be expressed Besu's syntax.
///
/// NOTE: This method is unchecked. If you typo it, you will get a runtime error.
///
/// # Example
/// ```rust
/// // TODO
/// ```
// TODO: How can custom SQLValue's be escaped into this
pub fn sql<S: Into<Cow<'static, str>>, M>(sql: S) -> WhereCondition<M> {
    WhereCondition::new(ErasedWhereCondition::Raw(sql.into()))
}

/// Ensure two values are equal.
///
/// # Example
/// ```rust
/// // TODO
/// ```
pub fn eq<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: "=",
        b: b.into_condition(),
    })
}

// TODO: Enforce only integer types
pub fn gt<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: ">",
        b: b.into_condition(),
    })
}

pub fn gte<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: ">=",
        b: b.into_condition(),
    })
}

// TODO: Enforce only integer types
pub fn lt<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: "<",
        b: b.into_condition(),
    })
}

pub fn lte<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: "<=",
        b: b.into_condition(),
    })
}

pub fn like<P, P2, A: ColumnValue, B: ColumnValue, M>(a: A, b: B) -> WhereCondition<M>
where
    (M,): Contains<P, A::Model<M>>,
    (M,): Contains<P2, B::Model<M>>,
    // This could technically be `B: ColumValue<Type = A::Type>` but the errors are *much* better this way
    (A::Type, B::Type): TyEq,
{
    WhereCondition::new(ErasedWhereCondition::Condition {
        a: a.into_condition(),
        operator: "LIKE",
        b: b.into_condition(),
    })
}

// pub fn _in<M, T>(column: Column<M, T>, value: Vec<T>) -> WhereCondition<M> {
//     todo!();
// }

// TODO: Allow custom where parameters using unchecked raw SQL -> Allow these to be defined publically!
