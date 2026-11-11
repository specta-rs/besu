use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor, TableDef, Value};

use crate::expr::ExprKind;
use crate::query::push_table;
use crate::selection::Selection;
use crate::table::{Insertable, Table};

/// What to do when an `INSERT` collides with a unique constraint.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConflictAction {
    /// Leave the existing row alone.
    Nothing,
    /// Update the listed columns.
    Update(Vec<(&'static str, Value)>),
}

/// An `ON CONFLICT` clause under construction.
#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    pub(crate) target: Vec<&'static str>,
    pub(crate) action: ConflictAction,
}

#[derive(Debug, Clone)]
pub(crate) struct InsertQuery {
    pub(crate) table: &'static TableDef,
    pub(crate) rows: Vec<Vec<Option<Value>>>,
    pub(crate) conflict: Option<OnConflict>,
    pub(crate) returning: Option<Vec<ExprKind>>,
}

impl InsertQuery {
    pub(crate) fn new(table: &'static TableDef) -> Self {
        Self {
            table,
            rows: Vec::new(),
            conflict: None,
            returning: None,
        }
    }

    pub(crate) fn push_row(&mut self, values: Vec<Option<Value>>) {
        self.rows.push(values);
    }

    pub(crate) fn build<D: besu_core::Dialect>(&self) -> besu_core::Sql<D> {
        let mut out = besu_core::Sql::new();

        if self.rows.is_empty() {
            out.set_error(Error::invalid_query(
                "`insert` requires at least one value; call `.value(..)` or `.values(..)`",
            ));
            return out;
        }

        if let Some(bad) = self
            .rows
            .iter()
            .find(|row| row.len() != self.table.columns.len())
        {
            out.set_error(Error::invalid_query(alloc::format!(
                "`insert` produced {} value(s) for a table with {} column(s); \
                 `Insertable` must return one entry per column",
                bad.len(),
                self.table.columns.len()
            )));
            return out;
        }

        // Columns that at least one row provides. Everything else is left to the database's
        // default, which is why a never-provided column simply disappears from the column
        // list instead of being written as `NULL`.
        let mut kept = Vec::new();
        for (index, _column) in self.table.columns.iter().enumerate() {
            if self.rows.iter().any(|row| row[index].is_some()) {
                kept.push(index);
            }
        }

        if kept.is_empty() {
            out.set_error(Error::invalid_query(
                "`insert` was given a value with every column unset",
            ));
            return out;
        }

        out.push_raw("INSERT INTO ");
        push_table(&mut out, self.table);
        out.push_raw(" (");
        for (i, index) in kept.iter().enumerate() {
            if i != 0 {
                out.push_raw(", ");
            }
            out.push_ident(self.table.columns[*index].name);
        }
        out.push_raw(") VALUES ");

        if kept.len().saturating_mul(self.rows.len()) > D::MAX_PARAMS {
            out.set_error(Error::invalid_query(alloc::format!(
                "this insert binds {} parameters but `{}` accepts at most {}; split it into \
                 batches of {} row(s) or fewer",
                kept.len() * self.rows.len(),
                D::NAME,
                D::MAX_PARAMS,
                D::MAX_PARAMS / kept.len().max(1)
            )));
            return out;
        }

        for (row_index, row) in self.rows.iter().enumerate() {
            if row_index != 0 {
                out.push_raw(", ");
            }
            out.push_raw("(");
            for (i, index) in kept.iter().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                match &row[*index] {
                    Some(value) => out.bind(value.clone()),
                    None => {
                        if D::SUPPORTS_DEFAULT_IN_VALUES {
                            out.push_raw("DEFAULT");
                        } else {
                            out.set_error(Error::unsupported(alloc::format!(
                                "`{}` does not support `DEFAULT` inside `VALUES`, and this \
                                 bulk insert provides `{}` in some rows but not others",
                                D::NAME,
                                self.table.columns[*index].name
                            )));
                        }
                    }
                }
            }
            out.push_raw(")");
        }

        if let Some(conflict) = &self.conflict {
            self.render_conflict(&mut out, conflict);
        }

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

    fn render_conflict<D: besu_core::Dialect>(
        &self,
        out: &mut besu_core::Sql<D>,
        conflict: &OnConflict,
    ) {
        if D::UPSERT.is_mysql() {
            if !matches!(conflict.action, ConflictAction::Update(_)) {
                // MySQL has no equivalent of `ON CONFLICT DO NOTHING` that is safe to
                // emulate here (`INSERT IGNORE` also swallows other errors).
                out.set_error(Error::unsupported(alloc::format!(
                    "`{}` has no `ON CONFLICT DO NOTHING`; use `do_update` instead",
                    D::NAME
                )));
                return;
            }
            out.push_raw(" ON DUPLICATE KEY UPDATE ");
            if let ConflictAction::Update(sets) = &conflict.action {
                for (i, (column, value)) in sets.iter().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    out.push_ident(column);
                    out.push_raw(" = ");
                    out.bind(value.clone());
                }
            }
            return;
        }

        out.push_raw(" ON CONFLICT (");
        for (i, column) in conflict.target.iter().enumerate() {
            if i != 0 {
                out.push_raw(", ");
            }
            out.push_ident(column);
        }
        out.push_raw(") ");

        match &conflict.action {
            ConflictAction::Nothing => out.push_raw("DO NOTHING"),
            ConflictAction::Update(sets) => {
                out.push_raw("DO UPDATE SET ");
                for (i, (column, value)) in sets.iter().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    out.push_ident(column);
                    out.push_raw(" = ");
                    out.bind(value.clone());
                }
            }
        }
    }
}

