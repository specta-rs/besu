# Operators

Every operator is a typed expression: it knows the SQL type it compares and the tables it may
reference. Comparisons exist both as methods on a column and as free functions in the prelude;
the rendered SQL is the same. Examples render for SQLite, where parameters are `?`; PostgreSQL
numbers them `$1`, `$2`.

## Comparison

| SQL | Column method | Free function |
| --- | --- | --- |
| `=` | `users.age.eq(18)` | `eq(users.age, 18)` |
| `<>` | `users.age.ne(18)` | `ne(users.age, 18)` |
| `>` | `users.age.gt(18)` | `gt(users.age, 18)` |
| `>=` | `users.age.gte(18)` | `gte(users.age, 18)` |
| `<` | `users.age.lt(18)` | `lt(users.age, 18)` |
| `<=` | `users.age.lte(18)` | `lte(users.age, 18)` |

```rust
db.select(users)
    .filter(users.age.gte(18))
    .filter(users.age.lt(65))
    .await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."age" >= ?) AND ("users"."age" < ?)
```

The free form checks both operands, so `eq(users.id, 1_i64)` and `eq(7, users.age)` compile while
`eq(users.id, "seven")` does not. The column form checks its argument against the column's type.
The same methods exist on `Expr`, which is how an aggregate gets compared:
`count_all().gt(1)`.

## Logical

`and`, `or` and `not` take anything usable as a condition, including a boolean column on its own.

```rust
db.select(users)
    .filter(and(
        users.age.gte(18),
        or(users.bio.is_not_null(), not(users.name.eq("Anonymous"))),
    ))
    .await?;
```

```sql
SELECT ... FROM "users"
WHERE (("users"."age" >= ?) AND (("users"."bio" IS NOT NULL) OR (NOT ("users"."name" = ?))))
```

A boolean column needs no comparison, so for a table with an `active: bool` column,
`filter(users.active)` renders `WHERE "users"."active"`, without parentheses.

## Null and boolean tests

`is_null` and `is_not_null` are column methods and functions. `is_true` and `is_false` are
functions only, and take any typed expression — a boolean column such as `active` included.

```rust
db.select(users)
    .filter(users.bio.is_null())
    .filter(is_true(users.active))
    .await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."bio" IS NULL) AND ("users"."active" IS TRUE)
```

These are `IS` tests rather than `= TRUE`, so a `NULL` column is neither true nor false, as SQL
intends.

## Pattern matching

`like`, `not_like`, `ilike` and `not_ilike` exist as a column method
(`users.name.like("A%")`) and as a function (`like(users.name, "A%")`).

```rust
db.select(users).filter(users.name.like("A%")).await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."name" LIKE ?)
```

The pattern is bound as a parameter, so `%` and `_` are SQL wildcards without any string
interpolation. `ILIKE` is PostgreSQL-only: elsewhere the statement builds and records
``unsupported: `sqlite` has no `ILIKE`; use `LIKE` with a lowercased column instead``.

## Ranges

```rust
db.select(users).filter(users.age.between(18, 65)).await?;
db.select(users).filter(not_between(users.age, 18, 65)).await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."age" BETWEEN ? AND ?)
SELECT ... FROM "users" WHERE ("users"."age" NOT BETWEEN ? AND ?)
```

`between` is a column method and a function; `not_between` is a function only. Both bounds are
inclusive, as in SQL.

## Sets

```rust
db.select(users).filter(users.name.in_list(["Alice", "Bob"])).await?;
db.select(users).filter(users.name.not_in(["Alice"])).await?;
```

```sql
SELECT ... FROM "users" WHERE ("users"."name" IN (?, ?))
SELECT ... FROM "users" WHERE ("users"."name" NOT IN (?))
```

Every item is its own bound parameter, which is why a very large list can exceed the dialect's
parameter limit. An empty list is not valid SQL — `IN ()` is a syntax error everywhere — so it
renders the constant predicate `(1 = 0)` for `in_list` and `(1 = 1)` for `not_in`, which the
database folds away and which keeps a dynamically built filter from producing broken SQL.

The items are checked against the column's SQL type, so `users.age.in_list(["Alice"])` is a
compile error rather than a runtime one. For a subquery, `in_query` and `not_in_query` take a
`select` instead of a list.

## Arithmetic

`plus`, `minus`, `times`, `divide` and `modulo` cover `+`, `-`, `*`, `/` and `%`.

```rust
db.select(users).pick(users.age.as_expr().plus(1)).await?;
```

```sql
SELECT ("users"."age" + ?) FROM "users"
```

The five methods exist on a column as well as on `Expr`, so `users.age.plus(1)` is the same
expression. The result keeps the column's SQL type: an `i32` column stays `i32`.

## Conditional

