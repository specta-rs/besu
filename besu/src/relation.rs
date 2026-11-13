//! Eager loading for relations.
//!
//! A relation is a value: `has_many(users.id, posts.author_id)` is a `HasMany` struct, and
//! `many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id)` is a
//! `ManyToMany`. Both are plain data — the columns' table and column names — so a relation
//! can be named, stored in a `const` or `static`, copied and reused:
//!
//! ```ignore
//! const POSTS: HasMany<UserTable, PostTable, Int8> = has_many(users.id, posts.author_id);
//!
//! db.select(users).with(POSTS).await?;
//! // `POSTS` is `Copy`, so passing it does not consume it.
//! db.select(users).with(POSTS).await?;
//! ```
//!
//! A join returns a tuple — `(User, Option<Post>)` — which is the honest shape for one
//! statement, but it is not a `User` with its posts. The same parent row is repeated once per
//! child, so the parent columns travel over the wire more than once, and grouping the rows
//! back into objects is left to the caller.
//!
//! [`Select::with`] instead loads a relation with further, visible statements:
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
//! This is not N+1. Each statement runs once for the whole level above it, no matter how
//! many rows there are, and a statement does not run at all when the level above it is empty:
//! there are no keys to bind, and an empty query is not worth a round trip.
//!
//! # Many-to-many
//!
//! A join table is a relation too: [`many_to_many`] takes the parent key, the two foreign
//! keys on the join table, and the child key. It loads in three statements — parent rows,
//! the join rows for those parents, then the child rows for those join rows:
//!
//! ```sql
//! SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."id" FROM "posts";
//! SELECT "post_tags"."post_id", "post_tags"."tag_id" FROM "post_tags"
//!   WHERE ("post_tags"."post_id" IN ($1, $2));
//! SELECT "tags"."id", "tags"."name", "tags"."id" FROM "tags"
//!   WHERE ("tags"."id" IN ($1, $2, $3));
//! ```
//!
//! One child row can belong to several parents, so a many-to-many relation requires the
//! child model to be `Clone`: each parent owns its own `Vec` of models, and a shared child
//! is duplicated once per parent that references it. Parent rows are never cloned.
//!
//! # Nested relations
//!
//! A relation can carry another relation for the child table. The result type follows the
//! shape of the tree, and each level is one statement:
//!
//! ```ignore
//! let authors: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
//!     .select(users)
//!     .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
//!     .await?;
//! ```
//!
//! Each level decodes its own statement and groups it by key; a parent with nothing at any
//! level gets an empty `Vec` there. A statement is only sent when the level above it
//! produced at least one key.
//!
//! # What is not modelled
//!
//! A relation is described entirely by the columns it joins on, so the schema layer does not
//! change: there is no relation list on [`TableDef`], and no lookup by name. Per-parent
//! limits and ordering, and inserting a parent and its children together, are not part of
//! this API; they need a different statement shape rather than another column pair. There is
//! no `belongs_to` (a child does not load its parent), and a many-to-many yields the child
//! models rather than the join rows, so a join table with data of its own is queried directly.
//!
//! Children arrive in the order the database returned them. Add an `ORDER BY` to the child
//! statement by hand if the order matters — this API deliberately does not invent one.
//!
//! # What the compiler checks
//!
//! Both columns of a has-many must be `NonNull` and share one [`SqlType`], and the two join
//! columns of a many-to-many must belong to the same join table. A relation must hang off the
//! table being selected. Each is a compile error rather than a check at load time.
//!
//! A nullable foreign key cannot be the child side of a relation:
//!
//! ```compile_fail
//! use besu::prelude::*;
//!
//! #[derive(Table)]
//! #[besu(table = "users")]
//! struct User {
//!     #[besu(primary_key)]
//!     id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "posts")]
//! struct Post {
//!     #[besu(primary_key)]
//!     id: i64,
//!     // `Option<i64>` is a nullable column, and a relation key is not nullable.
//!     author_id: Option<i64>,
//! }
//!
//! let _ = has_many(users.id, posts.author_id);
//! ```
//!
//! A key and a foreign key of different SQL types cannot be paired:
//!
//! ```compile_fail
//! use besu::prelude::*;
//!
//! #[derive(Table)]
//! #[besu(table = "users")]
//! struct User {
//!     #[besu(primary_key)]
//!     id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "posts")]
//! struct Post {
//!     #[besu(primary_key)]
//!     id: i64,
//!     // `i32` is `INT4`; `users.id` is `INT8`.
//!     author_id: i32,
//! }
//!
//! let _ = has_many(users.id, posts.author_id);
//! ```
//!
//! Both columns of a many-to-many must be on the join table:
//!
//! ```compile_fail
//! use besu::prelude::*;
//!
//! #[derive(Table)]
//! #[besu(table = "posts")]
//! struct Post {
//!     #[besu(primary_key)]
//!     id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "post_tags")]
//! struct PostTag {
//!     #[besu(primary_key)]
//!     post_id: i64,
//!     #[besu(primary_key)]
//!     tag_id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "other_tags")]
//! struct OtherTag {
//!     #[besu(primary_key)]
//!     tag_id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "tags")]
//! struct Tag {
//!     #[besu(primary_key)]
//!     id: i64,
//! }
//!
//! // `other_tags.tag_id` is on a different join table from `post_tags.post_id`.
//! let _ = many_to_many(posts.id, post_tags.post_id, other_tags.tag_id, tags.id);
//! ```
//!
//! A relation can only be loaded where its parent table is the selected one:
//!
//! ```compile_fail
//! use besu::prelude::*;
//!
//! #[derive(Table)]
//! #[besu(table = "users")]
//! struct User {
//!     #[besu(primary_key)]
//!     id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "posts")]
//! struct Post {
//!     #[besu(primary_key)]
//!     id: i64,
//!     author_id: i64,
//! }
//!
//! #[derive(Table)]
//! #[besu(table = "comments")]
//! struct Comment {
//!     #[besu(primary_key)]
//!     id: i64,
//!     post_id: i64,
//! }
//!
//! fn load<'a, E: Executor, R>(query: Select<'a, E, (UserTable,), R>) {
//!     // The relation's parent is `posts`, but the query selects `users`.
//!     let _ = query.with(has_many(posts.id, comments.post_id));
//! }
//! ```
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
    fmt,
    future::{Future, IntoFuture},
    hash::Hash,
    marker::PhantomData,
    pin::Pin,
};
use std::collections::{HashMap, HashSet};

