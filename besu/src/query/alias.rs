//! Table aliases: the same table under a second name in one statement.
//!
//! `#[derive(Table)]` gives each table one handle, and one handle means one name. A
//! self-join needs the same table twice under two names, which is what [`alias`] provides:
//!
//! ```ignore
//! let manager = alias(employees, "manager");
//! let pairs: Vec<(Employee, Option<Employee>)> = db
//!     .select(employees)
//!     .left_join(&manager, manager.column(employees.id).eq(employees.manager_id))
//!     .all()
//!     .await?;
//! ```
//!
//! ```sql
//! SELECT "employees"."id", ..., "manager"."id", ...
//! FROM "employees"
//! LEFT JOIN "employees" AS "manager" ON ("manager"."id" = "employees"."manager_id")
//! ```
//!
//! # Why the alias is a separate type
//!
//! The query's row set carries [`Alias<T>`] rather than `T`, so [`Contains`](crate::Contains)
//! proves that *this* alias was joined and not the table it renames. A query may hold
//! `Employee` and `Alias<Employee>` side by side, and neither stands in for the other.
//!
//! # Why the columns are reached through a method
//!
//! A derived table's columns are its projection and a common table expression's are the
//! table it reads; both are values, so [`Derived::column`](crate::Derived::column) and
//! [`Cte::column`](crate::Cte::column) take a column handle and re-qualify it. An alias is
//! the same problem with a shorter answer: its columns are the base table's, by definition,
//! so [`Alias::column`] only has to swap the qualifier:
//!
//! ```ignore
//! manager.column(employees.email) // "manager"."email"
//! ```
//!
//! The argument is a column of the base table, which is what keeps the Rust type, the SQL
//! type and the nullability checked against the schema.
//!
//! # One alias per table
//!
//! [`Alias<T>`] is one type per table, so two aliases of the same table in one statement are
//! indistinguishable to [`Contains`](crate::Contains). Joining both makes the containment
//! search find two matching paths for one column, and the query stops compiling with
//! `E0283: type annotations needed` rather than picking one — which alias a column referred
//! to would be a guess, and the guess could widen the wrong side of an outer join.
//!
//! Telling the two apart needs a type parameter for the name, and the function cannot invent
//! one; `docs/queries/aliases.md` describes the shape that change would take.
//!
//! # Read-only
//!
//! An alias exists only inside a statement, so it is not a [`Table`] and cannot be inserted
//! into, updated or deleted. Those builders take the declared table handle, which is the
//! only thing that knows the physical name.

use core::marker::PhantomData;

use crate::condition::Contains;
use crate::expr::{Expr, ExprKind};
use crate::query::{AliasSource, From};
use crate::table::{Table, TableRef};
use crate::types::{Column, Nullability, SqlType};

/// A table renamed inside one statement.
///
/// Created by [`alias`]; see it for the SQL and for how the columns are reached.
pub struct Alias<T: Table> {
    /// The name written after `AS`, and the qualifier every one of its columns carries.
    name: &'static str,
    /// `fn() -> T` because the alias is the same table, not a container for one: the marker
    /// has to stay `Send` and `Sync` whatever `T` is.
    phantom: PhantomData<fn() -> T>,
}

impl<T: Table> Clone for Alias<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Table> Copy for Alias<T> {}

impl<T: Table> core::fmt::Debug for Alias<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Alias")
            .field("table", &T::DEF.name)
            .field("alias", &self.name)
            .finish()
    }
}

/// Rename `table` for the statement that reads it.
///
/// ```ignore
/// let manager = alias(employees, "manager");
/// ```
///
/// ```sql
/// FROM "employees" AS "manager"
/// ```
///
/// The name is quoted, so it may be any identifier, but it is written into the SQL rather
/// than bound; passing a value from another source would be an injection. It must also be
/// unique within the statement, because it is how the database tells the two occurrences of
/// the table apart — the builder does not rename colliding aliases, since silently changing
/// a name would change which table a column reference meant.
///
/// Understand the result as a [`TableRef`](crate::TableRef): pass it to
/// [`Database::select`](crate::Database::select) to read from it, or by reference to any of
/// the join methods to add it to a query that already has a source. Its columns are reached
/// with [`Alias::column`], which is what makes the join condition type-check.
///
/// The table is taken by value only so the compiler knows which table is being renamed; the
/// handle itself is zero-sized and nothing is stored but the name.
pub fn alias<T: Table>(_table: T, name: &'static str) -> Alias<T> {
    Alias {
        name,
        phantom: PhantomData,
    }
}

impl<T: Table> Alias<T> {
    /// The name written after `AS`.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// One of the base table's columns, qualified by the alias.
    ///
    /// The argument is the column handle of the table the alias renames, so the Rust type and
    /// the SQL type are the ones the schema declares, and a column of another table is
    /// rejected. The returned expression is qualified by the alias instead of by the table:
    ///
    /// ```ignore
    /// let manager = alias(employees, "manager");
    /// manager.column(employees.email) // "manager"."email"
    /// ```
    ///
    /// The row set must contain this alias, so a handle the statement never joined does not
    /// compile. That check is what keeps the two sides of a self-join apart: `employees` and
    /// `manager` are different types in the row set even though they are one table.
    ///
    /// A presence check is all the compiler can do. Two aliases of one table are the same
    /// type, so this method cannot tell which of them the statement joined; the database
    /// checks the name itself. Only one alias per table may be in a statement at a time, and
    /// joining a second is rejected rather than guessed at.
    ///
    /// The result is an [`Expr`] rather than a [`Column`] because the column type carries
    /// the table it belongs to, and an alias is not a table. Being an expression, it can be
    /// compared, ordered, grouped and projected, which is everything a column can do in a
    /// query that only reads.
    pub fn column<RowSet, P, S, N>(&self, column: Column<T, S, N>) -> Expr<RowSet, S>
    where
        S: SqlType,
        N: Nullability,
        RowSet: Contains<P, Alias<T>>,
    {
        let _ = PhantomData::<P>;
        // Unprefixed: the alias is a name in the statement rather than a table in the
        // database, so neither it nor the tenant prefix belongs in front of the column.
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.name,
            column: column.name(),
        })
    }

    /// The `FROM` source this handle stands for.
    fn source(&self) -> From {
        From::Alias(AliasSource {
            alias: self.name,
            table: T::DEF,
        })
    }
}

impl<T: Table> TableRef for Alias<T> {
    type Marker = Alias<T>;
    type Model = T::Model;

    fn into_source(self) -> From {
        self.source()
    }
}

/// Implemented for the borrowed handle as well, so a join can name the alias without
/// giving it up — the join condition usually needs the alias too.
impl<T: Table> TableRef for &Alias<T> {
    type Marker = Alias<T>;
    type Model = T::Model;

    fn into_source(self) -> From {
        self.source()
    }
}
