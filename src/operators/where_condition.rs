use std::{borrow::Cow, marker::PhantomData};

use crate::SqlValue;

pub(crate) enum ErasedWhereCondition {
    Raw(Cow<'static, str>),
    Condition {
        a: Condition,
        operator: &'static str,
        b: Condition,
    },
}

pub(crate) enum Condition {
    Value(SqlValue),
    Column {
        table: &'static str,
        column: &'static str,
    },
}

pub struct WhereCondition<M> {
    pub(crate) condition: ErasedWhereCondition,
    phantom: PhantomData<M>,
}

impl<M> WhereCondition<M> {
    pub(crate) fn new(condition: ErasedWhereCondition) -> Self {
        Self {
            condition,
            phantom: PhantomData,
        }
    }

    // pub(crate) fn to_sql_part() {}
}
