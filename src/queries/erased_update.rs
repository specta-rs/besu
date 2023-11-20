use crate::{
    operators::{Condition, ErasedWhereCondition},
    schema::TableDef,
    SqlValue,
};

pub(crate) struct SetColumn {
    pub(crate) column_name: &'static str,
    pub(crate) value: SqlValue,
}

// `Update` is generic on the model and we wanna avoid monomorphization (especially on the `to_sql` function).
pub(crate) struct ErasedUpdate {
    pub(crate) table_name: &'static str,
    pub(crate) columns: Vec<SetColumn>,
    pub(crate) _where: Vec<ErasedWhereCondition>,
}

impl ErasedUpdate {
    pub fn new(from: TableDef) -> Self {
        Self {
            table_name: from.name,
            columns: Vec::new(),
            _where: Vec::new(),
        }
    }

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), ()> {
        let mut sql = format!("UPDATE ");
        sql.push_str(self.table_name);
        sql.push_str(" SET ");

        for (i, column) in self.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(column.column_name);
            sql.push_str(" = ");
            sql.push_str(&column.value.to_sql());
        }

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
