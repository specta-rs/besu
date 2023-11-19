use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{
    operators::{Condition, ErasedWhereCondition, WhereCondition},
    schema::{Table, TableDef},
    Database, FromValue, FullJoin, InnerJoin, LeftJoin, RightJoin, SqlValue,
};

pub struct Join {
    table: &'static str,
    condition: ErasedWhereCondition,
}

pub struct Query<M> {
    db: Database,
    // We can't use `M` as joins will change it.
    from: TableDef,
    _where: Option<Vec<ErasedWhereCondition>>,
    joins: Option<Vec<Join>>,
    phantom: PhantomData<M>,
}

impl<M> Query<M> {
    pub(crate) fn new<T: Table<Model = M>>(db: Database, table: fn() -> T) -> Self {
        let _ = table;
        Self {
            db,
            from: T::def(),
            _where: None,
            joins: None,
            phantom: PhantomData,
        }
    }

    fn new_t<T>(self) -> Query<T> {
        Query {
            db: self.db,
            from: self.from,
            _where: self._where,
            joins: self.joins,
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

    pub fn left_join<T: Table>(
        mut self,
        table: fn() -> T,
        condition: WhereCondition<LeftJoin<M, T::Model>>,
    ) -> Query<LeftJoin<M, T::Model>> {
        let _ = table;

        self.joins.get_or_insert_with(Default::default).push(Join {
            table: T::def().name,
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

        for joins in self.joins.iter().flatten() {
            sql.push_str(" LEFT JOIN ");
            sql.push_str(joins.table);
            sql.push_str(" ON ");
            match &joins.condition {
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

        let mut where_clause = false;
        for condition in self._where.iter().flatten() {
            if where_clause {
                sql.push_str(" AND ");
            } else {
                where_clause = true;
                sql.push_str(" WHERE ");
            }

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
