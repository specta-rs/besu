//! Eager loading for has-many relations.
//!
//! A join returns a tuple — `(User, Option<Post>)` — which is the honest shape for one
//! statement, but it is not a `User` with its posts. The same parent row is repeated once per
//! child, so the parent columns travel over the wire more than once, and grouping the rows back
//! into objects is left to the caller.
//!
//! [`Select::with`] instead loads a relation with a second, visible statement:
//!
//! ```sql
//! SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio", "users"."id"
//!   FROM "users" WHERE ("users"."age" > $1);
//! SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id"
//!   FROM "posts" WHERE ("posts"."author_id" IN ($1, $2, $3));
//! ```
//!
//! The parent statement selects the same columns it would have selected on its own, plus its
//! key. The child statement selects every declared child column and then the foreign key
//! again. The repetition is deliberate: the model consumes its columns in declaration order,
//! and the trailing copy is what the loader reads the key from, so the grouping works without
//! a hand-written decoder for every child type. It costs one column per child row, never a
//! duplicated parent row. Both statements are ordinary SQL that you could have written by
//! hand, which is the whole point: the query you write is the SQL that runs, and nothing is
//! smuggled in behind a join. They are built and run independently, so each numbers its own
//! placeholders from the start.
//!
//! This is not N+1. The child statement runs once for the whole parent result, no matter how
//! many parents there are, and it does not run at all when the parent result is empty — an
//! `IN ()` is a syntax error on some databases, and an empty query is not worth a round trip.
//!
//! # What is not modelled
//!
//! A relation is described entirely by the two columns it joins on, so the schema layer does
//! not change: there is no relation list on [`TableDef`], and no lookup by name. That is enough
//! for has-many. Inserting a parent and its children together, and many-to-many through a join
//! table, are not modelled; both need more than one column pair and would change the shape of
//! the API.
//!
//! Children arrive in the order the database returned them. Add an `ORDER BY` to the child
//! statement by hand if the order matters — this API deliberately does not invent one.
//!
//! # Example
//!
//! ```ignore
//! use besu::prelude::*;
//!
//! let users_with_posts: Vec<(User, Vec<Post>)> = db
//!     .select(users)
//!     .filter(users.age.gt(18))
//!     .with(has_many(users.id, posts.author_id))
//!     .await?;
//! ```

use core::{
    future::{Future, IntoFuture},
    hash::Hash,
    marker::PhantomData,
    pin::Pin,
};
use std::collections::{HashMap, HashSet};

use alloc::sync::Arc;
use alloc::vec::Vec;

use besu_core::{Decode, Dialect, DialectOf, Driver, ErrorOf, Executor, Sql, TableDef, Value};

use crate::expr::ExprKind;
use crate::query::{Select, SelectQuery};
use crate::table::Table;
use crate::types::{Column, NonNull, SqlType};

/// A has-many relation: one parent row owns any number of child rows.
///
/// Construct it with [`has_many`], from the parent's key column and the child's foreign key.
/// The two columns must have the same SQL type, which is what makes the parent key usable as
/// the value list in the child's `IN` clause.
///
/// `P` is the parent table, `C` the child table and `S` the shared column type; all three are
/// inferred from the columns, so a relation value is normally written inline:
///
/// ```ignore
/// db.select(users).with(has_many(users.id, posts.author_id))
/// ```
pub struct HasMany<P, C, S: SqlType> {
    parent_table: &'static str,
    parent_column: &'static str,
    child_table: &'static str,
    child_column: &'static str,
    phantom: PhantomData<fn() -> (P, C, S)>,
}

impl<P, C, S: SqlType> HasMany<P, C, S> {
    /// The parent key, as a renderable column reference.
    fn parent_key(&self) -> ExprKind {
        ExprKind::Column {
            table: self.parent_table,
            column: self.parent_column,
        }
    }

    /// The child foreign key, as a renderable column reference.
    fn child_key(&self) -> ExprKind {
        ExprKind::Column {
            table: self.child_table,
            column: self.child_column,
        }
    }
}