use alloc::sync::Arc;
use alloc::vec::Vec;

use besu_core::{Decode, Dialect, DialectOf, Driver, ErrorOf, Executor, Sql, TableDef, Value};

use crate::expr::{ExprKind, Operand};
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
///
/// `K` is the relation's own children, the unit type when there are none. See
/// [`HasMany::with`].
pub struct HasMany<P, C, S: SqlType, K = ()> {
    parent: &'static TableDef,
    parent_column: &'static str,
    child: &'static TableDef,
    child_column: &'static str,
    children: K,
    phantom: PhantomData<fn() -> (P, C, S)>,
}

impl<P, C, S: SqlType, K> HasMany<P, C, S, K> {
    /// The parent key, as a renderable column reference.
    fn parent_ref(&self) -> ExprKind {
        column_ref(self.parent.name, self.parent_column)
    }

    /// The child foreign key, as a renderable column reference.
    fn child_ref(&self) -> ExprKind {
        column_ref(self.child.name, self.child_column)
    }
}

impl<P, C, S: SqlType, K: Clone> Clone for HasMany<P, C, S, K> {
    fn clone(&self) -> Self {
        Self {
            parent: self.parent,
            parent_column: self.parent_column,
            child: self.child,
            child_column: self.child_column,
            children: self.children.clone(),
            phantom: PhantomData,
        }
    }
}

impl<P, C, S: SqlType, K: Copy> Copy for HasMany<P, C, S, K> {}

impl<P, C, S: SqlType, K> fmt::Debug for HasMany<P, C, S, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HasMany")
            .field(
                "parent",
                &format_args!("{}.{}", self.parent.name, self.parent_column),
            )
            .field(
                "child",
                &format_args!("{}.{}", self.child.name, self.child_column),
            )
            .finish_non_exhaustive()
    }
}

