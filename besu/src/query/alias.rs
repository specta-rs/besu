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
//! The query's row set carries [`Alias<T, 0>`](Alias) rather than `T`, so
//! [`Contains`](crate::Contains) proves that *this* alias was joined and not the table it
//! renames. A query may hold `Employee` and `Alias<Employee>` side by side, and neither stands
//! in for the other.
//!
//! # Two aliases of one table
//!
//! One self-join can need more than two readings of a table — employees, their manager, and
//! the manager's manager. [`alias`] always produces the tag `0`, and [`alias_as`] lets the
//! caller choose another one, so each alias is its own type:
//!
//! ```ignore
//! let report = alias(employees, "report");
//! let manager = alias_as::<_, 1>(employees, "manager");
//! let rows: Vec<(Employee, Employee)> = db
//!     .select(report)
//!     .inner_join(
//!         &manager,
//!         manager.column(employees.id).eq(report.column(employees.manager_id)),
//!     )
//!     .all()
//!     .await?;
//! ```
//!
//! ```sql
//! SELECT "report"."id", ..., "manager"."id", ...
//! FROM "employees" AS "report"
//! INNER JOIN "employees" AS "manager" ON ("manager"."id" = "report"."manager_id")
//! ```
//!
//! The tag is a type-level marker only: it is not written to the SQL and does not change the
//! alias's name. What it does is keep the row set holding two entries instead of one, so
//! [`Contains`](crate::Contains) finds exactly one path for each alias and `manager.column(..)`
//! only compiles in a query that joined the `1`-tagged handle. Using the manager's column
//! where the report's was meant is a compile error, not a query that silently compares the
//! wrong side.
//!
//! Tags only have to differ within one statement; any `u64` works, and a named constant reads
//! better than a literal when the alias crosses a function boundary.
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
/// Created by [`alias`] or [`alias_as`]; see them for the SQL and for how the columns are
/// reached. `TAG` tells two aliases of the same table apart at the type level and is never
/// written to the SQL. Omitting it means tag `0`, which is what [`alias`] returns.
pub struct Alias<T: Table, const TAG: u64 = 0> {
    /// The name written after `AS`, and the qualifier every one of its columns carries.
    name: &'static str,
    /// `fn() -> T` because the alias is the same table, not a container for one: the marker
    /// has to stay `Send` and `Sync` whatever `T` is.
    phantom: PhantomData<fn() -> T>,
}

impl<T: Table, const TAG: u64> Clone for Alias<T, TAG> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Table, const TAG: u64> Copy for Alias<T, TAG> {}

impl<T: Table, const TAG: u64> core::fmt::Debug for Alias<T, TAG> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Alias")
            .field("table", &T::DEF.name)
            .field("alias", &self.name)
            .field("tag", &TAG)
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
/// The result is [`Alias<T, 0>`](Alias). For a second alias of the same table in one
/// statement use [`alias_as`], which chooses a different tag.
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
pub fn alias<T: Table>(table: T, name: &'static str) -> Alias<T, 0> {
    alias_as::<T, 0>(table, name)
}

/// Rename `table` under a tag of your choosing, so it is distinct from another alias of the
/// same table.
///
/// ```ignore
/// let report = alias(employees, "report");
/// let manager = alias_as::<_, 1>(employees, "manager");
/// ```
///
/// ```sql
/// FROM "employees" AS "report" INNER JOIN "employees" AS "manager" ON ...
/// ```
///
/// The tag is a `u64` chosen by the caller. It does not appear in the SQL; it only tells the
/// query's row set that this is a different occurrence of the table from
/// [`alias`]'s tag `0`, so [`Alias::column`] finds exactly one of them. Tags only need to be
/// unique within one statement.
///
/// The tag cannot be inferred from the arguments, so name it at the call site —
/// `alias_as::<_, 1>(..)`. The table's type is inferred from the handle; write it out as
/// well only when a function signature leaves it ambiguous.
///
/// Prefer [`alias`] for the only alias of a table: it needs no annotation and means the same
/// thing as `alias_as::<_, 0>`.
pub fn alias_as<T: Table, const TAG: u64>(_table: T, name: &'static str) -> Alias<T, TAG> {
    Alias {
        name,
        phantom: PhantomData,
    }
}

impl<T: Table, const TAG: u64> Alias<T, TAG> {
    /// The name written after `AS`.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Re-tag this handle, keeping its name.
    ///
    /// The tag is normally chosen where the alias is created, by [`alias_as`]. This is for
    /// a handle that arrives already typed as `Alias<T>` — from a helper, or from a
    /// defaulted binding — that the caller wants to give a distinct tag to.
    ///
    /// The returned handle is a *different* alias as far as [`Contains`](crate::Contains) is
    /// concerned, even though it names the same rows in the SQL. Retagging a handle that has
    /// already been joined therefore makes its columns unreachable from that query; choose
    /// the tag before joining, not after.
    ///
    /// ```ignore
    /// let boss = alias(employees, "boss").tag::<1>();
    /// ```
    pub const fn tag<const N: u64>(self) -> Alias<T, N> {
        Alias {
            name: self.name,
            phantom: PhantomData,
        }
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
    /// The row set must contain *this* alias — same table and same tag — so a handle the
    /// statement never joined does not compile. That check is what keeps the sides of a
    /// self-join apart: `employees`, `Alias<Employee, 0>` and `Alias<Employee, 1>` are three
    /// different types in the row set even though the SQL reads one table.
    ///
    /// The result is an [`Expr`] rather than a [`Column`] because the column type carries
    /// the table it belongs to, and an alias is not a table. Being an expression, it can be
    /// compared, ordered, grouped and projected, which is everything a column can do in a
    /// query that only reads.
    pub fn column<RowSet, P, S, N>(&self, column: Column<T, S, N>) -> Expr<RowSet, S>
    where
        S: SqlType,
        N: Nullability,
        RowSet: Contains<P, Alias<T, TAG>>,
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

impl<T: Table, const TAG: u64> TableRef for Alias<T, TAG> {
    type Marker = Alias<T, TAG>;
    type Model = T::Model;

    fn into_source(self) -> From {
        self.source()
    }
}

/// Implemented for the borrowed handle as well, so a join can name the alias without
/// giving it up — the join condition usually needs the alias too.
impl<T: Table, const TAG: u64> TableRef for &Alias<T, TAG> {
    type Marker = Alias<T, TAG>;
    type Model = T::Model;

    fn into_source(self) -> From {
        self.source()
    }
}
