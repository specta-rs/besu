use std::{borrow::Cow, collections::HashMap};

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

    pub fn fields_len(&self) -> usize {
        self.from.fields.len()
            + self
                .joins
                .iter()
                .flatten()
                .map(|j| j.table.fields.len())
                .sum::<usize>()
    }

    pub fn to_sql(
        &self,
        mut field_map: Option<&mut Vec<(&'static str, &'static str)>>,
    ) -> Result<(String, Vec<SqlValue>), ()> {
        let mut sql = "SELECT ".to_string();

        let fields = self
            .from
            .fields
            .iter()
            .map(|(col_name, _)| Either::Left((self.from.name, col_name)))
            .chain(
                self.joins
                    .iter()
                    .flatten()
                    .map(|j| {
                        j.table
                            .fields
                            .iter()
                            .map(|(col_name, _)| Either::Left((j.table.name, col_name)))
                    })
                    .flatten(),
            )
            ._intersperse(Either::Right(", "));

        for v in fields {
            match v {
                Either::Left((table_name, column_name)) => {
                    if let Some(fields) = &mut field_map {
                        fields.push((table_name, column_name));
                    }
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