impl<P, C, S> HasMany<P, C, S, ()>
where
    P: Table,
    C: Table,
    S: SqlType,
{
    /// Attach a relation for the child table, making this a nested load.
    ///
    /// ```ignore
    /// let authors: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
    ///     .select(users)
    ///     .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
    ///     .await?;
    /// ```
    ///
    /// Each level is one statement. The result nests the child payloads beside the child
    /// model, exactly the way the relation values nest. This is a `const fn`, so a whole
    /// tree can live in a `const`:
    ///
    /// ```ignore
    /// const AUTHORED: HasMany<UserTable, PostTable, Int8, Nested<HasMany<PostTable, CommentTable, Int8>>> =
    ///     has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id));
    /// ```
    pub const fn with<N>(self, children: N) -> HasMany<P, C, S, Nested<N>>
    where
        N: Relation<Parent = C>,
    {
        HasMany {
            parent: self.parent,
            parent_column: self.parent_column,
            child: self.child,
            child_column: self.child_column,
            children: Nested(children),
            phantom: PhantomData,
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
/// [module documentation](self). This is a `const fn`, so the value can be named:
///
/// ```ignore
/// const POSTS: HasMany<UserTable, PostTable, Int8> = has_many(users.id, posts.author_id);
/// ```
pub const fn has_many<P, C, S>(
    parent: Column<P, S, NonNull>,
    child: Column<C, S, NonNull>,
) -> HasMany<P, C, S>
where
    P: Table,
    C: Table,
    S: SqlType,
{
    HasMany {
        parent: P::DEF,
        parent_column: parent.name(),
        child: C::DEF,
        child_column: child.name(),
        children: (),
        phantom: PhantomData,
    }
}

/// A many-to-many relation through a join table.
///
/// Construct it with [`many_to_many`], from the parent key, the join table's two foreign
/// keys and the child key. Every pair of joined columns must share one SQL type, and the two
/// join columns must be on the same join table; both are checked by the compiler.
///
/// `P` is the parent table, `J` the join table, `C` the child table, `S` the parent-side key
/// type and `T` the child-side key type. `K` is the relation's own children, `()` when there
/// are none. See [`ManyToMany::with`].
pub struct ManyToMany<P, J, C, S: SqlType, T: SqlType, K = ()> {
    parent: &'static TableDef,
    parent_column: &'static str,
    join: &'static TableDef,
    join_parent_column: &'static str,
    join_child_column: &'static str,
    child: &'static TableDef,
    child_column: &'static str,
    children: K,
    phantom: PhantomData<fn() -> (P, J, C, S, T)>,
}

impl<P, J, C, S: SqlType, T: SqlType, K> ManyToMany<P, J, C, S, T, K> {
    fn parent_ref(&self) -> ExprKind {
        column_ref(self.parent.name, self.parent_column)
    }

    fn join_parent_ref(&self) -> ExprKind {
        column_ref(self.join.name, self.join_parent_column)
    }

    fn join_child_ref(&self) -> ExprKind {
        column_ref(self.join.name, self.join_child_column)
    }

    fn child_ref(&self) -> ExprKind {
        column_ref(self.child.name, self.child_column)
    }
}

impl<P, J, C, S: SqlType, T: SqlType, K: Clone> Clone for ManyToMany<P, J, C, S, T, K> {
    fn clone(&self) -> Self {
        Self {
            parent: self.parent,
            parent_column: self.parent_column,
            join: self.join,
            join_parent_column: self.join_parent_column,
            join_child_column: self.join_child_column,
            child: self.child,
            child_column: self.child_column,
            children: self.children.clone(),
            phantom: PhantomData,
        }
    }
}

impl<P, J, C, S: SqlType, T: SqlType, K: Copy> Copy for ManyToMany<P, J, C, S, T, K> {}

impl<P, J, C, S: SqlType, T: SqlType, K> fmt::Debug for ManyToMany<P, J, C, S, T, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManyToMany")
            .field(
                "parent",
                &format_args!("{}.{}", self.parent.name, self.parent_column),
            )
            .field(
                "join",
                &format_args!(
                    "{}.{} -> {}.{}",
                    self.join.name, self.join_parent_column, self.join.name, self.join_child_column
                ),
            )
            .field(
                "child",
                &format_args!("{}.{}", self.child.name, self.child_column),
            )
            .finish_non_exhaustive()
    }
}

impl<P, J, C, S, T> ManyToMany<P, J, C, S, T, ()>
where
    P: Table,
    J: Table,
    C: Table,
    S: SqlType,
    T: SqlType,
{
    /// Attach a relation for the child table, making this a nested load.
    ///
    /// The child table is the one the join table points at, so the nested relation hangs off
    /// it exactly as it would under a [`HasMany`].
    pub const fn with<N>(self, children: N) -> ManyToMany<P, J, C, S, T, Nested<N>>
    where
        N: Relation<Parent = C>,
    {
        ManyToMany {
            parent: self.parent,
            parent_column: self.parent_column,
            join: self.join,
            join_parent_column: self.join_parent_column,
            join_child_column: self.join_child_column,
            child: self.child,
            child_column: self.child_column,
            children: Nested(children),
            phantom: PhantomData,
        }
    }
}

/// Describe a many-to-many relation through a join table.
///
/// ```ignore
/// // A post has many tags, through `post_tags`.
/// let tags = many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id);
/// let rows: Vec<(Post, Vec<Tag>)> = db.select(posts).with(tags).await?;
/// ```
///
/// The parent key and the join table's parent foreign key must share one SQL type, and the
/// join table's child foreign key and the child key must share the other. The two join
/// columns must belong to the same table. Nothing checks that the join rows are unique: a
/// duplicated `(parent, child)` pair is folded into one child, because the result is a `Vec`
/// of distinct child rows.
///
/// Because one child can belong to several parents, the child model must be `Clone` when the
/// relation is loaded; each parent receives its own copy. This is a `const fn`, so the value
/// can be named.
pub const fn many_to_many<P, J, C, S, T>(
    parent: Column<P, S, NonNull>,
    join_parent: Column<J, S, NonNull>,
    join_child: Column<J, T, NonNull>,
    child: Column<C, T, NonNull>,
) -> ManyToMany<P, J, C, S, T>
where
    P: Table,
    J: Table,
    C: Table,
    S: SqlType,
    T: SqlType,
{
    ManyToMany {
        parent: P::DEF,
        parent_column: parent.name(),
        join: J::DEF,
        join_parent_column: join_parent.name(),
        join_child_column: join_child.name(),
        child: C::DEF,
        child_column: child.name(),
        children: (),
        phantom: PhantomData,
    }
}

/// A relation nested under another relation.
///
/// This only exists to name the type of a nested relation value; construct it with
/// [`HasMany::with`] or [`ManyToMany::with`] rather than directly.
pub struct Nested<R>(R);

impl<R: Clone> Clone for Nested<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<R: Copy> Copy for Nested<R> {}

impl<R> fmt::Debug for Nested<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Nested").finish_non_exhaustive()
    }
}

