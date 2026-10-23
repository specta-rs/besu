use crate::{
    operators::{Condition, ErasedWhereCondition},
    schema::TableDefinition,
    Error, SqlValue,
};

// `DeleteQuery` is generic on the model and we wanna avoid monomorphization (especially on the `to_sql` function).
pub(super) struct Builder {
    pub(crate) table_name: &'static str,
    pub(crate) _where: Vec<ErasedWhereCondition>,
}

impl Builder {
    pub fn new(from: TableDefinition) -> Self {
        Self {
            table_name: from.name,
            _where: Vec::new(),
        }
    }

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), Error> {
        let mut sql = format!("DELETE FROM {}", self.table_name);
        let mut values = Vec::new();

        // TODO: Deduplicate this with the `ErasedQuery` code that is identical.
        let mut where_clause = false;
        for condition in self._where.iter() {
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
                            sql.push_str("?");
                            values.push(value.clone());
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
                            sql.push_str("?");
                            values.push(value.clone());
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

        Ok((sql, values))
    }
}
