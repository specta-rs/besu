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
// WHERE ("posts"."published")
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
emulates it by sorting on `(expr IS NULL)` first:

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
| `users` (the handle) | `User` |

A nullable column selected on its own becomes `Option<T>`, because the decoded value may be
`NULL`. That is why `Vec<Option<String>>` is the right annotation for `pick(users.bio)`.

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
| A projection whose type does not match the binding | Compile error at the `Vec<T>` annotation |
| A nullable column assigned to a non-`Option` type | Compile error |
| `ILIKE` on a dialect without it | Runtime error while building the SQL |
| `FULL OUTER JOIN` / `RIGHT JOIN` on a dialect without it | Runtime error while building the SQL |
| Row locking on SQLite | Runtime error while building the SQL |

## Gotchas

- **The free comparison functions do not carry the join check.** `posts.title.eq("x")` is
  checked against the row set; `eq(posts.title, "x")` is not, because the free function has no
  `Contains` bound. Both emit the same SQL. Prefer the method form when you want the
  compile-time guarantee.
- **`pick` on a table handle does not become `Option` for an outer join.** The default
  projection after `left_join` decodes to `(User, Option<Post>)`, but `pick((users, posts))`
  is typed `(User, Post)`. If the join misses, that projection tries to decode `NULL` into a
  non-nullable field and fails. Use the default projection, or pick columns and annotate the
  nullable ones yourself.
- **`pick` replaces the projection.** It does not add to the default column list.
- **`first()` overrides a previous `limit`.** It sets the limit to 1 unconditionally.
- **Grouping is not checked.** SQL requires every non-aggregated projection column to be
  grouped; Besu does not. The database will reject the query, not the compiler.
- **`sum` is a float.** `pick(sum(users.age))` is `Vec<f64>`, not `Vec<i32>`.
- **MySQL's `CAST` grammar is narrower than the renderer's.** `cast::<besu::Text>()` emits
  `CAST(x AS TEXT)`, which MySQL rejects; MySQL only accepts targets such as `CHAR`,
  `SIGNED`, `DECIMAL` and `DATETIME`. The cast is fine on SQLite and PostgreSQL, or write the
  cast as raw SQL.
- **`to_sql()` rarely returns `Err`.** The builder stores the first problem inside the `Sql`
  and still returns it; execution calls `finish()` and surfaces the error. Check
  `sql.error()` if you are generating SQL for inspection.
- **An outer row that is all-`NULL` decodes as `None`.** `Option<T>` is `None` when every
  column `T` consumes is `NULL`, so a real joined row whose columns happen to all be nullable
  and all `NULL` is indistinguishable from a missing one. Include a non-null key column in
  the joined table to keep `Some`.
