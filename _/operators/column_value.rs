use crate::{schema::Column, SqlValue};

use super::where_condition::Condition;

pub trait ColumnValue {
    // TODO: This being a GAT makes it hard to constrain in userspace code. I know removing it is hard but it *should* be done.
    type Model<M>;
    type Type;

    fn into_condition(self) -> Condition;
}

// TODO: This is a conflicting impl
// impl<T: ColumnValue> ColumnValue for &T {
//     type Model<M> = T::Model<M>;
//     type Type = T::Type;

//     fn into_condition(self) -> Condition {
//         T::into_condition(*self)
//     }
// }

impl ColumnValue for i32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // SAFETY: Cast to bigger int
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl ColumnValue for u32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // TODO: I don't think this is safe for really large numbers
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl ColumnValue for i64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Integer(self))
    }
}

impl ColumnValue for u64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // TODO: I don't think this is safe for really large numbers
        Condition::Value(SqlValue::Integer(self as i64))
    }
}

impl ColumnValue for f32 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        // SAFETY: Cast to bigger float
        Condition::Value(SqlValue::Float(self as f64))
    }
}

impl ColumnValue for f64 {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Float(self))
    }
}

impl ColumnValue for String {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::String(self))
    }
}

impl ColumnValue for bool {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Boolean(self))
    }
}

impl ColumnValue for Vec<u8> {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Binary(self))
    }
}

#[cfg(feature = "uuid")]
impl ColumnValue for uuid::Uuid {
    type Model<M> = M;
    type Type = Self;

    fn into_condition(self) -> Condition {
        Condition::Value(SqlValue::Binary(self.as_bytes().to_vec()))
    }
}

impl<C: Column> ColumnValue for C {
    type Model<MM> = C::Model;
    type Type = C::Type;

    fn into_condition(self) -> Condition {
        let def = C::def();
        Condition::Column {
            table: def.table_name,
            column: def.name,
        }
    }
}
