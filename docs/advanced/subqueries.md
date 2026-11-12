# Subqueries

A subquery is used in a `WHERE` clause, in three forms: `EXISTS`, `NOT EXISTS` and
`IN (SELECT ...)`.

```rust
// users who have written at least one post
db.select(users)
    .filter(exists(
        db.select(posts)
            .correlate(users)
            .filter(posts.author_id.eq(users.id)),
    ))
    .all()
    .await?;
```

```sql
SELECT ... FROM "users"
WHERE EXISTS (
    SELECT "posts"."id", "posts"."title", "posts"."author_id"
    FROM "posts" WHERE ("posts"."author_id" = "users"."id")
)
```

```rust
// users who have never written a post
db.select(users).filter(not_exists(/* the same subquery */))

// users whose id appears as an author
db.select(users)
    .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
```

```sql
SELECT ... FROM "users" WHERE ("users"."id" IN (SELECT "posts"."author_id" FROM "posts"))
```

## Correlating with the outer query

`db.select(posts)` fixes the subquery's row set to `(Post,)`, so referencing `users.id`
inside it does not compile. `correlate(users)` adds the outer table to the row set without
adding a `FROM` entry, which is exactly what a correlated subquery means.

The check runs in one direction. The subquery may only reference tables that are in scope,
which `correlate` provides. The reverse — requiring the *outer* query to contain the table the
subquery correlates with — is not checkable, because the outer query's row set is not in scope
where the subquery is written. Write the `correlate` call, or the SQL will reference a table
that is not there.

## `IN` and nullability

`in_query` checks that the subquery selects exactly one column, and that its Rust type matches
the column's. A nullable column's `pick` produces `Option<T>`, which is rejected: use
`as_expr()` if you mean to compare against a nullable column.

`IN` follows SQL's three-valued logic: if the subquery produces a `NULL`, `x IN (...)` is
`UNKNOWN` rather than false, and a `NOT IN` over a set containing `NULL` matches **nothing**.
Prefer `EXISTS` when the subquery's column is nullable, which is also usually the faster plan.

## Parameters

The inner statement's parameters continue the outer statement's numbering, so a correlated
subquery binds as one statement:

```sql
SELECT ... FROM "users"
WHERE EXISTS (SELECT ... FROM "posts" WHERE ("posts"."author_id" = "users"."id") AND ("posts"."published" = $1))
```

The inner statement is rendered by the same dialect and is not sealed separately, so a
PostgreSQL driver suffix is appended once, to the whole statement.

## What is checked

- The subquery's own tables must be in its row set; a correlated reference needs
  `correlate(..)`.
- `in_query` requires exactly one projected column, with a matching Rust type.
- `exists` and `not_exists` accept any subquery, because their result is a `Bool` and no
  column type is involved.

## Gotchas

- A scalar subquery in a projection (`SELECT (SELECT count(*) ...) AS n`) and a derived table
  in `FROM` are not expressible yet — [#79](https://github.com/specta-rs/besu/issues/79).
- `NOT IN` over a nullable column silently matches nothing. This is SQL's rule, not Besu's,
  but it is the trap that makes people swear off `NOT IN`.
- A correlated subquery is evaluated per outer row. `EXISTS` with an index on the correlated
  column is fast; without one it is a scan per row, and the builder cannot tell you which.
- A set operation is not accepted as a subquery yet; convert it to one with `sql!` if you need
  it.
- A named subquery is a [common table expression](ctes.md): `Select::cte` names one, and
  `db.from(&cte)` reads it back.
