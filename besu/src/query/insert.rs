use core::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use alloc::vec::Vec;

use besu_core::{Decode, DialectOf, Driver, Error, ErrorOf, Executor, TableDef, Value};

use crate::condition::Contains;
use crate::expr::{ExprKind, IntoCondition, IntoExpr};
use crate::query::push_table;
use crate::selection::Selection;
use crate::table::{Insertable, Table};
use crate::types::{Column, Nullability, SqlType};

/// What to do when an `INSERT` collides with a unique constraint.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConflictAction {
    /// Leave the existing row alone.
    Nothing,
    /// Update the listed columns.
    ///
    /// The right-hand side is an expression rather than a value so it can read the row the
    /// insert was trying to add, which is what `excluded(column)` produces.
    Update(Vec<(&'static str, ExprKind)>),
}

/// An `ON CONFLICT` clause under construction.
#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    /// The unique columns the conflict is inferred from. Empty is legal for `DO NOTHING`.
    pub(crate) target: Vec<&'static str>,
    /// The predicate that selects a partial unique index as the target.
    pub(crate) target_filter: Vec<ExprKind>,
    pub(crate) action: ConflictAction,
    /// The predicate that decides whether the update runs, evaluated against the existing
    /// row and the incoming one.
    pub(crate) action_filter: Vec<ExprKind>,
}

impl OnConflict {
    fn new(target: Vec<&'static str>) -> Self {
        Self {
            target,
            target_filter: Vec::new(),
            action: ConflictAction::Nothing,
            action_filter: Vec::new(),
        }
    }

    /// Whether the update reads the row the insert was trying to add.
    ///
    /// MySQL only accepts a row alias on a statement that mentions it, so this decides
    /// whether the alias is written at all.
    fn reads_upsert_row(&self) -> bool {
        match &self.action {
            ConflictAction::Nothing => false,
            ConflictAction::Update(sets) => sets.iter().any(|(_, expr)| expr.reads_upsert_row()),
        }
    }
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
    /// MySQL's name for the incoming row, written only when the conflict clause reads it.
    pub(crate) row_alias: &'static str,
    pub(crate) returning: Option<Vec<ExprKind>>,
}

impl InsertQuery {
    pub(crate) fn new(table: &'static TableDef) -> Self {
        Self {
            table,
            values: Vec::new(),
            conflict: None,
            row_alias: "new",
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
            let row_alias = self.row_alias;
            // MySQL reads the incoming row through a row alias, and only accepts the alias
            // on a statement that mentions it. Writing it for a plain `do_update` would
            // needlessly require 8.0.19 and change the statement's text for no gain.
            if D::UPSERT.is_mysql() && conflict.reads_upsert_row() {
                out.push_raw(" AS ");
                out.push_ident(row_alias);
            }
            render_conflict(&mut out, conflict, row_alias);
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

        // Sealed in the builder, like every other statement, so the text `to_sql` shows and
        // the text the driver caches are the same string.
        out.seal()
    }
}

/// Render an `ON CONFLICT` / `ON DUPLICATE KEY UPDATE` clause.
fn render_conflict<D: besu_core::Dialect>(
    out: &mut besu_core::Sql<D>,
    conflict: &mut OnConflict,
    row_alias: &'static str,
) {
    if D::UPSERT.is_mysql() {
        // Neither predicate has a MySQL spelling. `ON DUPLICATE KEY UPDATE` fires for every
        // unique key rather than an inferred index, and its `SET` list has no `WHERE`.
        if !conflict.target_filter.is_empty() {
            out.set_error(Error::unsupported(alloc::format!(
                "`{}` has no predicate on a conflict target: `ON DUPLICATE KEY UPDATE` fires \
                 for every unique key; remove `on_conflict_where`",
                D::NAME
            )));
            return;
        }
        if !conflict.action_filter.is_empty() {
            out.set_error(Error::unsupported(alloc::format!(
                "`{}` has no `WHERE` on an upsert's update; wrap the new value in a `case` \
                 expression instead",
                D::NAME
            )));
            return;
        }

        match &mut conflict.action {
            ConflictAction::Nothing => {
                if conflict.target.is_empty() {
                    // A no-op needs a column to assign to itself, and without a target there
                    // is none. `INSERT IGNORE` is not a substitute: it also discards
                    // truncation and foreign-key errors, which is a different statement.
                    out.set_error(Error::unsupported(alloc::format!(
                        "`{}` has no `ON CONFLICT DO NOTHING` without a target; name a key \
                         with `.on_conflict(column)` so the statement can assign it to \
                         itself, or use `do_update` instead",
                        D::NAME
                    )));
                    return;
                }
                out.push_raw(" ON DUPLICATE KEY UPDATE ");
                for (i, column) in conflict.target.iter().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    out.push_ident(column);
                    out.push_raw(" = ");
                    out.push_ident(column);
                }
            }
            ConflictAction::Update(sets) => {
                if sets.is_empty() {
                    out.set_error(Error::invalid_query(
                        "`do_update` needs at least one column; call \
                         `.do_update(column, value)`",
                    ));
                    return;
                }
                out.push_raw(" ON DUPLICATE KEY UPDATE ");
                for (i, (column, expr)) in sets.iter_mut().enumerate() {
                    if i != 0 {
                        out.push_raw(", ");
                    }
                    out.push_ident(column);
                    out.push_raw(" = ");
                    expr.set_upsert_row(row_alias);
                    expr.render(out);
                }
            }
        }
        return;
    }

