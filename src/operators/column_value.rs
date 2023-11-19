use crate::{schema::Column, SqlValue};

use super::where_condition::Condition;

pub trait ColumnValue: SealedColumnValue {}

pub(crate) trait SealedColumnValue {
    type Model<M>;
    type Type;

    fn into_condition(self) -> Condition;
}

impl<T: SealedColumnValue> ColumnValue for T {}

impl SealedColumnValue for i32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // SAFETY: Cast to bigger int
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl SealedColumnValue for u32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // TODO: I don't think this is safe for really large numbers
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl SealedColumnValue for i64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Integer(self))
    }
}

impl SealedColumnValue for u64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // TODO: I don't think this is safe for really large numbers
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl SealedColumnValue for f32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // SAFETY: Cast to bigger float
        Condition::Value(SqlValue::Float(self as f64))
    }
}

impl SealedColumnValue for f64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Float(self))
    }
}

impl SealedColumnValue for String {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Text(self))
    }
}

impl SealedColumnValue for bool {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Bool(self))
    }
}

impl<M, T> SealedColumnValue for Column<M, T> {
    type Model<MM> = M;
    type Type = T;

    fn into_condition(self) -> Condition {
        Condition::Column {
            table: self.table_name,
            column: self.name,
        }
    }
}
