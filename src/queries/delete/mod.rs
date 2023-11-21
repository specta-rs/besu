use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{operators::WhereCondition, schema::Table, Database, Error, SqlValue};

mod builder;

use builder::Builder;

pub struct DeleteQuery<M> {
    db: Database,
    query: Builder,
    phantom: PhantomData<M>,
}

impl<M> DeleteQuery<M> {
    pub(crate) fn new<T: Table<Model = M>>(db: Database, table: fn() -> T) -> Self {
        let _ = table;
        Self {
            db,
            query: Builder::new(T::def()),
            phantom: PhantomData,
        }
    }

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        self.query._where.push(condition.condition);
        self
    }

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), Error> {
        self.query.to_sql()
    }
}

impl<M: 'static> IntoFuture for DeleteQuery<M> {
    type Output = Result<(), Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (sql, values) = self.to_sql()?;
            self.db.execute(sql, values).await.map(|_| ())
        })
    }
}