/// A relation value that [`Select::with`] can load.
///
/// Implemented by [`HasMany`] and [`ManyToMany`]. The associated types describe the shape of
/// the loaded result: [`Relation::Parent`] is the table the relation hangs off,
/// [`Relation::Key`] the SQL type of its key, and [`Relation::Item`] the payload each parent
/// row receives a `Vec` of.
///
/// This is mostly a bound rather than something to implement: a relation is described by the
/// columns it joins on, which is why the constructor functions take columns rather than a
/// name. Implementing it by hand is possible but the loader traits are an implementation
/// detail.
pub trait Relation: Sync + 'static {
    /// The table whose rows the relation attaches to.
    type Parent: Table;

    /// The SQL type of the key that identifies one parent row.
    type Key: SqlType;

    /// The payload attached to one child row.
    ///
    /// This is the child model for a leaf relation, and `(Child, ...)` for a nested one.
    type Item: Send + 'static;

    /// The parent key column, as a renderable reference.
    fn parent_key(&self) -> ExprKind;

    /// Render this relation's statements, consuming one key list per level.
    #[doc(hidden)]
    fn sql<D: Dialect>(
        &self,
        prefix: Option<Arc<str>>,
        keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>>;
}

/// The loading half of a [`Relation`], for one driver.
///
/// Separate from [`Relation`] because decoding a model is a driver operation, and the
/// method's bounds would otherwise have to name the child model's type.
#[doc(hidden)]
pub trait RelationLoader<DriverImpl: Driver>: Relation {
    /// Load the relation for `keys`, grouped by the key each group belongs to.
    ///
    /// The returned map only contains keys that had children; the caller supplies an empty
    /// `Vec` for the rest. The statement is not sent when `keys` is empty.
    fn load<'e, E>(
        &'e self,
        exec: &'e mut E,
        keys: Vec<<Self::Key as SqlType>::Rust>,
    ) -> impl Future<
        Output = Result<HashMap<<Self::Key as SqlType>::Rust, Vec<Self::Item>>, ErrorOf<E>>,
    > + Send
    + 'e
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        <Self::Key as SqlType>::Rust:
            Decode<DriverImpl> + Clone + Eq + Hash + Into<Value> + Send + 'e;
}

/// How a relation's children are shaped and loaded: nothing, or another relation.
///
/// Implemented for `()` and [`Nested`]; not meant to be implemented outside this crate.
#[doc(hidden)]
pub trait RelationChildren<C: Table>: Sync + 'static {
    /// The value read from a child row that keys the next level.
    type Link: Clone + Eq + Hash + Send + 'static;

    /// What the next level loads for one link value.
    type Nested: Default + Send + 'static;

    /// The payload attached to one child row.
    type Item: Send + 'static;

    /// The column appended to the child statement for the next level, if any.
    fn link(&self) -> Option<ExprKind>;

    /// Combine a child model with the next level's payloads for it.
    fn assemble(&self, model: C::Model, nested: Self::Nested) -> Self::Item;

    /// Render the next level's statements.
    fn sql<D: Dialect>(
        &self,
        prefix: Option<Arc<str>>,
        keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>>;
}

/// The loading half of [`RelationChildren`], for one driver.
#[doc(hidden)]
pub trait RelationChildrenLoader<C: Table, DriverImpl: Driver>: RelationChildren<C> {
    /// Decode the next level's key from a child row, advancing `offset`.
    fn decode_link(
        &self,
        row: &DriverImpl::Row,
        offset: &mut usize,
    ) -> Result<Self::Link, DriverImpl::Error>
    where
        Self::Link: Decode<DriverImpl>;

    /// Load the next level for the given link values.
    fn load<'e, E>(
        &'e self,
        exec: &'e mut E,
        links: Vec<Self::Link>,
    ) -> impl Future<Output = Result<HashMap<Self::Link, Self::Nested>, ErrorOf<E>>> + Send + 'e
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        Self::Link: Decode<DriverImpl>;
}

