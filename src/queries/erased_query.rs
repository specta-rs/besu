use std::ops::Range;

use crate::{
    internal::{Either, IntersperseIter},
    operators::{Condition, ErasedWhereCondition},
    schema::TableDef,
    SqlValue,
};

pub(crate) struct Join {
    pub(crate) table: TableDef,
    pub(crate) condition: ErasedWhereCondition,
}

// `Query` is generic on the model and we wanna avoid monomorphisation (especially on the `to_sql` function).
pub(crate) struct ErasedQuery {
    pub(crate) from: TableDef,
    pub(crate) _where: Option<Vec<ErasedWhereCondition>>,
    pub(crate) joins: Option<Vec<Join>>,
}

impl ErasedQuery {
    pub fn new(from: TableDef) -> Self {
        Self {
            from,
            _where: None,
            joins: None,
        }
    }

    // TODO: Can be avoid the inner `Vec`'s by using lifetimes instead?
    // TOOD: `.drain` requires the `Vec` to reshuffle. Surley we can avoid that???
    pub fn decode_values<'a>(
        &'a self,
        values: &'a mut Vec<SqlValue>,
    ) -> impl Iterator<Item = Vec<SqlValue>> + 'a {
        let y = values.drain(0..self.from.columns.len()).collect::<Vec<_>>();
        [y].into_iter().chain(
            self.joins
                .iter()
                .flatten()
                .map(|j| values.drain(0..j.table.columns.len()).collect::<Vec<_>>()),
        )
    }

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), ()> {
        let mut sql = "SELECT ".to_string();

        let fields = self
            .from
            .columns
            .iter()
            .map(|col_name| Either::Left((self.from.name, col_name)))
            .chain(
                self.joins
                    .iter()
                    .flatten()
                    .map(|j| {
                        j.table
                            .columns
                            .iter()
                            .map(|col_name| Either::Left((j.table.name, col_name)))
                    })
                    .flatten(),
            )
            ._intersperse(Either::Right(", "));

        for v in fields {
            match v {
                Either::Left((table_name, column_name)) => {
                    sql.push_str(table_name);
                    sql.push('.');
                    sql.push_str(column_name);
                }
                Either::Right(value) => {
                    sql.push_str(value);
                }
            }
        }

        sql.push_str(" FROM ");
        sql.push_str(self.from.name);

        for joins in self.joins.iter().flatten() {
            sql.push_str(" LEFT JOIN ");
            sql.push_str(joins.table.name);
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
