//! Derived tables: a subquery in `FROM`.
//!
//! A [`Derived`] is a finished `SELECT` that an outer statement reads as if it were a table.
//! The expression is written out where it is used, so it never runs on its own:
//!
//! ```ignore
//! let totals = derived(db.select(posts).pick(posts.author_id), "totals");
//! let author = totals.column(posts.author_id);
//! let rows: Vec<(i64, i64)> = db
//!     .select(&totals)
//!     .pick((author.clone(), count_all()))
//!     .group_by(author)
//!     .all()
//!     .await?;
//! ```
//!
//! ```sql
//! SELECT "totals"."author_id", COUNT(*) FROM (
//!     SELECT "posts"."author_id" FROM "posts"
//! ) AS "totals" GROUP BY "totals"."author_id"
//! ```
//!
//! # Why the columns are reached through a method
//!
//! A declared table's columns exist before the program runs, so `#[derive(Table)]` can give
//! the handle one field per column. A derived table's columns are its projection, which is a
//! value: the type system knows the row type but not the names, and Rust has no field access
//! that can be built from a value. [`Derived::column`] supplies the missing half exactly as
//! [`Cte::column`](crate::Cte::column) does — it takes the column handle the projection came
//! from and returns the same column qualified by the alias. [`Derived::column_named`] is the
//! same idea for a projection that is an expression rather than a column.

use core::marker::PhantomData;

use besu_core::Executor;

use crate::condition::Contains;
use crate::expr::{Expr, ExprKind};
use crate::query::{DerivedSource, From, Select, SelectQuery};
use crate::table::{Table, TableRef};
use crate::types::{Column, Nullability, SqlType, TypeWitness};

/// A subquery given a name and read as a table by the enclosing statement.
///
/// Created by [`derived`]; see it for the SQL and for how the columns are reached.
pub struct Derived<R> {
    /// The name written after the subquery.
    alias: &'static str,
    /// The column names the subquery produces, in order.
    ///
    /// `None` means the projection is an expression, which has no portable name.
    columns: alloc::vec::Vec<Option<&'static str>>,
    /// The subquery itself.
    body: alloc::boxed::Box<SelectQuery>,
    phantom: PhantomData<fn() -> R>,
}

impl<R> Clone for Derived<R> {
    fn clone(&self) -> Self {
        Self {
            alias: self.alias,
            columns: self.columns.clone(),
            body: self.body.clone(),
            phantom: PhantomData,
        }
    }
}

impl<R> core::fmt::Debug for Derived<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Derived")
            .field("alias", &self.alias)
            .field("columns", &self.columns)
            .finish()
    }
}

/// Name `select` and read it as a table from the enclosing statement.
///
/// ```ignore
/// let counts = derived(
///     db.select(posts)
///         .filter(posts.published)
///         .pick(posts.author_id),
///     "counts",
/// );
/// ```
///
/// ```sql
/// SELECT ... FROM (
///     SELECT "posts"."author_id" FROM "posts" WHERE "posts"."published"
/// ) AS "counts"
/// ```
///
/// The expression is rendered into the enclosing statement, so its parameters continue that
/// statement's numbering, and its dialect is the enclosing statement's. That also means the
/// name must be unique within the statement: the builder does not rename colliding aliases,
/// because silently changing a name would change which table a column reference meant.
///
/// The alias is quoted, so it may be any identifier, but it is written into the SQL rather
/// than bound. Passing a value from another source would be an injection.
///
/// Read the expression's columns with [`Derived::column`], or select from it with
/// [`Database::select`](crate::Database::select) and let the default projection read every
/// column. A projection made only of plain columns is decodable as a whole row; one that
/// includes expressions has no portable column names, so it needs an explicit `pick` on the
/// outer query.
pub fn derived<'a, E: Executor, M, R>(
    select: Select<'a, E, M, R>,
    alias: &'static str,
) -> Derived<R> {
    let query = select.query;
    let columns = projection_names(&query);
    Derived {
        alias,
        columns,
        body: alloc::boxed::Box::new(query),
        phantom: PhantomData,
    }
}

