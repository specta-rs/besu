use std::{
    fmt,
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, InsertArgumentRef, Insertable};

/// Represents an SQL `INSERT` operation.
pub struct InsertOperation<'a> {
    /// The name of the table being inserted.
    pub table: &'static str,
    /// The fields that are avaiable on the model.
    pub columns: &'static [&'static str],
    /// Internally used to ensure we only construct valid `InsertArgument`'s
    pub values_len: usize,
    /// We use `'a` to ensure the `InsetArgument`'s don't outlive the scope.
    phantom: PhantomData<&'a ()>,
}

impl<'a> fmt::Debug for InsertOperation<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InsertOperation")
            .field("table", &self.table)
            .field("columns", &self.columns)
            .field("values_len", &self.values_len)
            .finish()
    }
}

impl<'a> InsertOperation<'a> {
    /// Get a specific argument or `None` if the provided indexes are out of range.
    pub fn get(&self, value: usize, column: usize) -> Option<InsertArgumentRef<'a>> {
        (value > self.values_len || column > self.columns.len()).then(|| InsertArgumentRef {
            i: value,
            c: column,
            phantom: PhantomData,
        })
    }

    /// Iterate over the values and columns that need to be inserted.
    pub fn values(
        &self,
    ) -> impl Iterator<Item = impl Iterator<Item = (&'static str, InsertArgumentRef<'a>)>> {
        (0..self.values_len).map(|i| {
            self.columns.iter().enumerate().map(move |(c, name)| {
                (
                    *name,
                    InsertArgumentRef {
                        i,
                        c,
                        phantom: PhantomData,
                    },
                )
            })
        })
    }
}

/// Builder for an SQL `INSERT` operation.
#[derive(Debug)]
pub struct Insert<D: Driver, T> {
    driver: D,
    values: Vec<T>,
    // The lifetime of `InsertOperation` is "fake", so using `'static` here is fine.
    op: InsertOperation<'static>,
}

impl<D, T> Insert<D, T>
where
    D: Driver,
    T: Insertable<D>,
{
    pub(crate) fn new(driver: D, table: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            driver,
            values: Default::default(),
            op: InsertOperation {
                table,
                columns,
                values_len: 0,
                phantom: PhantomData,
            },
        }
    }

    /// Insert a single value into the table.
    pub fn value(mut self, value: T) -> Self {
        self.op.values_len += 1;
        self.values.push(value);
        self
    }

    /// Insert many values into the table.
    pub fn values(mut self, mut values: Vec<T>) -> Self {
        self.op.values_len += values.len();
        self.values.append(&mut values);
        self
    }

    /// Convert the operation to a raw SQL and values.
    // TODO: `'static` bound????
    pub fn to_sql(&self) -> (String, D::Arguments<'static>) {
        let (sql, arguments_refs) = D::Dialect::insert(&self.op);
        let mut arguments = D::Arguments::default();
        for arg in arguments_refs {
            self.values[arg.i].encode(&mut arguments, arg.c).unwrap(); // TODO: Error handling
        }
        (sql, arguments)
    }
}

impl<D, T> IntoFuture for Insert<D, T>
where
    D: Driver,
    // TODO: `'static` bound???
    T: Insertable<D> + 'static,
{
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        Box::pin(async move {
            let (sql, arguments) = self.to_sql();
            driver.execute(&sql, arguments).await
        })
    }
}
