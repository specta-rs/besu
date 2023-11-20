use crate::{
    operators::{Condition, ErasedWhereCondition},
    schema::TableDef,
    Database, SqlValue,
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
    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), ()> {
        let mut sql = "SELECT ".to_string();
        for (i, (table_name, column_name)) in self
            .from
            .fields
            .iter()
            .map(|(col_name, _)| (self.from.name, col_name))
            .chain(
                self.joins
                    .iter()
                    .flatten()
                    .map(|j| {
                        j.table
                            .fields
                            .iter()
                            .map(|(col_name, _)| (j.table.name, col_name))
                    })
                    .flatten(),
            )
            .enumerate()
        {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(table_name);
            sql.push_str(".");
            sql.push_str(column_name);
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