impl<C: Table> RelationChildren<C> for ()
where
    C::Model: Send,
{
    type Link = ();
    type Nested = ();
    type Item = C::Model;

    fn link(&self) -> Option<ExprKind> {
        None
    }

    fn assemble(&self, model: C::Model, (): ()) -> C::Model {
        model
    }

    fn sql<D: Dialect>(
        &self,
        _prefix: Option<Arc<str>>,
        _keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>> {
        Vec::new()
    }
}

impl<C: Table, DriverImpl: Driver> RelationChildrenLoader<C, DriverImpl> for ()
where
    C::Model: Send,
{
    fn decode_link(
        &self,
        _row: &DriverImpl::Row,
        _offset: &mut usize,
    ) -> Result<Self::Link, DriverImpl::Error> {
        Ok(())
    }

    async fn load<'e, E>(
        &'e self,
        _exec: &'e mut E,
        _links: Vec<Self::Link>,
    ) -> Result<HashMap<Self::Link, Self::Nested>, ErrorOf<E>>
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        Self::Link: Decode<DriverImpl>,
    {
        Ok(HashMap::new())
    }
}

impl<C, N> RelationChildren<C> for Nested<N>
where
    C: Table,
    C::Model: Send,
    N: Relation<Parent = C>,
    <N::Key as SqlType>::Rust: Clone + Eq + Hash + Send,
{
    type Link = <N::Key as SqlType>::Rust;
    type Nested = Vec<N::Item>;
    type Item = (C::Model, Vec<N::Item>);

    fn link(&self) -> Option<ExprKind> {
        Some(self.0.parent_key())
    }

    fn assemble(&self, model: C::Model, nested: Vec<N::Item>) -> Self::Item {
        (model, nested)
    }

    fn sql<D: Dialect>(
        &self,
        prefix: Option<Arc<str>>,
        keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>> {
        self.0.sql(prefix, keys)
    }
}

impl<C, N, DriverImpl> RelationChildrenLoader<C, DriverImpl> for Nested<N>
where
    C: Table,
    C::Model: Send,
    DriverImpl: Driver,
    N: RelationLoader<DriverImpl, Parent = C>,
    <N::Key as SqlType>::Rust: Clone + Eq + Hash + Send,
{
    fn decode_link(
        &self,
        row: &DriverImpl::Row,
        offset: &mut usize,
    ) -> Result<Self::Link, DriverImpl::Error>
    where
        Self::Link: Decode<DriverImpl>,
    {
        Self::Link::decode(row, offset)
    }

    async fn load<'e, E>(
        &'e self,
        exec: &'e mut E,
        links: Vec<Self::Link>,
    ) -> Result<HashMap<Self::Link, Self::Nested>, ErrorOf<E>>
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        Self::Link: Decode<DriverImpl>,
    {
        self.0.load(exec, links).await
    }
}

impl<P, C, S, K> Relation for HasMany<P, C, S, K>
where
    P: Table,
    C: Table,
    S: SqlType,
    K: RelationChildren<C>,
{
    type Parent = P;
    type Key = S;
    type Item = K::Item;

    fn parent_key(&self) -> ExprKind {
        self.parent_ref()
    }

    fn sql<D: Dialect>(
        &self,
        prefix: Option<Arc<str>>,
        keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>> {
        let own = keys.next().unwrap_or_default();
        let mut out = Vec::with_capacity(1);
        out.push(render_child_statement::<D>(
            self.child,
            &self.child_ref(),
            self.children.link().as_ref(),
            own,
            prefix.clone(),
        ));
        out.extend(self.children.sql::<D>(prefix, keys));
        out
    }
}

impl<P, C, S, K, DriverImpl> RelationLoader<DriverImpl> for HasMany<P, C, S, K>
where
    P: Table,
    C: Table,
    S: SqlType,
    DriverImpl: Driver,
    C::Model: Decode<DriverImpl> + Send,
    K: RelationChildrenLoader<C, DriverImpl>,
    K::Link: Decode<DriverImpl>,
{
    async fn load<'e, E>(
        &'e self,
        exec: &'e mut E,
        keys: Vec<S::Rust>,
    ) -> Result<HashMap<S::Rust, Vec<Self::Item>>, ErrorOf<E>>
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        S::Rust: Decode<DriverImpl> + Clone + Eq + Hash + Into<Value> + Send + 'e,
    {
        // Nothing to load, so there are no keys to bind. Do not send a statement at
        // all.
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let statement = render_child_statement::<DialectOf<E>>(
            self.child,
            &self.child_ref(),
            self.children.link().as_ref(),
            keys.into_iter().map(Into::into),
            exec.table_prefix(),
        );
        let rows = exec.query(&statement).await?;

        // The child statement selects the foreign key after the declared columns, so
        // each row can be attributed to its parent. The next level's key rides behind
        // it when the relation nests.
        let mut grouped: HashMap<S::Rust, Vec<(C::Model, K::Link)>> =
            HashMap::with_capacity(rows.len());
        let mut links: Vec<K::Link> = Vec::new();
        let mut seen: HashSet<K::Link> = HashSet::new();
        for row in &rows {
            let mut offset = 0;
            let (model, key) = <(C::Model, S::Rust)>::decode(row, &mut offset)?;
            let link = self.children.decode_link(row, &mut offset)?;
            if seen.insert(link.clone()) {
                links.push(link.clone());
            }
            grouped.entry(key).or_default().push((model, link));
        }

        // One statement for the whole next level, and none at all when this level found
        // no rows to key it by.
        let mut nested = self.children.load(exec, links).await?;

        let mut out = HashMap::with_capacity(grouped.len());
        for (key, children) in grouped {
            let mut items = Vec::with_capacity(children.len());
            for (model, link) in children {
                let inner = nested.remove(&link).unwrap_or_default();
                items.push(self.children.assemble(model, inner));
            }
            out.insert(key, items);
        }
        Ok(out)
    }
}

