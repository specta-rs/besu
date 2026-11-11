use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor, TableDef, Value};

use crate::types::{Column, Nullability, SqlType};

use crate::expr::{ExprKind, IntoCondition, IntoExpr};
use crate::query::push_table;
use crate::selection::Selection;
use crate::table::Table;

#[derive(Debug, Clone)]
pub(crate) struct UpdateQuery {
    pub(crate) table: &'static TableDef,
    pub(crate) sets: Vec<(&'static str, crate::expr::ExprKind)>,
    pub(crate) filter: Vec<ExprKind>,
    pub(crate) returning: Option<Vec<ExprKind>>,
}

impl UpdateQuery {
    pub(crate) fn new(table: &'static TableDef) -> Self {
        Self {
            table,
            sets: Vec::new(),
            filter: Vec::new(),
            returning: None,
        }
    }

    pub(crate) fn build<D: besu_core::Dialect>(
        &self,
        prefix: Option<alloc::sync::Arc<str>>,
    ) -> besu_core::Sql<D> {
        let mut out = besu_core::Sql::new();
        out.set_table_prefix(prefix);

        if self.sets.is_empty() {
            out.set_error(Error::invalid_query(
                "`update` requires at least one column; call `.set(column, value)`",
            ));
            return out;
        }

        out.push_raw("UPDATE ");
        push_table(&mut out, self.table);
        out.push_raw(" SET ");
        for (i, (column, value)) in self.sets.iter().enumerate() {
            if i != 0 {
                out.push_raw(", ");
            }
            out.push_ident(column);
            out.push_raw(" = ");
            value.render(&mut out);
        }

        render_where(&mut out, &self.filter);

        if let Some(returning) = &self.returning {
            if !D::SUPPORTS_RETURNING {
                out.set_error(Error::unsupported(alloc::format!(
                    "`{}` does not support `RETURNING`",
                    D::NAME
                )));
                return out;
            }
            out.push_raw(" RETURNING ");
            for (i, expr) in returning.iter().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                expr.render(&mut out);
            }
        }

        out
    }
}

pub(crate) fn render_where<D: besu_core::Dialect>(
    out: &mut besu_core::Sql<D>,
    filter: &[ExprKind],
) {
    if filter.is_empty() {
        return;
    }
    out.push_raw(" WHERE ");
    for (i, condition) in filter.iter().enumerate() {
        if i != 0 {
            out.push_raw(" AND ");
        }
        condition.render(out);
    }
}

/// An `UPDATE` under construction.
///
/// The `SAFE` const parameter is the protection against accidental mass updates: an
/// `UPDATE` with no `WHERE` clause does not compile. If you really do mean to touch every
/// row, say so with [`Update::all_rows`].
pub struct Update<'a, E: Executor, T, const SAFE: bool = false> {
    pub(crate) exec: E,
    pub(crate) query: UpdateQuery,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> T)>,
}

impl<'a, E: Executor, T> Update<'a, E, T, false> {
    pub(crate) fn new(exec: E, table: &'static TableDef) -> Self {
        Self {
            exec,
            query: UpdateQuery::new(table),
            phantom: PhantomData,
        }
    }

    /// Add a `WHERE` condition, which makes the statement safe to run.
    pub fn filter<F, P>(self, condition: F) -> Update<'a, E, T, true>
    where
        T: Table,
        F: IntoCondition<(T,), P>,
    {
        let _ = PhantomData::<P>;
        let mut query = self.query;
        query.filter.push(condition.into_condition());
        Update {
            exec: self.exec,
            query,
            phantom: PhantomData,
        }
    }

    /// Explicitly opt into updating every row in the table.
    pub fn all_rows(self) -> Update<'a, E, T, true> {
        Update {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }
}

impl<'a, E: Executor, T, const SAFE: bool> Update<'a, E, T, SAFE> {
    /// `SET column = value`.
    pub fn set<S, N, V>(mut self, column: Column<T, S, N>, value: V) -> Self
    where
        T: Table,
        S: SqlType,
        N: Nullability,
        V: Into<Value> + Into<N::Wrap<S::Rust>>,
    {
        self.query
            .sets
            .push((column.name(), crate::expr::ExprKind::Value(value.into())));
        self
    }

    /// `SET` every column from a model.
    ///
    /// The primary key and any generated column are skipped, because rewriting them is
    /// almost never what you want; set them by hand if you do.
    ///
    /// ```ignore
    /// db.update(users)
    ///     .set_model(&user)
    ///     .filter(users.id.eq(user.id))
    ///     .await?;
    /// ```
    pub fn set_model(mut self, model: &T::Model) -> Self
    where
        T: Table,
    {
        let values = T::model_values(model);
        for (column, value) in T::DEF.columns.iter().zip(values) {
            if column.primary_key || column.auto_increment {
                continue;
            }
            self.query.sets.push((column.name, ExprKind::Value(value)));
        }
        self
    }

    /// `SET column = <expression>`.
    ///
    /// Use this to compute the new value in the database, for example
    /// `.set_expr(users.age, users.age.as_expr().plus(1))`.
    pub fn set_expr<S, N, V, M>(mut self, column: Column<T, S, N>, value: V) -> Self
    where
        T: Table,
        S: SqlType,
        N: Nullability,
        V: IntoExpr<(T,), S, M>,
    {
        self.query
            .sets
            .push((column.name(), value.into_expr().into_kind()));
        self
    }

    /// Render the exact SQL this query will run.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.build(self.exec.table_prefix()).seal())
    }
}

impl<'a, E: Executor, T> Update<'a, E, T, true> {
    /// Return the updated rows.
    pub fn returning<S, P>(mut self, selection: S) -> UpdateReturning<'a, E, T, S::Out>
    where
        T: Table,
        S: Selection<(T,), P>,
    {
        let _ = PhantomData::<P>;
        let (exprs, _) = selection.into_projection();
        self.query.returning = Some(exprs);
        UpdateReturning {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }
}

impl<'a, E, T> IntoFuture for Update<'a, E, T, true>
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

/// An `UPDATE ... RETURNING` under construction.
pub struct UpdateReturning<'a, E: Executor, T, R> {
    exec: E,
    query: UpdateQuery,
    phantom: PhantomData<(&'a (), fn() -> (T, R))>,
}

impl<'a, E, T, R> IntoFuture for UpdateReturning<'a, E, T, R>
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
