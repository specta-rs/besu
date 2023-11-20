use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{
    operators::WhereCondition, schema::Table, Database, FromValue, FullJoin, InnerJoin, LeftJoin,
    RightJoin, SqlValue,
};

use super::{erased_query::Join, ErasedQuery};

pub struct Query<M> {
    db: Database,
    query: ErasedQuery,
    phantom: PhantomData<M>,
}

impl<M> Query<M> {
    pub(crate) fn new<T: Table<Model = M>>(db: Database, table: fn() -> T) -> Self {
        let _ = table;
        Self {
            db,
            query: ErasedQuery::new(T::def()),
            phantom: PhantomData,
        }
    }

    fn new_t<T>(self) -> Query<T> {
        Query {
            db: self.db,
            query: self.query,
            phantom: PhantomData,
        }
    }

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        match &mut self.query._where {
            Some(conditions) => conditions.push(condition.condition),
            None => self.query._where = Some(vec![condition.condition]),
        }
        self
    }

    // pub fn order(&mut self) {}

    // pub fn limit(&mut self) {}

    // pub fn distinct(mut self) -> Self {
    //     // TODO
    //     self
    // }

    // TODO: Cursor pagination

    pub fn left_join<T: Table>(
        mut self,
        table: fn() -> T,
        condition: WhereCondition<LeftJoin<M, T::Model>>,
    ) -> Query<LeftJoin<M, T::Model>> {
        let _ = table;

        self.query
            .joins
            .get_or_insert_with(Default::default)
            .push(Join {
                table: T::def(),
                condition: condition.condition,
            });

        self.new_t()
    }

    pub fn right_join<T: Table>(
        self,
        table: fn() -> T,
        condition: WhereCondition<RightJoin<M, T::Model>>,
    ) -> Query<RightJoin<M, T::Model>> {
        let _ = table;
        todo!();
    }

    pub fn inner_join<T: Table>(
        self,
        table: fn() -> T,
        condition: WhereCondition<InnerJoin<M, T::Model>>,
    ) -> Query<InnerJoin<M, T::Model>> {
        let _ = table;
        todo!();
    }

    pub fn full_join<T: Table>(
        self,
        table: T,
        condition: WhereCondition<FullJoin<M, T::Model>>,
    ) -> Query<FullJoin<M, T::Model>> {
        let _ = table;
        todo!();
    }

    // TODO: Cross join

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), ()> {
        self.query.to_sql()
    }
}

// This is definitely DX over performance but `impl Trait` is gonna land soon and regardless I think I could make this no alloc for SQLite at least.
impl<M: FromValue + 'static> IntoFuture for Query<M> {
    type Output = Result<Vec<M>, ()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (sql, values) = self.to_sql()?;
            let values = self
                .db
                .execute(sql, values)
                .await?
                .expect("no return value. Skill issue!");

            Ok(values
                .into_iter()
                .map(|v| M::from_value(v).unwrap())
                .collect())
        })
    }
}

// This is definitely DX over performance but `impl Trait` is gonna land soon and regardless I think I could make this no alloc for SQLite at least.
impl<T: FromValue + 'static, U: FromValue + 'static> IntoFuture for Query<LeftJoin<T, U>> {
    type Output = Result<Vec<LeftJoin<T, U>>, ()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (sql, values) = self.to_sql()?;
            let values = self
                .db
                .execute(sql, values)
                .await?
                .expect("no return value. Skill issue!");

            println!("{values:?}");

            // Ok(values
            //     .into_iter()
            //     .map(|v| M::from_value(v).unwrap())
            //     .collect())
            todo!();
        })
    }
}