impl<P, J, C, S, T, K> Relation for ManyToMany<P, J, C, S, T, K>
where
    P: Table,
    J: Table,
    C: Table,
    S: SqlType,
    T: SqlType,
    K: RelationChildren<C>,
{
    type Parent = P;
    type Key = S;
    type Item = K::Item;

    fn parent_key(&self) -> ExprKind {
        self.parent_ref()
    }

    fn sql<D: Dialect>(
        &self,
        prefix: Option<Arc<str>>,
        keys: &mut dyn Iterator<Item = Vec<Value>>,
    ) -> Vec<Sql<D>> {
        let own = keys.next().unwrap_or_default();
        let join_parent = self.join_parent_ref();
        let mut join = SelectQuery::new(self.join);
        join.projection = Some(vec![join_parent.clone(), self.join_child_ref()]);
        join.filter.push(in_list(&join_parent, own));
        let join_statement = join.build::<D>(prefix.clone());

        let child_keys = keys.next().unwrap_or_default();
        let child_statement = render_child_statement::<D>(
            self.child,
            &self.child_ref(),
            self.children.link().as_ref(),
            child_keys,
            prefix.clone(),
        );

        let mut out = alloc::vec![join_statement, child_statement];
        out.extend(self.children.sql::<D>(prefix, keys));
        out
    }
}

impl<P, J, C, S, T, K, DriverImpl> RelationLoader<DriverImpl> for ManyToMany<P, J, C, S, T, K>
where
    P: Table,
    J: Table,
    C: Table,
    S: SqlType,
    T: SqlType,
    DriverImpl: Driver,
    C::Model: Decode<DriverImpl> + Send,
    K: RelationChildrenLoader<C, DriverImpl>,
    K::Link: Decode<DriverImpl>,
    K::Item: Clone,
    T::Rust: Decode<DriverImpl> + Clone + Eq + Hash + Send,
{
    async fn load<'e, E>(
        &'e self,
        exec: &'e mut E,
        keys: Vec<S::Rust>,
    ) -> Result<HashMap<S::Rust, Vec<Self::Item>>, ErrorOf<E>>
    where
        E: Executor<Driver = DriverImpl> + Send + 'e,
        S::Rust: Decode<DriverImpl> + Clone + Eq + Hash + Into<Value> + Send + 'e,
    {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        // First the join rows: they name the children each parent references, and the
        // distinct child keys become the next statement's `IN` list.
        let join_parent = self.join_parent_ref();
        let mut join = SelectQuery::new(self.join);
        join.projection = Some(vec![join_parent.clone(), self.join_child_ref()]);
        join.filter
            .push(in_list(&join_parent, keys.into_iter().map(Into::into)));
        let statement = join.build::<DialectOf<E>>(exec.table_prefix());
        let rows = exec.query(&statement).await?;

        let mut by_parent: HashMap<S::Rust, Vec<T::Rust>> = HashMap::new();
        let mut pairs: HashSet<(S::Rust, T::Rust)> = HashSet::new();
        let mut child_keys: Vec<T::Rust> = Vec::new();
        let mut seen: HashSet<T::Rust> = HashSet::new();
        for row in &rows {
            let (parent, child) = <(S::Rust, T::Rust)>::decode(row, &mut 0)?;
            // The result is a `Vec` of distinct child rows, so a duplicated join row
            // attaches the child once.
            if pairs.insert((parent.clone(), child.clone())) {
                by_parent.entry(parent).or_default().push(child.clone());
            }
            if seen.insert(child.clone()) {
                child_keys.push(child);
            }
        }

        if child_keys.is_empty() {
            return Ok(HashMap::new());
        }

        let statement = render_child_statement::<DialectOf<E>>(
            self.child,
            &self.child_ref(),
            self.children.link().as_ref(),
            child_keys.into_iter().map(Into::into),
            exec.table_prefix(),
        );
        let rows = exec.query(&statement).await?;

        let mut loaded: HashMap<T::Rust, (C::Model, K::Link)> = HashMap::with_capacity(rows.len());
        let mut links: Vec<K::Link> = Vec::new();
        let mut link_seen: HashSet<K::Link> = HashSet::new();
        for row in &rows {
            let mut offset = 0;
            let (model, key) = <(C::Model, T::Rust)>::decode(row, &mut offset)?;
            let link = self.children.decode_link(row, &mut offset)?;
            if link_seen.insert(link.clone()) {
                links.push(link.clone());
            }
            loaded.insert(key, (model, link));
        }

        let mut nested = self.children.load(exec, links).await?;
        let mut items: HashMap<T::Rust, K::Item> = HashMap::with_capacity(loaded.len());
        for (key, (model, link)) in loaded {
            let inner = nested.remove(&link).unwrap_or_default();
            items.insert(key, self.children.assemble(model, inner));
        }

        // A child row may belong to several parents, and the result gives each parent
        // its own `Vec`, so every occurrence but the last takes a clone. The last moves
        // the one already built, so nothing is cloned that the result did not need.
        let mut remaining: HashMap<T::Rust, usize> = HashMap::with_capacity(items.len());
        for children in by_parent.values() {
            for key in children {
                *remaining.entry(key.clone()).or_insert(0) += 1;
            }
        }

        let mut out = HashMap::with_capacity(by_parent.len());
        for (parent, children) in by_parent {
            let mut payloads = Vec::with_capacity(children.len());
            for key in children {
                let last = match remaining.get_mut(&key) {
                    Some(count) => {
                        *count = count.saturating_sub(1);
                        *count == 0
                    }
                    None => false,
                };
                let item = if last {
                    items.remove(&key)
                } else {
                    items.get(&key).cloned()
                };
                if let Some(item) = item {
                    payloads.push(item);
                }
            }
            out.insert(parent, payloads);
        }
        Ok(out)
    }
}