/// An `INSERT` under construction.
pub struct Insert<'a, E: Executor, M, I> {
    pub(crate) exec: E,
    pub(crate) query: InsertQuery,
    pub(crate) phantom: PhantomData<(&'a (), fn() -> (M, I))>,
}

impl<'a, E: Executor, M, I: Insertable> Insert<'a, E, M, I> {
    pub(crate) fn new(exec: E, table: &'static TableDef) -> Self {
        Self {
            exec,
            query: InsertQuery::new(table),
            phantom: PhantomData,
        }
    }

    /// Insert one row.
    pub fn value(mut self, value: I) -> Self {
        self.query.push_row(value.into_values());
        self
    }

    /// Insert many rows in a single statement.
    pub fn values(mut self, values: impl IntoIterator<Item = I>) -> Self {
        for value in values {
            self.query.push_row(value.into_values());
        }
        self
    }

    /// Start an `ON CONFLICT` clause targeting the given unique columns.
    pub fn on_conflict(mut self, target: impl ConflictTarget) -> Self {
        self.query.conflict = Some(OnConflict {
            target: target.into_columns(),
            action: ConflictAction::Nothing,
        });
        self
    }

    /// Ignore conflicting rows.
    pub fn do_nothing(mut self) -> Self {
        if let Some(conflict) = &mut self.query.conflict {
            conflict.action = ConflictAction::Nothing;
        }
        self
    }

    /// Update the given column when a conflict occurs.
    pub fn do_update<V>(mut self, column: impl Into<ColumnRef>, value: V) -> Self
    where
        V: Into<Value>,
    {
        let column = column.into().name;
        match &mut self.query.conflict {
            Some(conflict) => match &mut conflict.action {
                ConflictAction::Update(sets) => sets.push((column, value.into())),
                ConflictAction::Nothing => {
                    conflict.action = ConflictAction::Update(alloc::vec![(column, value.into())])
                }
            },
            None => {
                self.query.conflict.get_or_insert(OnConflict {
                    target: Vec::new(),
                    action: ConflictAction::Update(Vec::new()),
                });
                if let Some(conflict) = &mut self.query.conflict {
                    conflict.action = ConflictAction::Update(alloc::vec![(column, value.into())]);
                }
            }
        }
        self
    }

    /// Return the inserted rows.
    ///
    /// This is a terminal operation: it changes the query's return type, which is why it
    /// lives on its own type.
    pub fn returning<S, P>(mut self, selection: S) -> InsertReturning<'a, E, M, I, S::Out>
    where
        S: Selection<M, P>,
    {
        let _ = PhantomData::<P>;
        let (exprs, _) = selection.into_projection();
        self.query.returning = Some(exprs);
        InsertReturning {
            exec: self.exec,
            query: self.query,
            phantom: PhantomData,
        }
    }

    /// Render the exact SQL this query will run.
    pub fn to_sql(&self) -> Result<besu_core::Sql<DialectOf<E>>, Error> {
        Ok(self.query.build())
    }

    fn into_parts(self) -> (E, InsertQuery) {
        (self.exec, self.query)
    }
}

impl<'a, E, M, I> IntoFuture for Insert<'a, E, M, I>
where
    E: Executor + Send + 'a,
    M: 'a,
    I: Insertable + 'a,
{
    type Output = Result<E::Output, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (mut exec, query) = self.into_parts();
            let sql = query
                .build::<DialectOf<E>>()
                .finish()
                .map_err(<E::Driver as Driver>::error)?;
            exec.execute(&sql).await
        })
    }
}

/// An `INSERT ... RETURNING` under construction.
pub struct InsertReturning<'a, E: Executor, M, I, R> {
    exec: E,
    query: InsertQuery,
    phantom: PhantomData<(&'a (), fn() -> (M, I, R))>,
}

impl<'a, E, M, I, R> IntoFuture for InsertReturning<'a, E, M, I, R>
where
    E: Executor + Send + 'a,
    M: 'a,
    I: 'a,
    R: Decode<E::Driver> + 'a,
{
    type Output = Result<Vec<R>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let mut exec = self.exec;
            let sql = self
                .query
                .build::<DialectOf<E>>()
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

/// Something usable as an `ON CONFLICT` target.
pub trait ConflictTarget {
    /// The columns that make up the target.
    fn into_columns(self) -> Vec<&'static str>;
}

impl<T: Table, S: crate::types::SqlType, N: crate::types::Nullability> ConflictTarget
    for crate::types::Column<T, S, N>
{
    fn into_columns(self) -> Vec<&'static str> {
        alloc::vec![self.name()]
    }
}

/// A column reference used where only the name matters.
#[derive(Debug, Clone, Copy)]
pub struct ColumnRef {
    /// The SQL column name.
    pub name: &'static str,
}

impl<T: Table, S: crate::types::SqlType, N: crate::types::Nullability>
    From<crate::types::Column<T, S, N>> for ColumnRef
{
    fn from(column: crate::types::Column<T, S, N>) -> Self {
        Self {
            name: column.name(),
        }
    }
}

macro_rules! conflict_tuple {
    ($($name:ident),+) => {
        impl<$($name: ConflictTarget),+> ConflictTarget for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_columns(self) -> Vec<&'static str> {
                let ($($name,)+) = self;
                let mut out = Vec::new();
                $(out.extend($name.into_columns());)+
                out
            }
        }
    };
}

conflict_tuple!(A);
conflict_tuple!(A, B);
conflict_tuple!(A, B, C);
conflict_tuple!(A, B, C, D);
