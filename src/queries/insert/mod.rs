use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{Database, Error, SqlValue};

pub use value::InsertValue;

mod value;

pub struct InsertDefinition {
    pub(crate) table_name: &'static str,
    pub(crate) columns: &'static [&'static str],
}

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
    def: InsertDefinition,
    // TODO: Construct this into a string at the start instead cause less allocs and simplier
    values: Vec<Vec<SqlValue>>,
    db: Database,
    future: Option<Pin<Box<dyn Future<Output = Result<(), Error>>>>>,
}

impl InsertQueryFuture {
    pub fn to_sql(&self) -> Result<(String, Vec<crate::SqlValue>), Error> {
        let mut sql = format!(
            "INSERT INTO {}({}) VALUES ",
            self.def.table_name,
            self.def.columns.join(", "),
        );

        let mut values = self.values.iter().peekable();
        if values.peek().is_none() {
            return Err(Error::ErrValuesEmpty);
        }

        for (i, values) in values.enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }

            sql.push_str("(");
            for i in 0..values.len() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str("?");
            }
            sql.push_str(")");
        }

        Ok((sql, self.values.iter().flatten().cloned().collect()))
    }
}

impl Future for InsertQueryFuture {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Shortcut if there are no values to insert. This will skip the `Error::ErrValuesEmpty`.
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