    if conflict.target.is_empty() && !conflict.target_filter.is_empty() {
        out.set_error(Error::invalid_query(
            "`on_conflict_where` needs a conflict target, because its predicate selects \
             which unique index the target refers to; call `.on_conflict(column)` first",
        ));
        return;
    }

    if conflict.target.is_empty()
        && matches!(conflict.action, ConflictAction::Update(_))
        && !D::SUPPORTS_TARGETLESS_DO_UPDATE
    {
        // PostgreSQL requires the inference specification: `ON CONFLICT DO UPDATE` on its
        // own is a syntax error, and a `WHERE` does not stand in for the target because the
        // predicate has nowhere to attach without the column list. SQLite allows the
        // omission on its last conflict clause, so it is excluded here.
        out.set_error(Error::invalid_query(alloc::format!(
            "`ON CONFLICT DO UPDATE` needs a conflict target on `{}`; call \
             `.on_conflict(column)` to name the key it applies to, or use `.do_nothing()`",
            D::NAME
        )));
        return;
    }

    out.push_raw(" ON CONFLICT ");
    if !conflict.target.is_empty() {
        out.push_raw("(");
        for (i, column) in conflict.target.iter().enumerate() {
            if i != 0 {
                out.push_raw(", ");
            }
            out.push_ident(column);
        }
        out.push_raw(")");
        if !conflict.target_filter.is_empty() {
            crate::query::update::render_where(out, &mut conflict.target_filter);
        }
        out.push_raw(" ");
    }