/// Describe a has-many relation from the parent's key column to the child's foreign key.
///
/// ```ignore
/// // One user has many posts: `users.id` is the key, `posts.author_id` the foreign key.
/// db.select(users).with(has_many(users.id, posts.author_id))
/// ```
///
/// The parent column is treated as a key: it should be unique, as a primary key or unique
/// column is, because it is the value the children are grouped by.
///
/// The relation is not registered anywhere and does not touch the schema. It is just the pair
/// of columns, which is enough to run the batched second query described in the
/// [module documentation](self).
pub fn has_many<P, C, S>(
    parent: Column<P, S, NonNull>,
    child: Column<C, S, NonNull>,
) -> HasMany<P, C, S>
where
    P: Table,
    C: Table,
    S: SqlType,
{
    HasMany {
        parent_table: P::DEF.name,
        parent_column: parent.name(),
        child_table: C::DEF.name,
        child_column: child.name(),
        phantom: PhantomData,
    }
}

/// A `SELECT` that also eagerly loads a has-many relation.
///
/// Created by [`Select::with`]; awaiting it yields `Vec<(R, Vec<C::Model>)>` — one entry per
/// parent row, in the parent query's order, and a `Vec` of children for each. A parent with no
/// children gets an empty `Vec`, never a missing entry.
///
/// Two statements run, both of them ordinary SQL:
///
/// 1. the parent query, with its key column appended to the projection;
/// 2. the child query, selecting every child column plus the foreign key, with
///    `foreign_key IN (parent keys)`.
///
/// The keys are deduplicated before the second statement, so a hundred parents with three
/// distinct keys bind three parameters, not a hundred.
pub struct WithRelation<'a, E: Executor, R, C, S: SqlType> {
    exec: E,
    parent: SelectQuery,
    child: &'static TableDef,
    child_key: ExprKind,
    phantom: PhantomData<(&'a (), fn() -> (R, C, S))>,
}

impl<'a, E: Executor, T: Table, R> Select<'a, E, (T,), R> {
    /// Load one relation for every selected row, in one extra statement.
    ///
    /// ```ignore
    /// let users_with_posts: Vec<(User, Vec<Post>)> = db
    ///     .select(users)
    ///     .filter(users.age.gt(18))
    ///     .with(has_many(users.id, posts.author_id))
    ///     .await?;
    /// ```
    ///
    /// The parent query keeps its filters, ordering, limit and projection; `with` only appends
    /// the parent key to what it selects, so the rows can be matched to their children. The key
    /// is not part of `R`: `pick(users.name)` still yields `Vec<(String, Vec<Post>)>`.
    ///
    /// The second statement is a separate round trip. It is not hidden, it is not a join, and
    /// it does not run when the parent result is empty. See the
    /// [module documentation](self) for why that trade is the right one.
    pub fn with<C: Table, S: SqlType>(
        self,
        relation: HasMany<T, C, S>,
    ) -> WithRelation<'a, E, R, C, S> {
        let Select {
            exec,
            mut query,
            phantom: _,
        } = self;

        // `None` means "every column of the base table". The key has to be appended either
        // way, and appending to a `None` projection means spelling the list out, exactly as
        // the builder would have rendered it.
        let mut projection = match query.projection.take() {
            Some(projection) => projection,
            None => T::DEF
                .columns
                .iter()
                .map(|column| ExprKind::Column {
                    table: T::DEF.name,
                    column: column.name,
                })
                .collect(),
        };
        projection.push(relation.parent_key());
        query.projection = Some(projection);

        WithRelation {
            exec,
            parent: query,
            child: C::DEF,
            child_key: relation.child_key(),
            phantom: PhantomData,
        }
    }
}

impl<'a, E, R, C, S> WithRelation<'a, E, R, C, S>
where
    E: Executor,
    C: Table,
    S: SqlType,
    S::Rust: Into<Value>,
{
    /// Render both statements this load will run, without running either.
    ///
    /// The parent statement is the query as it stands with the key appended. The child
    /// statement is rendered with `keys` bound into its `IN` list; a real load takes those
    /// keys from the parent rows, so this is for seeing exactly what will run.
    ///
    /// An empty key list renders `IN ()`, which this method will happily do for inspection;
    /// an actual load skips the second statement entirely instead, because that SQL is not
    /// portable.
    pub fn to_sql(
        &self,
        keys: impl IntoIterator<Item = S::Rust>,
    ) -> (Sql<DialectOf<E>>, Sql<DialectOf<E>>) {
        (self.parent_sql().seal(), self.child_sql(keys).seal())
    }

    /// The parent statement, with the key appended to the projection.
    fn parent_sql(&self) -> Sql<DialectOf<E>> {
        render_parent(&self.parent, self.exec.table_prefix())
    }

    /// The child statement, selecting every child column plus the foreign key.
    fn child_sql(&self, keys: impl IntoIterator<Item = S::Rust>) -> Sql<DialectOf<E>> {
        render_child(self.child, &self.child_key, keys, self.exec.table_prefix())
    }
}

