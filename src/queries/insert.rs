use crate::Database;

use super::InsertValue;

pub struct InsertQuery {
    pub(crate) db: Database,
}

impl InsertQuery {
    pub async fn value<V: InsertValue>(self, value: V) -> Result<(), ()> {
        self.values([value]).await
    }

    pub async fn values<V: InsertValue, I: IntoIterator<Item = V>>(
        self,
        values: I,
    ) -> Result<(), ()> {
        let mut values = values.into_iter().peekable();

        let def = V::def();
        let mut sql = format!(
            "INSERT INTO {}({}) VALUES ",
            def.table_name,
            def.columns.join(", "),
        );

        if values.peek().is_none() {
            return Ok(());
        }

        for (i, values) in values.into_iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }

            sql.push_str("(");
            for (i, value) in values.values().into_iter().map(|v| v.to_sql()).enumerate() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&value);
            }
            sql.push_str(")");
        }

        self.db.execute(sql, vec![]).await.map(|_| ())
    }
}