    match &mut conflict.action {
        ConflictAction::Nothing => out.push_raw("DO NOTHING"),
        ConflictAction::Update(sets) => {
            if sets.is_empty() {
                out.set_error(Error::invalid_query(
                    "`do_update` needs at least one column; call `.do_update(column, value)`",
                ));
                return;
            }
            out.push_raw("DO UPDATE SET ");
            for (i, (column, expr)) in sets.iter_mut().enumerate() {
                if i != 0 {
                    out.push_raw(", ");
                }
                out.push_ident(column);
                out.push_raw(" = ");
                expr.render(out);
            }
            if !conflict.action_filter.is_empty() {
                crate::query::update::render_where(out, &mut conflict.action_filter);
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
        let values = values.into_iter();
        // `Vec::into_iter` and slices report an exact length, which is the common case for
        // a bulk insert: reserving once is one allocation and no copying, where growing
        // from empty reallocates and copies the whole buffer a dozen times.
        let (rows, _) = values.size_hint();
        self.query
            .values
            .reserve(rows.saturating_mul(self.query.stride()));

        for value in values {
            value.write_values(&mut self.query.values);
        }
        self
    }

    /// Start an `ON CONFLICT` clause targeting the given unique columns.
    ///
    /// A single column or a tuple of up to four names the key the clause applies to, which
    /// must be a unique constraint or index. Omit the target with [`Self::on_conflict_all`]
    /// when the action is `do_nothing`.
    pub fn on_conflict(mut self, target: impl ConflictTarget) -> Self {
        self.query.conflict = Some(OnConflict::new(target.into_columns()));
        self
    }

    /// Upsert against any unique constraint, with no target.
    ///
    /// PostgreSQL and SQLite render `ON CONFLICT DO NOTHING`, which needs no index named.
    /// A targetless `DO UPDATE` is SQLite-only: PostgreSQL requires the target to say which
    /// key the update applies to, and rejects the statement without it.
    pub fn on_conflict_all(mut self) -> Self {
        self.query.conflict = Some(OnConflict::new(Vec::new()));
        self
    }

    /// The predicate that selects a partial unique index as the conflict target.
    ///
    /// PostgreSQL and SQLite write it between the target and the action, as in
    /// `ON CONFLICT ("email") WHERE "deleted_at" IS NULL DO UPDATE ...`. MySQL's upsert
    /// fires for every unique key and has nowhere to put a predicate, so it records an
    /// `unsupported` error rather than dropping it.
    pub fn on_conflict_where<F, P>(mut self, condition: F) -> Self
    where
        F: IntoCondition<M, P>,
    {
        let _ = PhantomData::<P>;
        let condition = condition.into_condition();
        self.query
            .conflict
            .get_or_insert_with(|| OnConflict::new(Vec::new()))
            .target_filter
            .push(condition);
        self
    }

    /// Ignore conflicting rows.
    ///
    /// On MySQL this renders `ON DUPLICATE KEY UPDATE <target> = <target>`, which is the
    /// one spelling of "leave the row alone" that does not also swallow truncation and
    /// foreign-key errors the way `INSERT IGNORE` would. That needs a target to assign to
    /// itself, so MySQL keeps the `unsupported` error when there is none.
    pub fn do_nothing(mut self) -> Self {
        if let Some(conflict) = &mut self.query.conflict {
            conflict.action = ConflictAction::Nothing;
            // `DO NOTHING` has no `WHERE`, so a predicate left over from an earlier
            // `do_update_filter` would render into a grammar slot that does not exist.
            conflict.action_filter.clear();
        }
        self
    }

    /// Update the given column when a conflict occurs.
    ///
    /// The column must belong to the table being inserted into, and the value must be of
    /// that column's SQL type — `do_update(users.email, 42)` does not compile. The value may
    /// also be an expression, which is how the incoming row is read:
    ///
    /// ```ignore
    /// .on_conflict(users.email)
    /// .do_update(users.name, excluded(users.name))
    /// ```
    pub fn do_update<S, N, T, V, P, Pv>(mut self, column: Column<T, S, N>, value: V) -> Self
    where
        T: Table,
        S: SqlType,
        N: Nullability,
        M: Contains<P, T>,
        V: IntoExpr<M, S, Pv>,
    {
        let _ = PhantomData::<(P, Pv)>;
        self.push_update(column.name(), value.into_expr().into_kind());
        self
    }

    /// Update the given column to an expression when a conflict occurs.
    ///
    /// This is the explicit-expression spelling of [`Self::do_update`], which accepts a
    /// value or an expression. Both check the expression against the column's SQL type.
    /// Use it when the value being an expression is the point:
    ///
    /// ```ignore
    /// .do_update_expr(users.age, users.age.as_expr().plus(1))
    /// ```
    pub fn do_update_expr<S, N, T, V, P, Pv>(self, column: Column<T, S, N>, value: V) -> Self
    where
        T: Table,
        S: SqlType,
        N: Nullability,
        M: Contains<P, T>,
        V: IntoExpr<M, S, Pv>,
    {
        self.do_update(column, value)
    }

    /// Only run the update where the existing row satisfies `condition`.
    ///
    /// PostgreSQL and SQLite render it as `DO UPDATE SET ... WHERE ...`, evaluated against
    /// the existing row (and the incoming one, through `excluded`). MySQL's upsert has no
    /// per-row condition, so it records an `unsupported` error; build the value with a
    /// `case` expression there instead.
    pub fn do_update_filter<F, P>(mut self, condition: F) -> Self
    where
        F: IntoCondition<M, P>,
    {
        let _ = PhantomData::<P>;
        let condition = condition.into_condition();
        self.query
            .conflict
            .get_or_insert_with(|| OnConflict::new(Vec::new()))
            .action_filter
            .push(condition);
        self
    }

    /// Name MySQL's row alias for the row being inserted.
    ///
    /// MySQL 8.0.19 reads the incoming row through a row alias — `INSERT ... VALUES (...)
    /// AS new ON DUPLICATE KEY UPDATE ... = new.column` — rather than the deprecated
    /// `VALUES()` function, and the alias is `new` unless you change it here. The clause is
    /// only written when the update actually reads the incoming row, so a plain `do_update`
    /// statement is unchanged. PostgreSQL and SQLite ignore this: their spelling is the
    /// standard `excluded`.
    pub fn row_alias(mut self, alias: &'static str) -> Self {
        self.query.row_alias = alias;
        self
    }

    /// Add one `SET` entry to the conflict action, starting the clause if it is new.
    fn push_update(&mut self, column: &'static str, value: ExprKind) {
        match &mut self.query.conflict {
            Some(conflict) => match &mut conflict.action {
                ConflictAction::Update(sets) => sets.push((column, value)),
                ConflictAction::Nothing => {
                    conflict.action = ConflictAction::Update(alloc::vec![(column, value)]);
                }
            },
            None => {
                let mut conflict = OnConflict::new(Vec::new());
                conflict.action = ConflictAction::Update(alloc::vec![(column, value)]);
                self.query.conflict = Some(conflict);
            }
        }
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
        let values = values.into_iter();
        let (rows, _) = values.size_hint();
        let mut pending = Vec::with_capacity(rows.saturating_mul(stride));
        for value in values {
            value.write_values(&mut pending);
        }
        let mut pending = pending.into_iter();

        let per_statement = rows_per_statement.max(1).saturating_mul(stride.max(1));
        let mut outputs = Vec::new();
        loop {
            let mut query = InsertQuery::new(self.query.table);
            query.conflict = self.query.conflict.clone();
            query.row_alias = self.query.row_alias;
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
        Ok(self.query.clone().build(self.exec.table_prefix()))
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

/// An empty target, which is what `ON CONFLICT DO NOTHING` without an index needs.
///
/// [`Insert::on_conflict_all`] is the readable spelling; this exists so `on_conflict(())`
/// means the same thing.
impl ConflictTarget for () {
    fn into_columns(self) -> Vec<&'static str> {
        Vec::new()
    }
}

impl<T: Table, S: crate::types::SqlType, N: crate::types::Nullability> ConflictTarget
    for crate::types::Column<T, S, N>
{
    fn into_columns(self) -> Vec<&'static str> {
        alloc::vec![self.name()]
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