/// Render the parent half of a relation load.
fn render_parent<D: Dialect>(query: &SelectQuery, prefix: Option<Arc<str>>) -> Sql<D> {
    // Cloned because `to_sql` leaves the relation usable, and because rendering moves the
    // bound values into the statement.
    query.clone().build::<D>(prefix)
}

/// Render the child half of a relation load.
fn render_child<D: Dialect, K: Into<Value>>(
    child: &'static TableDef,
    child_key: &ExprKind,
    keys: impl IntoIterator<Item = K>,
    prefix: Option<Arc<str>>,
) -> Sql<D> {
    let mut query = SelectQuery::new(child);

    // The foreign key is what lets the rows be grouped back onto their parent, so it is
    // selected alongside the declared columns.
    let mut projection = Vec::with_capacity(child.columns.len() + 1);
    for column in child.columns {
        projection.push(ExprKind::Column {
            table: child.name,
            column: column.name,
        });
    }
    projection.push(child_key.clone());
    query.projection = Some(projection);

    query.filter.push(ExprKind::InList {
        expr: crate::expr::Operand::new(child_key.clone()),
        list: keys
            .into_iter()
            .map(|key| ExprKind::Value(key.into()))
            .collect(),
        negated: false,
    });

    query.build::<D>(prefix)
}

impl<'a, E, R, C, S> WithRelation<'a, E, R, C, S>
where
    E: Executor,
    R: Decode<E::Driver>,
    C: Table,
    C::Model: Decode<E::Driver>,
    S: SqlType,
    S::Rust: Decode<E::Driver> + Clone + Eq + Hash + Into<Value>,
{
    async fn load(self) -> Result<Vec<(R, Vec<C::Model>)>, ErrorOf<E>> {
        let WithRelation {
            exec,
            parent,
            child,
            child_key,
            phantom: _,
        } = self;
        let mut exec = exec;

        let parent = render_parent::<DialectOf<E>>(&parent, exec.table_prefix())
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&parent).await?;

        // One extra column rides along with the parent row: its key. Decoding the pair means
        // the key survives even when `R` is a `pick` that does not include it.
        let mut parents: Vec<(R, S::Rust)> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut offset = 0;
            parents.push(<(R, S::Rust)>::decode(row, &mut offset)?);
        }

        // Deduplicate in first-seen order. The order only affects the parameter list; the
        // result is rebuilt in the parent's order below.
        let mut seen: HashSet<S::Rust> = HashSet::with_capacity(parents.len());
        let mut keys: Vec<S::Rust> = Vec::new();
        for (_, key) in &parents {
            if seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        }

        // Nothing to load, and `IN ()` is a syntax error on some databases. Do not send a
        // statement at all.
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let child = render_child::<DialectOf<E>, _>(
            child,
            &child_key,
            keys.iter().cloned(),
            exec.table_prefix(),
        )
        .finish()
        .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&child).await?;

        let mut grouped: HashMap<S::Rust, Vec<C::Model>> = HashMap::with_capacity(keys.len());
        for row in &rows {
            let mut offset = 0;
            let (model, key) = <(C::Model, S::Rust)>::decode(row, &mut offset)?;
            grouped.entry(key).or_default().push(model);
        }

        // Rebuild the parent order, giving every parent its own `Vec` and an empty one when
        // the child statement found nothing for it.
        Ok(parents
            .into_iter()
            .map(|(parent, key)| {
                let children = grouped.remove(&key).unwrap_or_default();
                (parent, children)
            })
            .collect())
    }
}

impl<'a, E, R, C, S> IntoFuture for WithRelation<'a, E, R, C, S>
where
    E: Executor + Send + 'a,
    R: Decode<E::Driver> + Send + 'a,
    C: Table + 'a,
    C::Model: Decode<E::Driver> + 'a,
    S: SqlType + 'a,
    S::Rust: Decode<E::Driver> + Clone + Eq + Hash + Into<Value> + Send + 'a,
{
    type Output = Result<Vec<(R, Vec<C::Model>)>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.load())
    }
}
