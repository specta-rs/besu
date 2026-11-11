# Selecting data

A `SELECT` starts with the table handle and grows by chaining methods. Each method consumes
the builder and returns a new one, so the type carries what the query knows: which tables are
in scope (`M`) and what one row decodes into (`R`).

```rust
let adults: Vec<User> = db
    .select(users)
    .filter(users.age.gt(18))
    .order_by(users.name.asc())
    .limit(50)
    .await?;
```

The type annotation is load-bearing. `db.select(users)` returns a builder whose row type is
`User`; awaiting it produces `Vec<User>`. If you change the projection later, the annotation
has to change too, and the compiler tells you where.

## Filtering

`filter` adds a `WHERE` condition. Calling it repeatedly combines the conditions with `AND`:

```rust
db.select(users)
    .filter(users.age.gte(18))
    .filter(users.bio.is_null())
    .await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."age" >= ?) AND ("users"."bio" IS NULL)
```

Every column has the comparison methods below. They read like the operators they emit, and
each one binds its right-hand side as a parameter rather than formatting it into the SQL.

| Method | SQL |
| --- | --- |
| `eq`, `ne` | `=`, `<>` |
| `gt`, `gte`, `lt`, `lte` | `>`, `>=`, `<`, `<=` |
| `like`, `not_like` | `LIKE`, `NOT LIKE` |
| `ilike`, `not_ilike` | `ILIKE`, `NOT ILIKE` (PostgreSQL only) |
| `is_null`, `is_not_null` | `IS NULL`, `IS NOT NULL` |
| `in_list`, `not_in` | `IN (...)`, `NOT IN (...)` |
| `between` | `BETWEEN ... AND ...` |

There are also free functions for the cases that do not hang off a column:
`is_true`, `is_false`, `not_between`, and the combinators `and`, `or`, `not`.

```rust
db.select(users)
    .filter(and(
        users.age.gte(20),
        or(users.name.like("A%"), users.name.like("B%")),
    ))
    .await?;
```

A boolean column can be used directly, because `Column<T, Bool, N>` implements
`IntoCondition`:

```rust
db.select(posts).filter(posts.published).await?;
// WHERE "posts"."published"
```

`like` takes a pattern as a value, so `%` and `_` keep their SQL meaning without any string
interpolation:

```rust
.filter(users.name.like("A%"))
```

Rust integers and floats are checked against the column's SQL type. `users.age` is `Int4`, so
`users.age.gt(18)` compiles because `18` is inferred as `i32`. `users.age.gt("eighteen")` does
not: there is no `IntoSqlValue<Int4>` for `&str`, and the error points at the argument. This
is the `SameType` / `IntoSqlValue` machinery described in [Architecture](../architecture.md).

## Reading inside a JSON document

`json_get` takes a JSON value out of a document and keeps its JSON type; `json_text` returns
it unquoted, so it compares like any other string. The path is a dotted key path and must be a
literal, because it is rendered into the SQL rather than bound:

```rust
let names: Vec<String> = db
    .select(documents)
    .pick(json_text(documents.body, "user.name"))
    .await?;

db.select(documents)
    .filter(json_text(documents.body, "user.name").eq("Oscar"))
    .await?;
```

Each dialect renders this its own way — `#>> '{user,name}'` on PostgreSQL, `->> '$.user.name'`
on SQLite, `JSON_UNQUOTE(JSON_EXTRACT(..))` on MySQL — but the call site is the same. Keys
containing a dot are not expressible; drop to `raw` for those.

## Full-text search

`matches` searches a column's text, and `posts.title.matches("rust")` is the method form:

```rust
db.select(posts)
    .filter(posts.title.matches("rust"))
    .await?;
```

The search text is bound as a parameter, so untrusted input never becomes SQL, and each
database parses it with its own search grammar. Every dialect spells the match differently:

| Dialect | Rendered |
| --- | --- |
| PostgreSQL | `to_tsvector('english', col) @@ plainto_tsquery('english', $1)` |
| MySQL | `MATCH(col) AGAINST ($1 IN BOOLEAN MODE)` |
| SQLite | `col MATCH $1` |

The differences go beyond spelling, so what the schema needs differs too:

- **PostgreSQL** needs nothing. `to_tsvector` builds the vector for each row, so the query is
  correct without an index and only slow; add an expression index if it matters.
- **MySQL** requires a `FULLTEXT` index over exactly the columns the expression names. The
  schema layer cannot declare one, so add it yourself:
  `db.execute(sql!("ALTER TABLE posts ADD FULLTEXT INDEX posts_title_fts (title)"))`.
- **SQLite** matches an FTS5 virtual table, which the schema layer cannot create either.
  Create it with `sql!("CREATE VIRTUAL TABLE documents USING fts5(body)")` and query it
  through an ordinary table handle.

Using `matches` against SQLite's ordinary table is an error from the database — `MATCH` is a
property of the FTS5 table, not a function.

The search is a word match, not a substring one, and query syntax belongs to the database:
`"brown fox"` is a phrase search on SQLite, `+rust` requires a token on MySQL, and PostgreSQL
understands the `english` dictionary's operators.

PostgreSQL's text search configuration is fixed at `english` for now, which is why `borrow`
finds `borrowing`; a parameter for it, along with ranking and highlighting, is the rest of
[#57](https://github.com/specta-rs/besu/issues/57).

## Ordering

```rust
db.select(users)
    .order_by(users.name.asc())
    .order_by(users.age.desc())
    .await?;
```

`order_by` accepts a column, an expression or an `OrderClause`; a bare column is ascending.
`order_by_desc` reverses whatever it is given, which is handy when the clause comes from
elsewhere.

`NULL` placement is explicit:

```rust
db.select(users).order_by(users.bio.asc_nulls_first());
db.select(users).order_by(users.bio.desc_nulls_last());
```

`asc_nulls_first` and `desc_nulls_last` are the two common combinations. For anything else,
build the `OrderClause` and adjust it: `users.bio.asc().nulls_last()` or
`some_expression.desc().nulls_first()`.

SQLite and PostgreSQL render `NULLS FIRST` / `NULLS LAST`. MySQL has no such syntax, so Besu
emulates it by sorting on `(expr IS NULL)` first. `users.bio.asc().nulls_last()` becomes:

```sql
ORDER BY ("users"."bio" IS NULL) ASC, "users"."bio" ASC
```

That matches the common cases. It is an extra sort key, not a new index, so a query that
relies on `NULL` order can use a different plan on MySQL.

## Limit and offset

```rust
db.select(users)
    .order_by(users.id.asc())
    .limit(50)
    .offset(100)
    .await?;
```

Both are bound as parameters, not written into the SQL:

```sql
... ORDER BY "users"."id" ASC LIMIT ? OFFSET ?
```

`first()` is the one-row shorthand. It sets `limit(1)` and returns `Option<R>`:

```rust
let user: Option<User> = db.select(users).filter(users.id.eq(1)).first().await?;
```

`offset` without a limit needs a limit in some dialects. MySQL requires one, so Besu emits
`LIMIT 9223372036854775807 OFFSET n`; SQLite emits `LIMIT -1 OFFSET n`.

## Row locking

`SELECT ... FOR UPDATE` and `FOR SHARE` are available on dialects that support them:

```rust
let job: Option<Job> = db
    .select(jobs)
    .filter(jobs.state.eq("queued"))
    .for_update()
    .skip_locked()
    .first()
    .await?;
```

| Method | SQL |
| --- | --- |
| `for_update()` | `FOR UPDATE` |
| `for_share()` | `FOR SHARE` |
| `skip_locked()` | adds `SKIP LOCKED` to an existing lock |
| `no_wait()` | adds `NOWAIT` to an existing lock |

`skip_locked` and `no_wait` only modify a lock that is already there; calling them on their
own does nothing. The lock is written after `LIMIT`/`OFFSET`, which is the order the grammar
expects.

Row locks are held until the surrounding transaction ends, so they only make sense inside
`db.begin()` / `db.transaction(..)`. A statement run on the pool releases its lock when the
statement completes, which is almost never what a queue wants.

SQLite has no row locking; `for_update` on SQLite builds an `unsupported` error rather than
emitting SQL the database would reject.

## Distinct

```rust
let ages: Vec<i32> = db.select(users).pick(users.age).distinct().await?;
```

`distinct()` sets `SELECT DISTINCT`. It applies to the whole projection, so it interacts with
`pick` in the usual SQL way: two rows that differ only in a column you did not select become
one row.

## Projections with `pick`

By default a select returns every column of every table in the query, in table order. `pick`
replaces that projection and changes the row type:

```rust
let names: Vec<(i64, String)> = db
    .select(users)
    .pick((users.id, users.name))
    .await?;
```

The tuple *is* the row type. There is no row mapper and no `FromRow` derive, because the
selection's type already says what will be decoded. Tuples of up to twelve elements are
supported.

`pick` accepts columns, expressions, table handles and tuples of those:

| Selection | Row type |
| --- | --- |
| `users.name` | `String` |
| `users.bio` | `Option<String>` |
| `count_all()` | `i64` |
| `(users.id, users.name)` | `(i64, String)` |
| `(users.name, count(posts.id))` | `(String, i64)` |
| `users` (the base table handle) | `User` |

A nullable column selected on its own becomes `Option<T>`, because the decoded value may be
`NULL`. That is why `Vec<Option<String>>` is the right annotation for `pick(users.bio)`.

Nullability also follows the join path. A column from the outer side of a `LEFT JOIN` or
`FULL JOIN` becomes `Option<T>` even when the column is declared `NOT NULL`, and a table
handle picked from that side becomes `Option<Model>`:

```rust
let rows: Vec<(User, Option<String>)> = db
    .select(users)
    .left_join(posts, eq(users.id, posts.author_id))
    .pick((users, posts.title))
    .await?;

let rows: Vec<(User, Option<Post>)> = db
    .select(users)
    .left_join(posts, eq(users.id, posts.author_id))
    .pick((users, posts))
    .await?;
```

The second annotation is not a special case in `pick`: the path through `Option<Post>` is what
makes the type widen. An `INNER JOIN` does not put the joined table behind an `Option`, so
`pick((users, posts))` there is `(User, Post)`.

If you want a computed column, pick the expression:

```rust
let counts: Vec<i64> = db.select(users).pick(count_all()).await?;
```

## Joins

A join takes the table handle and an `ON` expression built from columns of both tables:

```rust
let feed: Vec<(User, Option<Post>)> = db
    .select(users)
    .left_join(posts, eq(users.id, posts.author_id))
    .await?;
```

The joined table joins the row set, and the row type grows by one element:

| Method | SQL | Row type after |
| --- | --- | --- |
| `inner_join(posts, on)` | `INNER JOIN` | `(R, Post)` |
| `left_join(posts, on)` | `LEFT JOIN` | `(R, Option<Post>)` |
| `right_join(posts, on)` | `RIGHT JOIN` | `(Option<R>, Post)` |
| `full_join(posts, on)` | `FULL OUTER JOIN` | `(Option<R>, Option<Post>)` |
| `cross_join(posts)` | `CROSS JOIN` | `(R, Post)` |

### Why the joined side is `Option`

A left join produces at most one row per left row, but it may produce *no* matching right row.
The result set still has a row — with every right-hand column `NULL`. `Option<Post>` is the
only honest type for that, and it is what makes the compiler force you to handle the missing
case:

```rust
for (user, post) in &feed {
    println!("{} -> {}", user.name, post.as_ref().map_or("<none>", |p| p.title.as_str()));
}
```

The decoding rule is uniform: an `Option<T>` is `None` when every column `T` would consume is
`NULL`. That is why the same `Option` works both for a nullable column and for a missing outer
row.

Joins nest, so a third table produces a nested tuple:

```rust
let rows: Vec<((User, Option<Post>), Comment)> = db
    .select(users)
    .left_join(posts, eq(users.id, posts.author_id))
    .inner_join(comments, eq(posts.id, comments.post_id))
    .await?;
```

The query's row set is the same nesting, which is what lets `comments.text.eq("hi")` compile
after the second join but not before it. Filters and projections refer to any table already in
the query.

The `ON` expression is a boolean expression. `eq(users.id, posts.author_id)` is the common
case; `and(...)` works for a compound join condition.

`CROSS JOIN` takes no `ON`, because it deliberately pairs every row with every row.

## Eager loading with `with`

A join gives you `(User, Option<Post>)` — one row per post, with the user repeated and
`Option` to handle the user who has none. When you want each user *with* all of its posts,
`with` loads the relation in a second, visible statement:

```rust
let users_with_posts: Vec<(User, Vec<Post>)> = db
    .select(users)
    .filter(users.age.gt(18))
    .with(has_many(users.id, posts.author_id))
    .await?;
```

Two statements run, and both are ordinary SQL you could have written yourself:

```sql
SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio", "users"."id"
  FROM "users" WHERE ("users"."age" > $1);

SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id"
  FROM "posts" WHERE ("posts"."author_id" IN ($1, $2, $3));
```

`has_many` takes the parent's key column and the child's foreign key. They must have the same
SQL type, which is what makes the parent keys usable as the child's `IN` list. Nothing is
registered on the table: the relation is just the column pair. The two statements are built
and run independently, so each numbers its own placeholders from the start.

The first statement is the query as you wrote it, with the parent key appended to the
projection. The second selects the child table's columns and then the foreign key again; the
repetition is what the loader reads the key from, and it costs one column per child row, not a
duplicated parent row.

The keys are deduplicated before the child statement, so a hundred parents with three distinct
keys bind three parameters, not a hundred. The child statement runs once for the whole result,
so this is not N+1. When the parent result is empty no second statement is sent at all, because
`IN ()` is a syntax error on some databases.

Every parent is present in the result, in the parent query's order. A parent with no children
gets an empty `Vec` rather than disappearing:

```rust
for (user, posts) in &users_with_posts {
    println!("{} has {} posts", user.name, posts.len());
}
```

Children arrive in the order the database returned them; this API does not add an `ORDER BY`
of its own. Add one to the child statement by hand if the order matters.

`with` composes with the rest of the builder, including `pick`:

```rust
let named: Vec<(String, Vec<Post>)> = db
    .select(users)
    .order_by(users.name.asc())
    .pick(users.name)
    .with(has_many(users.id, posts.author_id))
    .await?;
```

`with` is only available on a plain `db.select(table)`; after a join the row set is no longer a
single table, so there is no unambiguous parent to attach children to. Use the join and group
the rows yourself if you need that shape.

To see exactly what will run, `to_sql(keys)` renders both statements for a key list:

```rust
let (parent, child) = db
    .select(users)
    .with(has_many(users.id, posts.author_id))
    .to_sql([1, 2, 3]);
```

### What is not modelled

- **Inserting a parent and its children together.** The insert builder writes one table, and
  a transaction is the explicit way to write two.
- **Many-to-many.** It needs a join table and two column pairs, so `has_many` cannot describe
  it.
- **Belongs-to.** A child's parent is the same relation read from the other end; load it with
  a `select` on the parent table, or with a join.
- **Nested relations.** `with` returns `Vec<(R, Vec<C>)>`, not a child that itself has
  children loaded.
- **Per-parent limits.** `LIMIT` applies to the parent query; the child statement has no
  `LIMIT` and no per-parent pagination.

## Grouping and aggregates

`group_by` and `having` sit between filtering and ordering:

```rust
let grouped: Vec<(i32, i64)> = db
    .select(users)
    .pick((users.age, count_all()))
    .group_by(users.age)
    .having(count_all().gt(1))
    .order_by(users.age.asc())
    .await?;
```

```sql
SELECT "users"."age", COUNT(*) FROM "users"
GROUP BY "users"."age"
HAVING (COUNT(*) > ?)
ORDER BY "users"."age" ASC
```

`group_by` accepts a column or expression. `having` accepts the same conditions as `filter`,
and repeated calls are combined with `AND`. The two are separate because SQL evaluates them at
different points: `WHERE` filters rows before grouping, `HAVING` filters groups after it.

The common aggregate functions:

| Function | Result type | Notes |
| --- | --- | --- |
| `count(expr)` | `Int8` (`i64`) | `COUNT(expr)` |
| `count_all()` | `Int8` | `COUNT(*)` |
| `count_distinct(expr)` | `Int8` | `COUNT(DISTINCT expr)` |
| `sum(expr)` | `Float8` (`f64`) | Widened, because integer sums overflow |
| `avg(expr)` | `Float8` | |
| `min(expr)`, `max(expr)` | Same as the argument | |
| `lower`, `upper`, `trim` | `Text` | |
| `length(expr)` | `Int8` | |
| `concat(a, b)` | `Text` | Two arguments |
| `coalesce(a, fallback)` | Same as `a` | |
| `round`, `ceil`, `floor` | Same as the argument | |
| `abs` | Same as the argument | In the prelude |
| `now()` | `TimestampTz` | `CURRENT_TIMESTAMP` |

`sum` returning `f64` is deliberate: SQL integer sums overflow silently in some databases, so
the builder widens the result. If you want an integer sum, cast the result back and accept the
range, or use `raw`.

## Set operations

`union`, `union_all`, `intersect`, `intersect_all`, `except` and `except_all` combine two
`SELECT`s into one statement:

```rust
let adults = db.select(users).filter(users.age.gte(18));
let minors = db.select(users).filter(users.age.lt(18));

let everyone: Vec<User> = adults
    .union(minors)
    .order_by(users.name.asc())
    .await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."age" >= $1)
UNION
SELECT ... FROM "users" WHERE ("users"."age" < $2)
ORDER BY "name" ASC
```

The builder is a `SetQuery` rather than a `Select`, and the second operand has to decode as
the same row type `R`. That is what stops `select(users).union(select(posts))` from
compiling: a compound row is decoded by the row type of the first branch, so a second branch
that decodes differently has no meaning. A `pick` on both sides is the usual fix.

| Method | SQL | Duplicates |
| --- | --- | --- |
| `union` | `UNION` | removed |
| `union_all` | `UNION ALL` | kept |
| `intersect` | `INTERSECT` | removed |
| `intersect_all` | `INTERSECT ALL` | smaller of the two counts |
| `except` | `EXCEPT` | removed |
| `except_all` | `EXCEPT ALL` | left count minus right count |

The `ALL` forms are multiset operations, not a spelling variant: `intersect_all` of two
copies with two copies is two copies, while `intersect` of the same inputs is one. SQLite has
neither, so those two queries fail while the statement is built instead of being sent.

Operations chain, and the chain keeps every operator in order:

```rust
let rows: Vec<User> = adults
    .union(db.select(users).filter(users.age.lt(10)))
    .except(db.select(users).filter(users.name.like("A%")))
    .order_by(users.name.asc())
    .await?;
```

`order_by`, `limit`, `offset` and `first` apply to the whole compound, which is where SQL
puts them; `first` sets `limit(1)`, replacing any limit already set. The ordering term names
an output column and is written unqualified:

```sql
... UNION ... EXCEPT ... ORDER BY "name" ASC LIMIT $1
```

It has to be unqualified because PostgreSQL and MySQL only accept a result column name in a
compound's `ORDER BY`; a table-qualified name is rejected by both. An expression there is
written as given and may be rejected too — those two databases allow only column names.

A branch may not carry its own `ORDER BY`, `LIMIT` or `OFFSET`. PostgreSQL and SQLite could
parenthesise a branch and keep it, but MySQL wants a derived table, and dropping it silently
would run a different query from the one you wrote, so it is an error that names the branch.

Mixing `INTERSECT` with `UNION` or `EXCEPT` in one chain is refused as well. The SQL has no
parentheses because SQLite cannot parse a parenthesised compound operand, and without them
the two families group differently: PostgreSQL and MySQL bind `INTERSECT` tighter, SQLite
evaluates left to right. `1 UNION 2 INTERSECT 2` is `{1, 2}` there and `{2}` on SQLite. Use
one operator family per chain.

## Subqueries

`exists` and `not_exists` take a whole `Select` and produce a condition:

```rust
let with_posts: Vec<User> = db
    .select(users)
    .filter(exists(
        db.select(posts)
            .correlate(users)
            .filter(posts.author_id.eq(users.id)),
    ))
    .await?;
```

```sql
SELECT ... FROM "users" WHERE EXISTS (
    SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts"
    WHERE ("posts"."author_id" = "users"."id")
)
```

`correlate(users)` is what lets the subquery read `users.id`. It adds `users` to the
subquery's row set without adding a `FROM` entry, so the column is type-checked exactly as it
would be in the outer query:

```rust
db.select(posts)
    .correlate(users)                        // `users.id` is now in scope here
    .filter(posts.author_id.eq(users.id));
```

Without it, `users.id` inside the subquery is a compile error, because `users` is not one of
the subquery's own tables. The reverse direction — that the enclosing query really does
select from `users` — is not checked: the outer query's type is not in scope at the point the
subquery is built, so a correlation with a table the outer query does not have fails in the
database as an unknown column.

`exists` ignores the subquery's projection; only whether a row comes back matters. The
subquery is rendered as part of the enclosing statement, so its parameters continue that
statement's numbering — it is one statement, not a round trip.

`in_query` and `not_in_query` compare a column against a one-column subquery:

```rust
let authors: Vec<User> = db
    .select(users)
    .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
    .await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."id" IN (SELECT "posts"."author_id" FROM "posts"))
```

The subquery's row type must be the column's Rust type, which is how the two sides are kept
honest: `pick` erases the SQL type to whatever the selection decodes as, so `i64` against
`i64` is checked but a `VARCHAR` against an integer is left to the database. It also means a
nullable column's pick is `Option<T>` and is rejected; select it through `as_expr()` to
compare the value without the nullability.

Prefer `not_exists` to `not_in_query` when the subquery can produce `NULL`: `NOT IN` is
unknown rather than true when the list contains a null, while `NOT EXISTS` stays true.

## Expressions and casts

Columns support arithmetic directly:

```rust
db.update(users)
    .set_expr(users.age, users.age.as_expr().plus(1))
    .all_rows()
    .await?;
```

`plus`, `minus`, `times`, `divide` and `modulo` exist on both `Column` and `Expr`, and they
check that the right-hand side is the same SQL type. `as_expr()` is the bridge from a column
to the expression API when you need something the column methods do not offer.

`cast` changes the SQL type:

```rust
let ages: Vec<f64> = db
    .select(users)
    .pick(sum(users.age.as_expr().cast::<besu::Float8>()))
    .await?;
```

That renders `SUM(CAST("users"."age" AS REAL))` on SQLite. The target is a `SqlType`, so
`besu::Text`, `besu::Int8`, `besu::Float8` and the rest are all valid.

For an expression Besu does not model, `raw` inserts SQL verbatim:

```rust
db.select(users).filter(raw("age * 2 > 40")).await?;
```

The string is not checked and not escaped, and the query's row set is inferred from the call
site. It is the expression-level escape hatch; see [Raw SQL](raw-sql.md).

### `CASE`

`case` builds a conditional expression. The SQL type is explicit because a bare literal could
equally be `TEXT`, `VARCHAR(n)` or `CHAR(n)`:

```rust
use besu::prelude::*;

let labels: Vec<String> = db
    .select(users)
    .pick(
        case::<_, besu::Text>()
            .when(users.age.gte(18), "adult")
            .otherwise("minor"),
    )
    .await?;
```

```sql
SELECT (CASE WHEN ("users"."age" >= ?) THEN ? ELSE ? END) FROM "users"
```

`when` can be called repeatedly and `otherwise` finishes the expression. A `CASE` with no
`otherwise` is not expressible: the builder always writes `ELSE`, which keeps the SQL type
non-nullable and matches what most callers mean.

## Looking at the SQL

`to_sql()` renders without a database:

```rust
let db = besu::dry::<besu_sqlite::SQLite>();

let sql = db
    .select(users)
    .filter(users.age.gt(18))
    .filter(users.bio.is_null())
    .order_by(users.name.asc())
    .limit(10)
    .to_sql()
    .unwrap();

assert_eq!(
    sql.sql(),
    r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > ?) AND ("users"."bio" IS NULL) ORDER BY "users"."name" ASC LIMIT ?"#
);
assert_eq!(sql.args().len(), 2);
```

`inspect()` is the logging form: it substitutes the parameters into the text. That output is
for humans, never for execution.

## What is checked

| Mistake | When it fails |
| --- | --- |
| A column that does not exist (`users.missing`) | Compile error: no such field |
| A value of the wrong type (`users.age.eq("old")`) | Compile error at the argument |
| A table that was never joined, via a column method | Compile error: `Contains` is not satisfied |
| A relation on a query with a join (`Select::with`) | Compile error: the row set is not a single table |
| A `union` with a branch of a different row type | Compile error at the second operand |
| A projection whose type does not match the binding | Compile error at the `Vec<T>` annotation |
| A nullable column assigned to a non-`Option` type | Compile error |
| `ILIKE` on a dialect without it | Runtime error while building the SQL |
| `FULL OUTER JOIN` / `RIGHT JOIN` on a dialect without it | Runtime error while building the SQL |
| Row locking on SQLite | Runtime error while building the SQL |
| `INTERSECT ALL` / `EXCEPT ALL` on SQLite | Runtime error while building the SQL |
| A set branch with its own `ORDER BY`, `LIMIT` or `OFFSET` | Runtime error while building the SQL |
| `INTERSECT` mixed with `UNION` or `EXCEPT` in one chain | Runtime error while building the SQL |

## Gotchas

- **The free comparison functions do not carry the join check.** `posts.title.eq("x")` is
  checked against the row set; `eq(posts.title, "x")` is not, because the free function has no
  `Contains` bound. Both emit the same SQL. Prefer the method form when you want the
  compile-time guarantee.
- **`pick` on a table handle widens with the join.** After an outer join, `pick((users,
  posts))` is `(User, Option<Post>)`, not `(User, Post)`; the path through `Option` supplies
  the nullability. That is the opposite of what a hand-written `Decode` would do, so let the
  annotation follow the compiler.
- **`pick` replaces the projection.** It does not add to the default column list.
- **`with` selects the child foreign key twice.** The model consumes the declared columns in
  order and the trailing copy is what the loader reads the key from, so the second statement
  has one redundant column per child row. That is the cost of grouping any child type without
  a hand-written decoder.
- **A table handle does not use the model's `Decode`.** `pick(users)` is a list of columns, so
  a field marked `#[besu(skip)]` is filled with `Default::default()` exactly as it is in the
  default projection.
- **`first()` overrides a previous `limit`.** It sets the limit to 1 unconditionally, including
  on a set query.
- **A correlated subquery needs `correlate`.** `users.id` inside a subquery is only in scope
  once `correlate(users)` has put it there, and the compiler cannot check that the enclosing
  query really selects from it.
- **A set query's `ORDER BY` must name an output column.** It is written unqualified, because
  PostgreSQL and MySQL accept nothing else there.
- **`in_query` compares Rust types, not SQL types.** A nullable column's pick is `Option<T>`
  and does not match a non-null column; use `as_expr()` on the picked column.
- **Grouping is not checked.** SQL requires every non-aggregated projection column to be
  grouped; Besu does not. The database will reject the query, not the compiler.
- **`sum` is a float.** `pick(sum(users.age))` is `Vec<f64>`, not `Vec<i32>`.
- **`matches` is not type-checked against the column.** Any expression is accepted, because
  `TEXT`, `VARCHAR(n)` and a hand-declared FTS5 table are all plausible, so matching an
  integer column fails in the database rather than the compiler.
- **MySQL's `CAST` grammar is narrower than the renderer's.** `cast::<besu::Text>()` emits
  `CAST(x AS TEXT)`, which MySQL rejects; MySQL only accepts targets such as `CHAR`,
  `SIGNED`, `DECIMAL` and `DATETIME`. The cast is fine on SQLite and PostgreSQL, or write the
  cast as raw SQL.
- **`to_sql()` does not fail for a capability problem.** The builders return `Ok(Sql)` with
  `.error()` set; execution calls `finish()` and surfaces it. Check `sql.error()` if you are
  generating SQL for inspection.
- **An outer row that is all-`NULL` decodes as `None`.** `Option<T>` is `None` when every
  column `T` consumes is `NULL`, so a real joined row whose columns happen to all be nullable
  and all `NULL` is indistinguishable from a missing one. Include a non-null key column in
  the joined table to keep `Some`.