`case` builds `CASE WHEN ... THEN ... ELSE ... END`. The result type is a type parameter, because
a bare literal could be `TEXT`, `VARCHAR(n)` or `CHAR(n)`.

```rust
db.select(users)
    .pick(case::<_, besu::Text>()
        .when(users.age.gte(18), "adult")
        .otherwise("minor"))
    .await?;
```

```sql
SELECT (CASE WHEN ("users"."age" >= ?) THEN ? ELSE ? END) FROM "users"
```

`coalesce` falls back when the first expression is `NULL`, and `cast` changes the SQL type:

```rust
db.select(users)
    .pick((
        coalesce(users.bio.as_expr(), "none"),
        users.age.as_expr().cast::<besu::Text>(),
    ))
    .await?;
```

```sql
SELECT COALESCE("users"."bio", ?), CAST("users"."age" AS TEXT) FROM "users"
```

`COALESCE` is also a column method, `users.bio.coalesce("none")`. `CAST` is a method on `Expr`
only; the target is a `SqlType` such as `besu::Text`, `besu::Int8` or `besu::Float8`.

The type named in a cast is not always the type named in DDL: MySQL's `CAST` accepts `SIGNED`,
`CHAR`, `DOUBLE`, `BINARY`, `DATETIME`, `DECIMAL(p,s)` and a few others, but not `INTEGER`,
`TEXT` or `VARCHAR(n)`. The dialect renders the cast spelling itself, so `cast::<besu::Int8>()`
becomes `CAST(... AS SIGNED)` on MySQL and `CAST(... AS BIGINT)` on PostgreSQL. A custom type is
passed through verbatim, because only you know what the server accepts for it.

## Functions and aggregates

```rust
db.select(users)
    .pick((
        concat(users.name.as_expr(), users.email.as_expr()),
        count_all(),
        count_distinct(users.age),
        sum(users.age),
        avg(users.age),
    ))
    .await?;
```

```sql
SELECT CONCAT("users"."name", "users"."email"), COUNT(*), COUNT(DISTINCT "users"."age"),
       CAST(SUM("users"."age") AS REAL), CAST(AVG("users"."age") AS REAL) FROM "users"
```

`lower`, `upper`, `length`, `trim`, `abs`, `round`, `ceil`, `floor`, `min` and `max` are the
rest; `min` and `max` keep the argument's type. `sum` and `avg` are widened to `f64` and cast in
the database (`REAL` on SQLite, `DOUBLE PRECISION` on PostgreSQL), because a sum of integers can
overflow and PostgreSQL and MySQL return `NUMERIC` for one. `count(expr)` counts non-`NULL`.

`now()` renders `CURRENT_TIMESTAMP`. `json_get` and `json_text` take a dotted key path into a
JSON column — here `events.payload` — and render `->` / `->>` (PostgreSQL `#>` / `#>>`);
`json_get` keeps the JSON type, so comparing needs `json_text`.

```rust
db.select(events)
    .pick((now(), json_get(events.payload, "kind")))
    .filter(json_text(events.payload, "kind").eq("signup"))
    .await?;
```

```sql
SELECT CURRENT_TIMESTAMP, "events"."payload" -> '$.kind' FROM "events"
WHERE ("events"."payload" ->> '$.kind' = ?)
```

`matches` is a full-text search, spelled by the dialect: `to_tsvector(..) @@
plainto_tsquery(..)` on PostgreSQL, `MATCH(..) AGAINST (..)` on MySQL and `MATCH` on SQLite,
where the table has to be an FTS5 virtual table.

## What is missing

Compared with Drizzle's operators, none of the following exist yet — see
[Drizzle parity](../parity.md) and [#86](https://github.com/specta-rs/besu/issues/86):

- `IS DISTINCT FROM` / `IS NOT DISTINCT FROM`, so a `NULL`-safe inequality is `raw` for now.
- Array operators: `@>`, `<@`, `&&`, `||` and the rest.
- Regex operators: `~`, `~*`, `!~`, `SIMILAR TO`.
- `ANY` / `ALL` over an array or a subquery.

## What is checked

| Mistake | When it fails |
| --- | --- |
| A literal or column of another SQL type | Compile error |
| A table that was not joined | Compile error |
| `ILIKE` on SQLite or MySQL, or too many `IN` values | Build error |

## Gotchas

- **`between` is not type-checked against the column.** It takes two values of one type, but
  not the column's type, so a mismatched pair fails in the database.
- **`not_between`, `is_true` and `is_false` are functions, not column methods.**
- **`concat` takes typed expressions, not literals.** Literal text has no `SqlType`; pass a
  column, or `Expr::<_, besu::Text>::raw("'!'")` when it must be a constant.
- **`sum` and `avg` return `f64`.** Annotate `Vec<f64>` when you pick one, and note that
  `json_get` paths use dots — a key containing one cannot be expressed, so write it with `raw`.
