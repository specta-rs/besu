//! Common table expressions: `WITH` and `WITH RECURSIVE`.
//!
//! A [`Cte`] is a named query that the rest of the statement can read as if it were a table.
//! It is created from a completed `SELECT` — or a compound query — and carries its own
//! definition, so the statement that uses it, as a source, as a join, or through
//! [`with_cte`](crate::Select::with_cte), renders the `WITH` clause itself, and its
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
//!
//! A projection that is not a whole table — an aggregate, a join, a subset of columns — has
//! columns the schema does not name. [`Select::cte_named`] takes that list at the call site,
//! and [`Cte::column_named`] reads one back with a type witness:
//!
//! ```ignore
//! let totals = db.select(posts)
//!     .pick((posts.author_id, count_all()))
//!     .cte_named(
//!         "totals",
//!         (("author_id", posts.author_id), ("total", Int8)),
//!     );
//! let total = totals.column_named("total", Int8);
//! ```

use core::marker::PhantomData;

use alloc::borrow::Cow;
use alloc::vec::Vec;

use besu_core::{ColumnDef, Error, Executor};

use crate::condition::Contains;
use crate::expr::{Expr, ExprKind};
use crate::query::set::{SetOp, SetParts};
use crate::query::{CteEntry, CteRoot, CteSource, From, QueryBody, Select, SetQuery, WithClause};
use crate::types::{Column, ColumnWitness, Nullability, SqlType, TypeWitness};

/// A named query that the rest of a statement can read from.
///
/// Created by [`Select::cte`] for a whole-row `SELECT`, or by [`Select::cte_named`] and
/// [`SetQuery::cte_named`] for any other projection. See those methods for how it is built
/// and how its columns are reached.
///
/// `R` is the row type the expression decodes to — the table's model for a whole-row
/// `SELECT`, or the projection's Rust type for a named one. The handle owns its definition
/// and is cheap to clone relative to the query it holds, so one expression can be read by
/// several statements — or twice by the same statement, which is exactly what a recursive
/// definition does.
pub struct Cte<R> {
    /// The name the `WITH` clause gives the expression.
    name: &'static str,
    /// The expression's definition: a `SELECT` or a compound query.
    body: QueryBody,
    /// The columns the expression produces, in order.
    columns: Cow<'static, [ColumnDef]>,
    /// Whether the `WITH` definition spells the output column names explicitly.
    ///
    /// A whole-row expression gets its names from the table, so the list is left out; a
    /// projected one supplies names the SQL could not otherwise learn, so they are written.
    named_columns: bool,
    /// The expressions this one reads from, which must be defined before it.
    prerequisites: Option<WithClause>,
    /// A construction error, recorded on the statement that uses the expression.
    error: Option<Error>,
    phantom: PhantomData<fn() -> R>,
}

impl<R> Clone for Cte<R> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            body: self.body.clone(),
            columns: self.columns.clone(),
            named_columns: self.named_columns,
            prerequisites: self.prerequisites.clone(),
            error: self.error.clone(),
            phantom: PhantomData,
        }
    }
}

impl<R> core::fmt::Debug for Cte<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cte")
            .field("name", &self.name)
            .field("columns", &self.columns)
            .finish()
    }
}