/// The names a query's projection produces, in order.
///
/// A plain column keeps its name. An expression records `None`, because the name a database
/// invents for `COUNT(*)` differs between the three: PostgreSQL calls it `count`, while
/// SQLite and MySQL call it `count(*)`.
fn projection_names(query: &SelectQuery) -> alloc::vec::Vec<Option<&'static str>> {
    match &query.projection {
        Some(projection) => projection.iter().map(column_name).collect(),
        None => {
            let mut names = query.from.column_names();
            for join in &query.joins {
                names.extend(join.source.column_names());
            }
            names
        }
    }
}

/// The name of a projected expression, when it has one of its own.
fn column_name(expr: &ExprKind) -> Option<&'static str> {
    match expr {
        ExprKind::Column { column, .. } | ExprKind::UnprefixedColumn { column, .. } => Some(column),
        _ => None,
    }
}

impl<R: 'static> Derived<R> {
    /// The alias written after the subquery.
    pub const fn alias(&self) -> &'static str {
        self.alias
    }

    /// The number of columns the subquery produces.
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// One of the expression's columns, typed like the column it came from.
    ///
    /// The argument is the column handle the projection selected, so the Rust type, the SQL
    /// type and the nullability are the ones the schema declares. The returned expression is
    /// qualified by the alias instead of the table's:
    ///
    /// ```ignore
    /// let totals = derived(db.select(posts).pick(posts.author_id), "totals");
    /// totals.column(posts.author_id)
    /// ```
    ///
    /// The row set must contain this expression, so a handle the statement never selected
    /// from does not compile. A presence check is all the compiler can do: two expressions
    /// over the same row type are different values of the same type, so the compiler cannot
    /// tell them apart. The database checks the column name itself.
    pub fn column<RowSet, P, T, S, N>(&self, column: Column<T, S, N>) -> Expr<RowSet, S>
    where
        T: Table,
        S: SqlType,
        N: Nullability,
        RowSet: Contains<P, Derived<R>>,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.alias,
            column: column.name(),
        })
    }

    /// One of the expression's columns, for a projection that is an expression.
    ///
    /// `name` is the column's name in the subquery's result and `witness` supplies its SQL
    /// type, exactly as [`scalar`](crate::expr::scalar) uses a witness.
    ///
    /// Without an `AS` in the subquery an expression column is named by the database, and the
    /// three do not agree — PostgreSQL calls `COUNT(*)` `count` while the other two call it
    /// `count(*)` — so this is an escape hatch for a projection whose name you control on
    /// every dialect you target. Prefer [`Derived::column`] whenever the projection is a
    /// plain column:
    ///
    /// ```ignore
    /// // `count` is the name PostgreSQL gives a `COUNT(*)` column; MySQL and SQLite use a
    /// // different one, so this projection is only portable if it is named explicitly.
    /// let totals = derived(db.select(posts).pick((posts.author_id, count_all())), "totals");
    /// let author = totals.column(posts.author_id);
    /// let total = totals.column_named("count", Int8);
    /// ```
    ///
    /// `name` is quoted, so it may not rely on the unquoted-identifier folding each dialect
    /// applies. It is written into the SQL rather than bound.
    pub fn column_named<RowSet, P, S, W>(&self, name: &'static str, witness: W) -> Expr<RowSet, S>
    where
        S: SqlType,
        W: TypeWitness<S>,
        RowSet: Contains<P, Derived<R>>,
    {
        let _ = (witness, PhantomData::<P>);
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.alias,
            column: name,
        })
    }

    /// The `FROM` source the enclosing statement writes out.
    fn source(&self) -> From {
        From::Derived(DerivedSource {
            alias: self.alias,
            columns: self.columns.clone(),
            body: self.body.clone(),
        })
    }
}

impl<R: 'static> TableRef for Derived<R> {
    type Marker = Derived<R>;
    type Model = R;

    fn into_source(self) -> From {
        From::Derived(DerivedSource {
            alias: self.alias,
            columns: self.columns,
            body: self.body,
        })
    }
}

impl<R: 'static> TableRef for &Derived<R> {
    type Marker = Derived<R>;
    type Model = R;

    fn into_source(self) -> From {
        self.source()
    }
}
