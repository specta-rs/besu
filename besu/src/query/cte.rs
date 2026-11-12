//! Common table expressions: `WITH` and `WITH RECURSIVE`.
//!
//! A [`Cte`] is a named query that the rest of the statement can read as if it were a table.
//! It is created from a completed `SELECT` and carries its own definition, so the statement
//! that uses it — as a source, as a join, or through
//! [`with_cte`](crate::Select::with_cte) — renders the `WITH` clause itself, and its
//! parameters come first.
//!
//! ```ignore
//! let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
//! let rows: Vec<User> = db.from(&adults).filter(adults.column(users.age).lt(65)).all().await?;
//! ```
//!
//! ```sql
//! WITH "adults" AS (SELECT ... FROM "users" WHERE ("users"."age" >= $1))
//! SELECT "adults"."id", ... FROM "adults" WHERE ("adults"."age" < $2)
//! ```
//!
//! # Why the columns are reached through a method
//!
//! A table handle gets one field per column from `#[derive(Table)]`, because a table's
//! columns are known when the program is compiled. A common table expression's name is
//! chosen at the call site and its definition is a value, so there is nothing for the derive
//! to expand. [`Cte::column`] supplies the missing half: it takes the column of the table
//! the expression selects from and returns the same column qualified by the expression's
//! name, which is type-checked against the same Rust type.

use core::marker::PhantomData;

use besu_core::{ColumnDef, Executor};

use crate::condition::Contains;
use crate::expr::{Expr, ExprKind};
use crate::query::set::{SetOp, SetParts};
use crate::query::{CteEntry, CteRoot, CteSource, From, Select, SelectQuery, SetQuery, WithClause};
use crate::table::Table;
use crate::types::{Column, Nullability, SqlType};

/// A named query that the rest of a statement can read from.
///
/// Created by [`Select::cte`]; see [`Select::cte`] for how it is built and how its
/// columns are reached.
///
/// The handle owns its definition and is cheap to clone relative to the query it holds, so
/// one expression can be read by several statements — or twice by the same statement, which
/// is exactly what a recursive definition does.
pub struct Cte<T: Table> {
    /// The name the `WITH` clause gives the expression.
    name: &'static str,
    /// The expression's definition: one or more `SELECT`s joined by set operators.
    body: SetParts,
    /// The expressions this one reads from, which must be defined before it.
    prerequisites: Option<WithClause>,
    phantom: PhantomData<fn() -> T>,
}

impl<T: Table> Clone for Cte<T> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            body: self.body.clone(),
            prerequisites: self.prerequisites.clone(),
            phantom: PhantomData,
        }
    }
}

impl<T: Table> core::fmt::Debug for Cte<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cte")
            .field("name", &self.name)
            .field("table", &T::DEF.name)
            .finish()
    }
}

impl<T: Table> Cte<T> {
    /// Build a handle from a finished definition.
    pub(crate) fn new(
        name: &'static str,
        body: SetParts,
        prerequisites: Option<WithClause>,
    ) -> Self {
        Self {
            name,
            body,
            prerequisites,
            phantom: PhantomData,
        }
    }

    /// The name the `WITH` clause gives this expression.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The columns the expression produces, in order.
    pub fn columns(&self) -> &'static [ColumnDef] {
        T::DEF.columns
    }

    /// The expression as a `SELECT` source.
    pub(crate) fn source(&self) -> From {
        From::Cte(CteSource {
            name: self.name,
            columns: T::DEF.columns,
        })
    }

    /// The entries a statement must define before it can read this expression: everything
    /// this one depends on, then this one.
    pub(crate) fn entries(&self) -> WithClause {
        let mut with = self.prerequisites.clone().unwrap_or(WithClause {
            recursive: false,
            entries: alloc::vec::Vec::new(),
        });
        if with.entries.iter().all(|entry| entry.name != self.name) {
            with.entries.push(CteEntry {
                name: self.name,
                columns: T::DEF.columns,
                body: self.body.clone(),
            });
        }
        with
    }

    /// One of the expression's columns, typed like the column it came from.
    ///
    /// The argument is the column handle of the table the expression selects from, so the
    /// Rust type, the SQL type and the nullability are the ones the schema declares. The
    /// returned expression is qualified by the expression's name instead of the table's:
    ///
    /// ```ignore
    /// let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    /// db.from(adults).filter(adults.column(users.age).lt(65))
    /// ```
    ///
    /// The row set must contain this expression, so using a handle the statement never
    /// defined or joined does not compile. A presence check is all the compiler can do: two
    /// expressions over the same table are different types only because the handles are
    /// different values of the same type, so the compiler cannot tell them apart.
    pub fn column<RowSet, P, S: SqlType, N: Nullability>(
        &self,
        column: Column<T, S, N>,
    ) -> Expr<RowSet, S>
    where
        RowSet: Contains<P, Cte<T>>,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.name,
            column: column.name(),
        })
    }

    /// `UNION`: this expression's rows plus `other`'s, with duplicates removed.
    ///
    /// `other` must produce the same row type, which is what stops two differently shaped
    /// `SELECT`s from being combined.
    pub fn union<'a, E: Executor, M2>(
        self,
        other: Select<'a, E, M2, T::Model>,
    ) -> SetQuery<'a, E, (Cte<T>,), T::Model> {
        self.compound(SetOp::Union, other)
    }

    /// `UNION ALL`: this expression's rows plus `other`'s, keeping duplicates.
    ///
    /// This is the recursive form: the right-hand branch may read the expression itself,
    /// and the statement renders `WITH RECURSIVE`.
    ///
    /// ```ignore
    /// let tree = db.select(nodes).filter(nodes.parent_id.is_null()).cte("tree");
    /// let rows: Vec<Node> = tree
    ///     .clone()
    ///     .union_all(
    ///         db.select(nodes)
    ///             .inner_join_cte(&tree, nodes.parent_id.eq(tree.column(nodes.id))),
    ///     )
    ///     .all()
    ///     .await?;
    /// ```
    pub fn union_all<'a, E: Executor, M2>(
        self,
        other: Select<'a, E, M2, T::Model>,
    ) -> SetQuery<'a, E, (Cte<T>,), T::Model> {
        self.compound(SetOp::UnionAll, other)
    }

    fn compound<'a, E: Executor, M2>(
        self,
        op: SetOp,
        other: Select<'a, E, M2, T::Model>,
    ) -> SetQuery<'a, E, (Cte<T>,), T::Model> {
        let mut body = self.body;
        body.push(op, other.query);
        // A compound built on an expression is that expression's definition, so the
        // statement reads the definition back out and every branch sees the name.
        body.set_root(CteRoot {
            name: self.name,
            columns: T::DEF.columns,
        });
        if let Some(prerequisites) = self.prerequisites {
            body.push_with(prerequisites);
        }
        SetQuery::from_parts(other.exec, body)
    }
}

/// Start a `SELECT` that reads a common table expression.
///
/// The expression's definition is attached to the statement, so the statement renders the
/// `WITH` clause itself and its parameters come first.
pub(crate) fn select_from<'a, T: Table, E: Executor>(
    exec: E,
    cte: &Cte<T>,
) -> Select<'a, E, (Cte<T>,), T::Model> {
    let mut query = SelectQuery::from_source(cte.source());
    query.push_with(cte.entries());
    Select {
        exec,
        query,
        phantom: PhantomData,
    }
}