impl<R: 'static> Cte<R> {
    /// Build a handle from a finished definition.
    pub(crate) fn new(
        name: &'static str,
        body: QueryBody,
        columns: Cow<'static, [ColumnDef]>,
        named_columns: bool,
        prerequisites: Option<WithClause>,
    ) -> Self {
        Self {
            name,
            body,
            columns,
            named_columns,
            prerequisites,
            error: None,
            phantom: PhantomData,
        }
    }

    /// Record an error that the statement using this handle will report.
    pub(crate) fn with_error(mut self, error: Option<Error>) -> Self {
        self.error = error;
        self
    }

    /// The name the `WITH` clause gives this expression.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The columns the expression produces, in order.
    pub fn columns(&self) -> &[ColumnDef] {
        &self.columns
    }

    /// The expression as a `SELECT` source.
    pub(crate) fn source(&self) -> From {
        From::Cte(CteSource {
            name: self.name,
            columns: self.columns.clone(),
            error: self.error.clone(),
        })
    }

    /// The entries a statement must define before it can read this expression: everything
    /// this one depends on, then this one.
    pub(crate) fn entries(&self) -> WithClause {
        let mut with = self.prerequisites.clone().unwrap_or(WithClause {
            recursive: false,
            entries: Vec::new(),
        });
        if with.entries.iter().all(|entry| entry.name != self.name) {
            with.entries.push(CteEntry {
                name: self.name,
                columns: self.columns.clone(),
                named_columns: self.named_columns,
                body: self.body.clone(),
                error: self.error.clone(),
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
    pub fn column<RowSet, P, T, S, N>(&self, column: Column<T, S, N>) -> Expr<RowSet, S>
    where
        S: SqlType,
        N: Nullability,
        RowSet: Contains<P, Cte<R>>,
    {
        let _ = PhantomData::<P>;
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.name,
            column: column.name(),
        })
    }

    /// One of the expression's columns, for a projection that is not a whole table.
    ///
    /// `name` is the output name supplied to [`Select::cte_named`] or
    /// [`SetQuery::cte_named`], and `witness` supplies its SQL type, exactly as
    /// [`scalar`](crate::expr::scalar) uses a witness — either the marker itself (`Int8`) or
    /// any column of the intended type (`posts.author_id`).
    ///
    /// ```ignore
    /// let totals = db.select(posts)
    ///     .pick((posts.author_id, count_all()))
    ///     .cte_named("totals", (("author_id", posts.author_id), ("total", Int8)));
    ///
    /// let author = totals.column_named("author_id", posts.author_id);
    /// let total = totals.column_named("total", Int8);
    /// ```
    ///
    /// `name` is quoted, so it does not rely on the unquoted-identifier folding each dialect
    /// applies. It is written into the SQL rather than bound, and the database reports a name
    /// the definition does not produce.
    pub fn column_named<RowSet, P, S, W>(&self, name: &'static str, witness: W) -> Expr<RowSet, S>
    where
        S: SqlType,
        W: TypeWitness<S>,
        RowSet: Contains<P, Cte<R>>,
    {
        let _ = (witness, PhantomData::<P>);
        Expr::from_kind(ExprKind::UnprefixedColumn {
            table: self.name,
            column: name,
        })
    }

    /// `UNION`: this expression's rows plus `other`'s, with duplicates removed.
    ///
    /// `other` must produce the same row type, which is what stops two differently shaped
    /// `SELECT`s from being combined.
    pub fn union<'a, E: Executor, M2>(
        self,
        other: Select<'a, E, M2, R>,
    ) -> SetQuery<'a, E, (Cte<R>,), R> {
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
        other: Select<'a, E, M2, R>,
    ) -> SetQuery<'a, E, (Cte<R>,), R> {
        self.compound(SetOp::UnionAll, other)
    }

    fn compound<'a, E: Executor, M2>(
        self,
        op: SetOp,
        other: Select<'a, E, M2, R>,
    ) -> SetQuery<'a, E, (Cte<R>,), R> {
        let Self {
            name,
            body,
            columns,
            named_columns,
            prerequisites,
            ..
        } = self;

        // A single `SELECT` becomes the first branch of a compound; an existing compound
        // gains a branch. Both end up as the same `SetParts`.
        let mut body = match body {
            QueryBody::Select(query) => SetParts::new(query),
            QueryBody::Set(parts) => parts,
        };
        body.push(op, other.query);
        // A compound built on an expression is that expression's definition, so the
        // statement reads the definition back out and every branch sees the name.
        body.set_root(CteRoot {
            name,
            columns,
            named_columns,
        });
        if let Some(prerequisites) = prerequisites {
            body.push_with(prerequisites);
        }
        SetQuery::from_parts(other.exec, body)
    }
}

/// One output column of a projected common table expression.
///
/// A value is a `(&'static str, W)` pair: the output name and a witness for the column's SQL
/// type. The witness is either the type marker itself (`Int8`, `Text`) or a column of that
/// type (`posts.author_id`), exactly as [`TypeWitness`] allows:
///
/// ```ignore
/// .cte_named("totals", (("author_id", posts.author_id), ("total", Int8)))
/// ```
///
/// A tuple of pairs is accepted — which is how differently typed columns fit in one list
/// without a trait object — as is a homogeneous array or `Vec`.
pub trait CteColumns {
    /// Turn the list into column definitions.
    #[doc(hidden)]
    fn into_columns(self) -> Vec<ColumnDef>;
}

