# Relations

A relation is a value. `has_many(users.id, posts.author_id)` is a `HasMany` struct built from
the columns it joins on, and `many_to_many(..)` is a `ManyToMany`. Nothing is registered in
the schema and nothing is hidden in a join, so a relation can be declared once, stored in a
`const`, copied and reused across queries:

```rust
use besu::prelude::*;
use besu::types::Int8;

#[derive(Table)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub email: String,
    pub age: i32,
}

#[derive(Table)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    #[besu(references = "users.id", on_delete = "cascade")]
    pub author_id: i64,
}

const POSTS: HasMany<UserTable, PostTable, Int8> = has_many(users.id, posts.author_id);

let users_with_posts: Vec<(User, Vec<Post>)> = db
    .select(users)
    .filter(users.age.gt(18))
    .with(POSTS)
    .await?;
```

`HasMany` and `ManyToMany` are `Copy` when their children are, so passing `POSTS` to `with`
does not consume it. The constructors are `const fn`, which is what lets the value live in a
`const` or `static`. Adding an order or a limit makes the relation own its order terms, so it
is `Clone` rather than `Copy` after that; see [Per-parent limits and
ordering](#per-parent-limits-and-ordering).

## The result

The result has one entry per parent row, in the parent query's order. The parent columns are
decoded into `User` and the children into a `Vec<Post>` beside it. A parent with no children
gets `Vec::new()`, never a missing entry.

## Has-many: two statements

`with` runs the parent query and then one child query. On PostgreSQL that is:

```sql
SELECT "users"."id", "users"."email", "users"."age", "users"."id"
  FROM "users" WHERE ("users"."age" > $1);

SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id"
  FROM "posts" WHERE ("posts"."author_id" IN ($1, $2, $3));
```

The parent statement selects the columns it would have selected on its own, plus its key. The
child statement selects every declared child column, plus the foreign key again. The
repetition is deliberate: the model consumes its columns in declaration order, and the
trailing copy is where the loader reads the grouping key from.

Both statements are built and run independently, so each numbers its own placeholders from
the start. The keys are deduplicated before the second statement, so a parent result with
three distinct keys binds three parameters, however many parents there are.

## Many-to-many: three statements

A join table is a relation too. `many_to_many` takes the parent key, the join table's two
foreign keys, and the child key:

```rust
#[derive(Table)]
#[besu(table = "tags")]
pub struct Tag {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
}

#[derive(Table)]
#[besu(table = "post_tags")]
pub struct PostTag {
    #[besu(primary_key)]
    pub post_id: i64,
    #[besu(primary_key)]
    pub tag_id: i64,
}

const TAGS: ManyToMany<PostTable, PostTagTable, TagTable, Int8, Int8> =
    many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id);

let posts_with_tags: Vec<(Post, Vec<Tag>)> = db.select(posts).with(TAGS).await?;
```

The load is the parent statement, then the join rows for those parents, then the child rows
those join rows name:

```sql
SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."id" FROM "posts";

SELECT "post_tags"."post_id", "post_tags"."tag_id" FROM "post_tags"
  WHERE ("post_tags"."post_id" IN ($1, $2));

SELECT "tags"."id", "tags"."name", "tags"."id" FROM "tags"
  WHERE ("tags"."id" IN ($1, $2, $3));
```

Each statement binds only the keys the previous one produced, so the join statement is one
round trip for the whole result and the child statement is one more. A duplicated
`(parent, child)` join row is folded into one child: the result is a set of child rows, not a
multiset of join rows.

One child can belong to several parents, so a many-to-many needs the child model to be
`Clone`. Each parent owns its own `Vec`, and a shared child is copied once per parent that
references it. The last use moves the value instead of copying it, so the total number of
child allocations is the size of the result. Parent rows are never cloned.

## Nested relations

A relation can carry a relation for its child table, and the result type nests the same way:

```rust
let authors: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
    .select(users)
    .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
    .await?;
```

`users -> posts -> comments` is one statement for the parents and one per level. The posts
statement carries `posts.id` as a trailing column so the comments statement can be filtered by
the ids it found, without another query. The shape composes: a `ManyToMany` can be the inner
relation of a `HasMany`, and a `HasMany` can be the inner relation of a `ManyToMany`.

Every level groups its own statement and preserves the order of the level above it. A parent
with nothing at any level gets `Vec::new()` there. A statement is not sent when the level
above it produced no rows, so an empty intermediate result costs nothing beyond the statement
that discovered it.

## Per-parent limits and ordering

`order_by` and `limit` control the children of each parent. The limit is per parent, not a
global row count: every parent gets at most `n` children in the requested order.

```rust
let users_with_recent_posts: Vec<(User, Vec<Post>)> = db
    .select(users)
    .with(
        has_many(users.id, posts.author_id)
            .order_by(posts.created_at.desc())
            .limit(3),
    )
    .await?;
```

A limit cannot be a plain `LIMIT` on the child statement: that would keep three children for
the whole result, which is wrong for every parent but one. It also cannot be a lateral join,
which SQLite does not have. The statement is a derived table with a window function, which
PostgreSQL, MySQL 8 and SQLite 3.25+ all support:

```sql
SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id"
  FROM (
        SELECT "posts"."id", "posts"."title", "posts"."author_id",
               ROW_NUMBER() OVER (
                   PARTITION BY "posts"."author_id"
                   ORDER BY "posts"."created_at" DESC
               ) AS "besu_row"
          FROM "posts" WHERE ("posts"."author_id" IN ($1, $2))
       ) AS "posts"
 WHERE ("posts"."besu_row" <= $3)
 ORDER BY "posts"."besu_row" ASC;
```

It is still one statement for the whole level, not one per parent. The outer `ORDER BY` keeps
the rows in rank order so each parent's `Vec` arrives in the requested order.

Ordering a has-many on its own needs no window: the child statement simply gets an `ORDER BY`. A limit
with no explicit order falls back to the child table's primary key, so it keeps a stable set
of rows rather than an arbitrary one. Add an order whenever you care which children survive.

The same applies to a many-to-many. Ordering or a limit both use the ranked statement: the
order names a child column, and the join rows have no order that could carry it back to each
parent. An unconfigured many-to-many keeps the cheaper join + child pair. The ranked form is
one statement, because only the ranking join needs to happen before the rank:

```sql
SELECT "tags"."id", "tags"."name", "tags"."id", "besu_ranked"."post_id"
  FROM (
        SELECT "post_tags"."post_id", "post_tags"."tag_id",
               ROW_NUMBER() OVER (
                   PARTITION BY "post_tags"."post_id"
                   ORDER BY "tags"."name" ASC
               ) AS "besu_row"
          FROM "post_tags"
          INNER JOIN "tags" ON ("tags"."id" = "post_tags"."tag_id")
         WHERE ("post_tags"."post_id" IN ($1, $2))
       ) AS "besu_ranked"
 INNER JOIN "tags" ON ("tags"."id" = "besu_ranked"."tag_id")
 WHERE ("besu_ranked"."besu_row" <= $3)
 ORDER BY "besu_ranked"."besu_row" ASC;
```

A duplicated `(parent, child)` join row is still folded into one child in the result, but it
counts against the limit because the rank is assigned before duplicates are folded. A join
table with a primary key — the usual case — cannot produce such a row.

A nested relation gives every level its own order and limit:

```rust
let authors: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
    .select(users)
    .with(
        has_many(users.id, posts.author_id)
            .order_by(posts.created_at.desc())
            .limit(3)
            .with(
                has_many(posts.id, comments.post_id)
                    .order_by(comments.created_at.desc())
                    .limit(2),
            ),
    )
    .await?;
```

## Why it is not a join

A join returns a tuple, `(User, Option<Post>)`, and repeats the same parent columns once per
child. Grouping those rows back into a `User` with its posts is left to the caller, and the
parent data crosses the wire once per child.

`with` returns `(User, Vec<Post>)` instead. It costs one extra round trip per level for the
whole result, not one per parent, and the parent row is sent once. This is why it is not
N+1: each statement runs once regardless of how many rows are above it.

## Filters, ordering and `pick`

The parent query keeps its filters, ordering, limit and projection. `with` only appends the
parent key to what is selected, so the key does not appear in `R`:

```rust
let loaded: Vec<(String, Vec<Post>)> = db
    .select(users)
    .filter(users.age.gte(18))
    .order_by(users.email.desc())
    .pick(users.email)
    .with(POSTS)
    .await?;
```

## Inspecting the SQL

`to_sql(keys)` renders the parent statement and the first relation statement without running
either. The keys stand in for the parents a real load would have found, and duplicate keys
are removed exactly as a load removes them:

```rust
let db = besu::dry::<besu_sqlite::SQLite>();

let statements = db
    .select(users)
    .filter(users.age.gt(18))
    .with(POSTS)
    .to_sql([3, 5, 3]);

assert_eq!(statements.len(), 2);
assert_eq!(statements[1].args().len(), 2);
```

A nested relation's deeper statements are keyed by rows the previous statement returns, which
`to_sql` cannot know. `to_sql_levels` takes one key list per level; a missing list renders the
empty-list form (`1 = 0`):

```rust
let statements = db
    .select(users)
    .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
    .to_sql_levels([
        vec![besu::Value::Int(1)],
        vec![besu::Value::Int(10), besu::Value::Int(11)],
    ]);

assert_eq!(statements.len(), 3);
```

`to_sql` returns one `Sql` per statement in run order, which is why it is a `Vec` rather than
the pair it used to be. A real load skips a statement entirely when its key list is empty;
the inspection methods still render it so the shape is visible.

## What is checked

- Both columns must exist on their tables, be `NonNull`, and share one `SqlType`. A nullable
  foreign key is a compile error; so is an `i64` key paired with an `i32` key.
- A many-to-many's two join columns must belong to the same join table, and each pair of
  joined columns must share one SQL type.
- `with` is only available on a single-table `Select`, and the relation's parent table must
  be that table. After an inner or left join the model type is a tuple, and the method is
  gone; a relation for `posts` cannot be attached to a `users` query.
- The parent column is treated as a key. Nothing checks that it is unique, so a non-unique
  parent key will attach the same child to several parents. For nested relations the same
  applies to the inner key: it is what the next level's rows are grouped by.

## What is not modelled

- **The other direction.** There is no `belongs_to`: a child does not carry a loaded parent.
  Query the parent by key, or use a join.
- **Writing.** Inserting a parent and its children together is not part of this API. The
  insert builder is generic over an executor, which has no portable way to begin a
  transaction, and MySQL has no `RETURNING` to hand back a generated parent key. Write the two
  inserts inside [`Database::transaction`], or make the foreign key deferrable and insert the
  children once the parent key is known:
  ```rust
  db.transaction(async |tx| {
      let user = tx.insert(users).value(users.insert(..)).returning(users).await?;
      tx.insert(posts).value(posts.insert("Title", user[0].id)).await?;
      Ok(())
  })
  .await?;
  ```
- **Relationships in the schema.** Nothing is stored on `TableDef`. The foreign key is a
  separate declaration, and a relation works whether or not the database enforces it.
- **Join rows in the result.** A many-to-many yields the child models, not the join rows. If
  the join table carries data of its own, query it directly.

## Gotchas

- The relation is not registered. It does not add a foreign key, and `push` does not know it
  exists.
- `with` is a separate round trip per level, so it is not free under a very tight latency
  budget. Each level is one statement for the whole result, not one per row.
- A child row whose foreign key is `NULL` is outside the relation, because `has_many` takes
  two `NonNull` columns. Declare the key non-null, or query those children separately.
- The child statement binds one parameter per distinct key. A level with more distinct keys
  than the database's parameter limit allows will fail at build time; narrow the parent query.
- An unconfigured child statement has no `ORDER BY`, so the order is the database's. Call
  `order_by` when it matters. A limit without an order uses the child's primary key, which is
  stable but not necessarily the children you want.
- A relation with an order or limit is `Clone` rather than `Copy`, because it owns its order
  terms. An unconfigured relation is still `Copy` and renders exactly the same SQL it did
  before.

[`Database::transaction`]: https://docs.rs/besu/latest/besu/struct.Database.html#method.transaction