/// A `SELECT` that also eagerly loads a relation.
///
/// Created by [`Select::with`]; awaiting it yields `Vec<(R, Vec<Rel::Item>)>` — one entry per
/// parent row, in the parent query's order, and the relation's payloads for each. A parent
/// with no children gets an empty `Vec`, never a missing entry.
///
/// One statement runs per level, all of them ordinary SQL:
///
/// 1. the parent query, with its key column appended to the projection;
/// 2. for a has-many, the child query selecting every child column plus the foreign key,
///    with `foreign_key IN (parent keys)`; for a many-to-many, the join rows for those
///    parents and then the child query for the join rows' child keys.
///
/// The keys are deduplicated before each statement, so a hundred parents with three distinct
/// keys bind three parameters, not a hundred.
pub struct WithRelation<'a, E: Executor, R, Rel: Relation> {
    exec: E,
    parent: SelectQuery,
    relation: Rel,
    phantom: PhantomData<(&'a (), fn() -> R)>,
}

impl<'a, E: Executor, T: Table, R> Select<'a, E, (T,), R> {
    /// Load one relation for every selected row, in one extra statement per level.
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
    /// The relation is a value, so it can be declared once and reused:
    ///
    /// ```ignore
    /// const POSTS: HasMany<UserTable, PostTable, Int8> = has_many(users.id, posts.author_id);
    ///
    /// let by_id: Vec<(User, Vec<Post>)> = db.select(users).with(POSTS).await?;
    /// ```
    ///
    /// The second statement is a separate round trip. It is not hidden, it is not a join, and
    /// it does not run when the parent result is empty. See the
    /// [module documentation](self) for why that trade is the right one.
    pub fn with<Rel: Relation<Parent = T>>(self, relation: Rel) -> WithRelation<'a, E, R, Rel> {
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
            relation,
            phantom: PhantomData,
        }
    }
}

impl<'a, E: Executor, R, Rel: Relation> WithRelation<'a, E, R, Rel> {
    /// Render the statements this load will run, without running any of them.
    ///
    /// The parent statement is the query as it stands with the key appended. The remaining
    /// statements are rendered with `keys` bound into the first `IN` list; a real load takes
    /// those keys from the parent rows, so this is for seeing exactly what will run. Duplicate
    /// keys are removed first, exactly as a load removes them.
    ///
    /// An empty key list renders the always-false form the builder uses for an empty `IN`
    /// list (`1 = 0`), which this method will happily do for inspection; an actual load skips
    /// the statement entirely instead. The first statement is always present, even when there
    /// are no parents.
    ///
    /// A nested relation needs a key list per level, which a parent result does not supply
    /// until the previous level runs. Use [`to_sql_levels`](Self::to_sql_levels) to render
    /// deeper levels; this method renders them with an empty list.
    pub fn to_sql(
        &self,
        keys: impl IntoIterator<Item = <Rel::Key as SqlType>::Rust>,
    ) -> Vec<Sql<DialectOf<E>>>
    where
        <Rel::Key as SqlType>::Rust: Clone + Eq + Hash + Into<Value>,
    {
        let mut seen: HashSet<<Rel::Key as SqlType>::Rust> = HashSet::new();
        let mut first: Vec<Value> = Vec::new();
        for key in keys {
            if seen.insert(key.clone()) {
                first.push(key.into());
            }
        }

        let mut levels = core::iter::once(first);
        let mut out = alloc::vec![self.parent_sql()];
        out.extend(
            self.relation
                .sql::<DialectOf<E>>(self.exec.table_prefix(), &mut levels),
        );
        out
    }

