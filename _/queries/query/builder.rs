use crate::{
    internal::{Either, IntersperseIter},
    operators::{Condition, ErasedWhereCondition, OrderClause},
    schema::TableDefinition,
    Error, SqlValue,
};

pub(crate) struct Join {
    pub(crate) table: TableDefinition,
    pub(crate) condition: ErasedWhereCondition,
}

// `Query` is generic on the model and we wanna avoid monomorphization (especially on the `to_sql` function).
pub(super) struct Builder {
    pub(crate) from: TableDefinition,
    pub(crate) _where: Option<Vec<ErasedWhereCondition>>,
    pub(crate) joins: Option<Vec<Join>>,
    pub(crate) order_by: Option<Vec<OrderClause>>,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: Option<usize>,
    pub(crate) distinct: bool,
}

impl Builder {
    pub fn new(from: TableDefinition) -> Self {
        Self {
            from,
            _where: None,
            joins: None,
            order_by: None,
            limit: None,
            offset: None,
            distinct: false,
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

    pub fn to_sql(&self) -> Result<(String, Vec<SqlValue>), Error> {
        let mut sql = "SELECT ".to_string();
        let mut values = Vec::new();

        if self.distinct {
            sql.push_str("DISTINCT ");
        }

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
                            values.push(value.clone());
                            sql.push_str("?"); // TODO: Get this from the DB
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
                            values.push(value.clone());
                            sql.push_str("?"); // TODO: Get this from the DB
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
                            values.push(value.clone());
                            sql.push_str("?"); // TODO: Get this from the DB
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
                            values.push(value.clone());
                            sql.push_str("?"); // TODO: Get this from the DB
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

        for clause in self.order_by.iter().flatten() {
            sql.push_str(clause.table_name);
            sql.push('.');
            sql.push_str(clause.column_name);
            if let Some(suffix) = clause.suffix {
                sql.push(' ');
                sql.push_str(suffix);
            }
        }

        if let Some(limit) = self.limit {
            sql.push_str(" LIMIT ");
            sql.push_str(&limit.to_string());
        }

        if let Some(offset) = self.offset {
            sql.push_str(" OFFSET ");
            sql.push_str(&offset.to_string());
        }

        Ok((sql, values))
    }
}