/// Build one column definition from a name and a type witness.
fn column_def<W: ColumnWitness>(name: &'static str, witness: W) -> ColumnDef {
    let _ = witness;
    ColumnDef {
        name,
        ty: W::TYPE,
        nullable: W::NULLABLE,
        auto_increment: false,
        primary_key: false,
        unique: false,
        default: None,
        generated: None,
        renamed_from: None,
    }
}

impl<W: ColumnWitness> CteColumns for (&'static str, W) {
    fn into_columns(self) -> Vec<ColumnDef> {
        alloc::vec![column_def(self.0, self.1)]
    }
}

impl<W: ColumnWitness> CteColumns for Vec<(&'static str, W)> {
    fn into_columns(self) -> Vec<ColumnDef> {
        self.into_iter()
            .map(|(name, witness)| column_def(name, witness))
            .collect()
    }
}

impl<W: ColumnWitness, const N: usize> CteColumns for [(&'static str, W); N] {
    fn into_columns(self) -> Vec<ColumnDef> {
        self.into_iter()
            .map(|(name, witness)| column_def(name, witness))
            .collect()
    }
}

macro_rules! impl_cte_columns {
    ($($name:ident),+) => {
        impl<$($name: ColumnWitness),+> CteColumns for ($((&'static str, $name),)+) {
            #[allow(non_snake_case)]
            fn into_columns(self) -> Vec<ColumnDef> {
                let ($($name,)+) = self;
                let mut columns = Vec::new();
                $(columns.push(column_def($name.0, $name.1));)+
                columns
            }
        }
    };
}

impl_cte_columns!(A);
impl_cte_columns!(A, B);
impl_cte_columns!(A, B, C);
impl_cte_columns!(A, B, C, D);
impl_cte_columns!(A, B, C, D, E);
impl_cte_columns!(A, B, C, D, E, F);
impl_cte_columns!(A, B, C, D, E, F, G);
impl_cte_columns!(A, B, C, D, E, F, G, H);

/// Check a projected expression's name and column list.
///
/// The list has to describe the projection exactly, because it is what the handle exposes and
/// what a `SELECT` from the expression reads. The names are plain identifiers: they are quoted
/// when written, so anything else would work as SQL but is almost certainly a mistake at the
/// call site, and catching it here makes the error visible before the statement is sent.
pub(crate) fn validate_cte(
    name: &'static str,
    columns: &[ColumnDef],
    projected: usize,
) -> Option<Error> {
    if let Some(error) = validate_identifier(name, "common table expression") {
        return Some(error);
    }

    for column in columns {
        if let Some(error) = validate_identifier(column.name, "column of common table expression") {
            return Some(error);
        }
    }

    if columns.len() != projected {
        return Some(Error::invalid_query(alloc::format!(
            "the common table expression `{name}` was given {} column name(s) but its \
             projection produces {projected}",
            columns.len()
        )));
    }

    None
}

/// Reject a name that is not a plain identifier.
fn validate_identifier(name: &str, what: &str) -> Option<Error> {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_');
    if valid_start && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return None;
    }

    Some(Error::invalid_query(alloc::format!(
        "`{name}` is not a plain identifier, so it cannot be the name of a {what}; use \
         letters, digits and underscores, starting with a letter or an underscore"
    )))
}

/// Start a `SELECT` that reads a common table expression.
///
/// The expression's definition is attached to the statement, so the statement renders the
/// `WITH` clause itself and its parameters come first.
pub(crate) fn select_from<'a, R: 'static, E: Executor>(
    exec: E,
    cte: &Cte<R>,
) -> Select<'a, E, (Cte<R>,), R> {
    let mut query = crate::query::SelectQuery::from_source(cte.source());
    query.push_with(cte.entries());
    Select {
        exec,
        query,
        phantom: PhantomData,
    }
}