    /// Render every statement this load will run, with one key list per level.
    ///
    /// The first list is the parent keys used by the first relation statement, the second is
    /// the key list that statement would produce for the next level, and so on. A level with
    /// no list given renders the empty-list form (`1 = 0`), which is what
    /// [`to_sql`](Self::to_sql) does for levels beyond the first.
    ///
    /// The values are bound as given: this method cannot deduplicate them, because they are
    /// runtime [`Value`]s rather than the relation's key type. A real load deduplicates each
    /// level before it binds it.
    pub fn to_sql_levels(
        &self,
        keys: impl IntoIterator<Item = Vec<Value>>,
    ) -> Vec<Sql<DialectOf<E>>> {
        let mut levels = keys.into_iter();
        let mut out = alloc::vec![self.parent_sql()];
        out.extend(
            self.relation
                .sql::<DialectOf<E>>(self.exec.table_prefix(), &mut levels),
        );
        out
    }

    fn parent_sql(&self) -> Sql<DialectOf<E>> {
        self.parent.clone().build(self.exec.table_prefix())
    }
}

impl<'a, E, R, Rel> WithRelation<'a, E, R, Rel>
where
    E: Executor,
    R: Decode<E::Driver>,
    Rel: RelationLoader<E::Driver>,
{
    async fn load(self) -> Result<Vec<(R, Vec<Rel::Item>)>, ErrorOf<E>>
    where
        E: Send,
        <Rel::Key as SqlType>::Rust: Decode<E::Driver> + Clone + Eq + Hash + Into<Value> + Send,
    {
        let WithRelation {
            exec,
            parent,
            relation,
            phantom: _,
        } = self;
        let mut exec = exec;

        let parent = parent
            .build::<DialectOf<E>>(exec.table_prefix())
            .finish()
            .map_err(<E::Driver as Driver>::error)?;
        let rows = exec.query(&parent).await?;

        // One extra column rides along with the parent row: its key. Decoding the pair means
        // the key survives even when `R` is a `pick` that does not include it.
        let mut parents: Vec<(R, <Rel::Key as SqlType>::Rust)> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut offset = 0;
            parents.push(<(R, <Rel::Key as SqlType>::Rust)>::decode(
                row,
                &mut offset,
            )?);
        }

        // Deduplicate in first-seen order. The order only affects the parameter list; the
        // result is rebuilt in the parent's order below.
        let mut seen: HashSet<<Rel::Key as SqlType>::Rust> = HashSet::with_capacity(parents.len());
        let mut keys: Vec<<Rel::Key as SqlType>::Rust> = Vec::new();
        for (_, key) in &parents {
            if seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        }

        // Nothing to load, so there are no keys to bind. Do not send a statement at all.
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut grouped = relation.load(&mut exec, keys).await?;

        // Rebuild the parent order, giving every parent its own payloads and an empty `Vec`
        // when nothing was found for it.
        Ok(parents
            .into_iter()
            .map(|(parent, key)| {
                let children = grouped.remove(&key).unwrap_or_default();
                (parent, children)
            })
            .collect())
    }
}

impl<'a, E, R, Rel> IntoFuture for WithRelation<'a, E, R, Rel>
where
    E: Executor + Send + 'a,
    R: Decode<E::Driver> + Send + 'a,
    Rel: RelationLoader<E::Driver> + Send + 'a,
    <Rel::Key as SqlType>::Rust: Decode<E::Driver> + Clone + Eq + Hash + Into<Value> + Send + 'a,
{
    type Output = Result<Vec<(R, Vec<Rel::Item>)>, ErrorOf<E>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.load())
    }
}

/// A qualified column reference.
fn column_ref(table: &'static str, column: &'static str) -> ExprKind {
    ExprKind::Column { table, column }
}

/// An `IN` list over a single column.
fn in_list(key: &ExprKind, keys: impl IntoIterator<Item = Value>) -> ExprKind {
    ExprKind::InList {
        expr: Operand::new(key.clone()),
        list: keys.into_iter().map(ExprKind::Value).collect(),
        negated: false,
    }
}

/// Render the child statement of a relation: every child column, then the key that groups
/// the row, then the next level's key when the relation nests.
fn render_child_statement<D: Dialect>(
    child: &'static TableDef,
    child_key: &ExprKind,
    link: Option<&ExprKind>,
    keys: impl IntoIterator<Item = Value>,
    prefix: Option<Arc<str>>,
) -> Sql<D> {
    let mut query = SelectQuery::new(child);

    // The foreign key is what lets the rows be grouped back onto their parent, so it is
    // selected alongside the declared columns.
    let mut projection = Vec::with_capacity(child.columns.len() + 2);
    for column in child.columns {
        projection.push(ExprKind::Column {
            table: child.name,
            column: column.name,
        });
    }
    projection.push(child_key.clone());
    if let Some(link) = link {
        projection.push(link.clone());
    }
    query.projection = Some(projection);

    query.filter.push(in_list(child_key, keys));

    query.build::<D>(prefix)
}
