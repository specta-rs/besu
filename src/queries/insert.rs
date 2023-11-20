use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{Database, SqlValue};

use super::{InsertDef, InsertValue};

pub struct InsertQuery {
    pub(crate) db: Database,
}

impl InsertQuery {
    pub fn value<V: InsertValue>(self, value: V) -> InsertQueryFuture {
        self.values([value])
    }

    pub fn values<V: InsertValue, I: IntoIterator<Item = V>>(self, values: I) -> InsertQueryFuture {
        let values = values.into_iter().map(|v| v.values()).collect::<Vec<_>>();
        InsertQueryFuture {
            def: V::def(),
            values,
            db: self.db,
            future: None,
        }
    }
}

pub struct InsertQueryFuture {
    def: InsertDef,
    // TODO: Construct this into a string at the start instead cause less allocs and simplier
    values: Vec<Vec<SqlValue>>,
    db: Database,
    future: Option<Pin<Box<dyn Future<Output = Result<(), ()>>>>>,
}

impl InsertQueryFuture {
    pub fn to_sql(&self) -> Result<(String, Vec<crate::SqlValue>), ()> {
        let mut sql = format!(
            "INSERT INTO {}({}) VALUES ",
            self.def.table_name,
            self.def.columns.join(", "),
        );

        let mut values = self.values.iter().peekable();
        if values.peek().is_none() {
            return Err(()); // TODO: What does drizzle do?
        }

        for (i, values) in values.enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }

            sql.push_str("(");
            for (i, value) in values.into_iter().map(|v| v.to_sql()).enumerate() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&value);
            }
            sql.push_str(")");
        }

        Ok((sql, Vec::new()))
    }
}

impl Future for InsertQueryFuture {
    type Output = Result<(), ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Shortcut if there are no values to insert
        if self.values.is_empty() {
            return Poll::Ready(Ok(()));
        }

        loop {
            match self.future {
                Some(ref mut f) => return f.as_mut().poll(cx),
                None => {
                    let result = self.to_sql();
                    let db = self.db.clone();
                    self.future = Some(Box::pin(async move {
                        let (sql, values) = result?;
                        db.execute(sql, values).await.map(|_| ())
                    }));
                }
            }
        }
    }
}
