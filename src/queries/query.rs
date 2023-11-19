use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{
    database::Connection,
    operators::{Condition, ErasedWhereCondition, WhereCondition},
    schema::{Table, TableDef},
    SqlValue,
};

pub struct Query<M> {
    db: Connection,
    // We can't use `M` as joins will change it.
    from: TableDef,
    _where: Option<Vec<ErasedWhereCondition>>,
    phantom: PhantomData<M>,
}

impl<M> Query<M> {
    pub(crate) fn new<T: Table<Model = M>>(db: Connection, table: fn() -> T) -> Self {
        Self {
            db,
            from: T::def(),
            _where: None,
            phantom: PhantomData,
        }
    }

    pub fn _where(mut self, condition: WhereCondition<M>) -> Self {
        match &mut self._where {
            Some(conditions) => conditions.push(condition.condition),
            None => self._where = Some(vec![condition.condition]),
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

    // TODO: Where condition type - `M2` is not correct
    pub fn left_join<T: Table>(
        self,
        table: T,
        condition: WhereCondition<(M, Option<T::Model>)>,
    ) -> Query<(M, Option<T::Model>)> {
        todo!();
    }

    // TODO: Right Join
    // TODO: Inner Join

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), ()> {
        let mut sql = "SELECT ".to_string();
        for (i, (column_name, _)) in self.from.fields.iter().enumerate() {
            sql.push_str(column_name);
            if i != self.from.fields.len() - 1 {
                sql.push_str(", ");
            }
        }
        sql.push_str(" FROM ");
        sql.push_str(self.from.name);

        for condition in self._where.iter().flatten() {
            sql.push_str(" WHERE ");
            match condition {
                ErasedWhereCondition::Raw(raw) => sql.push_str(&raw),
                ErasedWhereCondition::Condition { a, operator, b } => {
                    match a {
                        Condition::Value(value) => {
                            sql.push_str(&value.to_sql());
                        }
                        Condition::Column { table, column } => {
                            sql.push_str(table);
                            sql.push('.');
                            sql.push_str(column);
                        }
                    }
                    sql.push(' ');
                    sql.push_str(operator);
                    sql.push(' ');
                    match b {
                        Condition::Value(value) => {
                            sql.push_str(&value.to_sql());
                        }
                        Condition::Column { table, column } => {
                            sql.push_str(table);
                            sql.push('.');
                            sql.push_str(column);
                        }
                    }
                }
            }
        }

        Ok((sql, vec![]))
    }
}

// This is definetly DX over preformance but `impl Trait` is gonna land soon and regardless I think I could make this no alloc for SQLite at least.
impl<M: 'static> IntoFuture for Query<M> {
    type Output = Result<Vec<M>, ()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (sql, values) = self.to_sql()?;
            self.db.execute(sql.into()).await?;
            Ok(vec![])
        })
    }
}
