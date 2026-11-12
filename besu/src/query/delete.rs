use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor, TableDef};

use crate::expr::{ExprKind, IntoCondition};
use crate::query::{push_table, update::render_where};
use crate::selection::Selection;
use crate::table::Table;

#[derive(Debug, Clone)]
pub(crate) struct DeleteQuery {
    pub(crate) table: &'static TableDef,
    pub(crate) filter: Vec<ExprKind>,
    pub(crate) returning: Option<Vec<ExprKind>>,
    pub(crate) limit: Option<i64>,
    /// An application-level label, rendered as a trailing `/* ... */`.
    pub(crate) comment: Option<alloc::string::String>,
}

impl DeleteQuery {
    pub(crate) fn new(table: &'static TableDef) -> Self {
        Self {
            table,
            filter: Vec::new(),
            returning: None,
            limit: None,
            comment: None,
        }
    }

    pub(crate) fn build<D: besu_core::Dialect>(
        mut self,
        prefix: Option<alloc::sync::Arc<str>>,
    ) -> besu_core::Sql<D> {
        let mut out = besu_core::Sql::new();
        out.set_table_prefix(prefix);

        out.push_raw("DELETE FROM ");
        push_table(&mut out, self.table);
        render_where(&mut out, &mut self.filter);

        if let Some(limit) = self.limit {
            if !D::SUPPORTS_DELETE_LIMIT {
                out.set_error(Error::unsupported(alloc::format!(
                    "`{}` has no `DELETE ... LIMIT`; filter down to the rows instead",
                    D::NAME
                )));
                return out;
            }
            out.push_raw(" LIMIT ");
            out.bind(limit);
        }

        if let Some(returning) = &mut self.returning {
            if !D::SUPPORTS_RETURNING {
                out.set_error(Error::unsupported(alloc::format!(
                    "`{}` does not support `RETURNING`",
                    D::NAME
                )));
                return out;
            }
            out.push_raw(" RETURNING ");
            for (i, expr) in returning.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                expr.render(&mut out);
            }
        }

        let mut out = out.seal();
        crate::query::render_comment(&mut out, self.comment.as_deref());
        out
    }
}

/// A `DELETE` under construction.
///
/// The `SAFE` const parameter makes an accidental `DELETE FROM users` impossible: without a
/// `WHERE` clause the type does not implement [`IntoFuture`], so `.await` does not compile.
pub struct Delete<'a, E: Executor, T, const SAFE: bool = false> {
    pub(crate) exec: E,
    pub(crate) query: DeleteQuery,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> T)>,
}

impl<'a, E: Executor, T> Delete<'a, E, T, false> {
    pub(crate) fn new(exec: E, table: &'static TableDef) -> Self {
        Self {
            exec,
            query: DeleteQuery::new(table),
            phantom: PhantomData,
        }
    }

    /// Add a `WHERE` condition, which makes the statement safe to run.
    pub fn filter<F, P>(self, condition: F) -> Delete<'a, E, T, true>
    where
        T: Table,
        F: IntoCondition<(T,), P>,
    {
        let _ = PhantomData::<P>;
        let mut query = self.query;
        query.filter.push(condition.into_condition());
        Delete {
            exec: self.exec,
            query,
            phantom: PhantomData,
        }
    }

    /// Explicitly opt into deleting every row in the table.
    pub fn all_rows(self) -> Delete<'a, E, T, true> {
        Delete {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }
}

impl<'a, E: Executor, T, const SAFE: bool> Delete<'a, E, T, SAFE> {
    /// Limit how many rows are deleted. MySQL and SQLite support this; PostgreSQL does not.
    pub fn limit(mut self, limit: i64) -> Self {
        self.query.limit = Some(limit);
        self
    }

    /// Render the exact SQL this query will run.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.clone().build(self.exec.table_prefix()))
    }

    /// Label the statement with a comment, rendered as a trailing `/* ... */`.
    ///
    /// See [`Select::comment`](crate::Select::comment) for what the label is for.
    pub fn comment(mut self, comment: impl Into<alloc::string::String>) -> Self {
        self.query.comment = Some(comment.into());
        self
    }
}

impl<'a, E: Executor, T> Delete<'a, E, T, true> {
    /// Return the deleted rows.
    pub fn returning<S, P>(mut self, selection: S) -> DeleteReturning<'a, E, T, S::Out>
    where
        T: Table,
        S: Selection<(T,), P>,
    {
        let _ = PhantomData::<P>;
        let (exprs, _) = selection.into_projection();
        self.query.returning = Some(exprs);
        DeleteReturning {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }
}

impl<'a, E, T> IntoFuture for Delete<'a, E, T, true>
where
    E: Executor + Send + 'a,
    T: 'a,
{
    type Output = Result<E::Output, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let prefix = self.exec.table_prefix();
            let mut exec = self.exec;
            let sql = self
                .query
                .build::<DialectOf<E>>(prefix)
                .finish()
                .map_err(<E::Driver as Driver>::error)?;
            exec.execute(&sql).await
        })
    }
}

/// A `DELETE ... RETURNING` under construction.
pub struct DeleteReturning<'a, E: Executor, T, R> {
    exec: E,
    query: DeleteQuery,
    phantom: PhantomData<(&'a (), fn() -> (T, R))>,
}

impl<'a, E: Executor, T, R> DeleteReturning<'a, E, T, R> {
    /// Label the statement with a comment, rendered as a trailing `/* ... */`.
    ///
    /// See [`Select::comment`](crate::Select::comment) for what the label is for.
    pub fn comment(mut self, comment: impl Into<alloc::string::String>) -> Self {
        self.query.comment = Some(comment.into());
        self
    }

    /// Render the exact SQL this query will run.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.clone().build(self.exec.table_prefix()))
    }
}

impl<'a, E, T, R> IntoFuture for DeleteReturning<'a, E, T, R>
where
    E: Executor + Send + 'a,
    T: 'a,
    R: Decode<E::Driver> + 'a,
{
    type Output = Result<Vec<R>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let prefix = self.exec.table_prefix();
            let mut exec = self.exec;
            let sql = self
                .query
                .build::<DialectOf<E>>(prefix)
                .finish()
                .map_err(<E::Driver as Driver>::error)?;
            let rows = exec.query(&sql).await?;
            let mut out = Vec::with_capacity(rows.len());
            for row in &rows {
                let mut offset = 0;
                out.push(R::decode(row, &mut offset)?);
            }
            Ok(out)
        })
    }
}
