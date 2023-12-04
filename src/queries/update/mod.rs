use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{
    operators::WhereCondition,
    schema::{Column, Table},
    Database, Error, ExecutedQuery, SqlValue,
};

use builder::{Builder, SetColumn};

mod builder;

pub struct UpdateQuery<M> {
    db: Database,
    query: Builder,
    phantom: PhantomData<M>,
}

impl<M> UpdateQuery<M> {
    pub(crate) fn new<T: Table<Model = M>>(db: Database, table: fn() -> T) -> Self {
        let _ = table;
        Self {
            db,
            query: Builder::new(T::def()),
            phantom: PhantomData,
        }
    }

    pub fn set<C: Column<Model = M>, T: Into<C::Type>>(mut self, field: C, value: T) -> Self {
        let _ = field;
        let def = C::def();
        let t: C::Type = value.into();
        self.query.columns.push(SetColumn {
            column_name: def.name,
            value: t.into(),
        });
        self
    }

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        self.query._where.push(condition.condition);
        self
    }

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), Error> {
        self.query.to_sql()
    }
}

impl<M: 'static> IntoFuture for UpdateQuery<M> {
    type Output = Result<ExecutedQuery, Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (sql, values) = self.to_sql()?;
            self.db.execute(sql, values).await
        })
    }
}
