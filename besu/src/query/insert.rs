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
    /// Every row's values, row after row, with one entry per column.
    ///
    /// Flat rather than a `Vec` of rows: a bulk insert used to be one allocation per row,
    /// each sized to a power of two for a handful of entries, and rendering walked the heap
    /// to find them. One buffer holds the lot and the values stay next to each other.
    pub(crate) values: Vec<Option<Value>>,
    pub(crate) conflict: Option<OnConflict>,
    pub(crate) returning: Option<Vec<ExprKind>>,
}

impl InsertQuery {
    pub(crate) fn new(table: &'static TableDef) -> Self {
        Self {
            table,
            values: Vec::new(),
            conflict: None,
            returning: None,
        }
    }

    /// The number of entries each row contributes.
    fn stride(&self) -> usize {
        self.table.columns.len()
    }

    /// How many rows have been added.
    fn row_count(&self) -> usize {
        match self.stride() {
            0 => 0,
            stride => self.values.len() / stride,
        }
    }

    pub(crate) fn build<D: besu_core::Dialect>(
        mut self,
        prefix: Option<alloc::sync::Arc<str>>,
    ) -> besu_core::Sql<D> {
        let stride = self.stride();
        let rows = self.row_count();

        // Size the output once. The estimate is per row and per kept column, both of which
        // are known from the table and the number of values, so a bulk insert never grows
        // the string: it writes into exactly the space it needs.
        let bytes = 32 + self.table.name.len() + stride * 8 + rows * (stride * 4 + 4);
        let mut out = besu_core::Sql::with_capacity(bytes);
        out.set_table_prefix(prefix);

        if self.values.is_empty() {
            out.set_error(Error::invalid_query(
                "`insert` requires at least one value; call `.value(..)` or `.values(..)`",
            ));
            return out;
        }

        if stride == 0 || self.values.len() % stride != 0 {
            out.set_error(Error::invalid_query(alloc::format!(
                "`insert` produced {} value(s) for a table with {} column(s); \
                 `Insertable` must return one entry per column",
                self.values.len(),
                stride
            )));
            return out;
        }

        // Columns that at least one row provides. Everything else is left to the database's
        // default, which is why a never-provided column simply disappears from the column
        // list instead of being written as `NULL`.
        let kept: Vec<usize> = (0..stride)
            .filter(|index| {
                self.values
                    .iter()
                    .skip(*index)
                    .step_by(stride)
                    .any(Option::is_some)
            })
            .collect();

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

        if kept.len().saturating_mul(rows) > D::MAX_PARAMS {
            out.set_error(Error::invalid_query(alloc::format!(
                "this insert binds {} parameters but `{}` accepts at most {}; split it into \
                 batches of {} row(s) or fewer",
                kept.len() * rows,
                D::NAME,
                D::MAX_PARAMS,
                D::MAX_PARAMS / kept.len().max(1)
            )));
            return out;
        }

        out.reserve_args(kept.len() * rows);

        for (row_index, row) in self.values.chunks_exact_mut(stride).enumerate() {
            if row_index != 0 {
                out.push_raw(", ");
            }
            out.push_raw("(");
            for (i, index) in kept.iter().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                match &mut row[*index] {
                    Some(value) => out.bind_value(core::mem::replace(value, Value::Null)),
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

        if let Some(conflict) = &mut self.conflict {
            render_conflict(&mut out, conflict);
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

        out
    }
}

/// Render an `ON CONFLICT` / `ON DUPLICATE KEY UPDATE` clause.
fn render_conflict<D: besu_core::Dialect>(out: &mut besu_core::Sql<D>, conflict: &mut OnConflict) {
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
        if let ConflictAction::Update(sets) = &mut conflict.action {
            for (i, (column, value)) in sets.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                out.push_ident(column);
                out.push_raw(" = ");
                out.bind_value(core::mem::replace(value, Value::Null));
            }
        }
        return;
    }

    if conflict.target.is_empty() {
        out.set_error(Error::invalid_query(
            "`ON CONFLICT` needs a target column; call `.on_conflict(column)` first",
        ));
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

    match &mut conflict.action {
        ConflictAction::Nothing => out.push_raw("DO NOTHING"),
        ConflictAction::Update(sets) => {
            out.push_raw("DO UPDATE SET ");
            for (i, (column, value)) in sets.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                out.push_ident(column);
                out.push_raw(" = ");
                out.bind_value(core::mem::replace(value, Value::Null));
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
        value.write_values(&mut self.query.values);
        self
    }

    /// Insert many rows in a single statement.
    pub fn values(mut self, values: impl IntoIterator<Item = I>) -> Self {
        for value in values {
            value.write_values(&mut self.query.values);
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

    /// Insert many rows, splitting them across statements so no single statement exceeds the
    /// database's parameter limit.
    ///
    /// Returns what each statement reported. The batches are **not** atomic — wrap the call in
    /// [`Database::transaction`](crate::Database::transaction) if all of them must succeed or
    /// none. A `returning` clause is not available here, because there is no single statement
    /// to attach it to.
    pub async fn values_chunked(
        self,
        values: impl IntoIterator<Item = I>,
        rows_per_statement: usize,
    ) -> Result<Vec<E::Output>, ErrorOf<E>> {
        let prefix = self.exec.table_prefix();
        let mut exec = self.exec;
        let stride = self.query.table.columns.len();
        // One flat buffer for the whole iterator, drained one statement at a time. Building
        // a row at a time and pushing it into a `VecDeque` allocated per row for no reason:
        // nothing here needs to revisit a value.
        let mut pending = Vec::new();
        for value in values {
            value.write_values(&mut pending);
        }
        let mut pending = pending.into_iter();

        let per_statement = rows_per_statement.max(1).saturating_mul(stride.max(1));
        let mut outputs = Vec::new();
        loop {
            let mut query = InsertQuery::new(self.query.table);
            query.conflict = self.query.conflict.clone();
            query.values.reserve(per_statement);
            for _ in 0..per_statement {
                match pending.next() {
                    Some(value) => query.values.push(value),
                    None => break,
                }
            }
            if query.values.is_empty() {
                break;
            }

            let sql = query
                .build::<DialectOf<E>>(prefix.clone())
                .finish()
                .map_err(<E::Driver as Driver>::error)?;
            outputs.push(exec.execute(&sql).await?);
        }

        Ok(outputs)
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
        Ok(self.query.clone().build(self.exec.table_prefix()).seal())
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
            let prefix = self.exec.table_prefix();
            let (mut exec, query) = self.into_parts();
            let sql = query
                .build::<DialectOf<E>>(prefix.clone())
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
            let prefix = self.exec.table_prefix();
            let mut exec = self.exec;
            let sql = self
                .query
                .build::<DialectOf<E>>(prefix.clone())
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
