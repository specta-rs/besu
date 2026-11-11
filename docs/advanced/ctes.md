# Common table expressions

> **Not supported yet.** `WITH` and `WITH RECURSIVE` cannot be expressed in the typed builders.
> Write the whole statement with `sql!` and bind its parameters there.

A common table expression names a subquery so the rest of the statement can refer to it. It is
worth the escape hatch because the cases that need one — a hierarchy walk, a per-group
ranking, a value reused twice — are exactly the cases a builder cannot type for you.

## A recursive CTE

A `WITH RECURSIVE` clause has an anchor term and a recursive term joined by `UNION ALL`. The
only thing Besu does here is rewrite the `?` placeholders and bind the arguments:

```rust
let rows: Vec<(i64, i64)> = db
    .execute(sql!(
        "WITH RECURSIVE tree (id, parent_id, depth) AS (
             SELECT id, parent_id, 0
               FROM nodes
              WHERE parent_id IS NULL
             UNION ALL
             SELECT n.id, n.parent_id, tree.depth + 1
               FROM nodes AS n
               JOIN tree ON n.parent_id = tree.id
         )
         SELECT id, depth
           FROM tree
          WHERE depth <= ?
          ORDER BY depth, id",
        3
    ))
    .all()
    .await?;
```

The argument is `3`, bound as one parameter; on PostgreSQL the rendered statement contains
`$1` at that position and the other two dialects keep `?`. The macro checks that the number of
placeholders and arguments agree at compile time, and rewriting them for the dialect happens
at render time.

## A non-recursive CTE

The same form covers a subquery that would otherwise be repeated. `WITH` is a prefix on the
statement, so it composes with any `SELECT` the database understands.

```rust
let cutoff = chrono::Utc::now() - chrono::Duration::days(7);

let rows: Vec<(String, i64)> = db
    .execute(sql!(
        "WITH recent AS (
             SELECT author_id
               FROM posts
              WHERE created_at >= ?
         )
         SELECT u.name, COUNT(*)
           FROM users AS u
           JOIN recent AS r ON r.author_id = u.id
          GROUP BY u.name
          ORDER BY u.name",
        cutoff
    ))
    .all()
    .await?;
```

## Why the whole statement

A CTE cannot be handed to `db.select(..)`. The builder has no CTE clause — its `with(..)` is
the eager-loading helper, not SQL's `WITH` — and it has no derived table in `FROM` either, so
there is no seam where a raw subquery could be attached to a typed query. The statement is
therefore raw from `WITH` to the final `SELECT`, and the result is decoded positionally into
whatever type you name.

The [parity scoreboard](../parity.md) calls the query-layer gaps — aliases, CTEs and derived
tables — the ones that bite first, though no issue tracks them yet.

That is also why the row type has to be spelled out. `sql!` produces a `Fragment`, and
`Database::execute` wraps it in a `RawQuery`, which decodes only through `all::<T>()` or
`first::<T>()`. The macro cannot infer the row type from the SQL text, so name it at the
binding or on the call.

```rust
let first: Option<(i64, i64)> = db
    .execute(sql!(
        "WITH RECURSIVE tree (id, depth) AS (
             SELECT id, 0 FROM nodes WHERE parent_id IS NULL
             UNION ALL
             SELECT n.id, tree.depth + 1 FROM nodes n JOIN tree ON n.parent_id = tree.id
         )
         SELECT id, depth FROM tree ORDER BY depth DESC LIMIT 1"
    ))
    .first()
    .await?;
```

## Gotchas

- **Decoding is positional.** `(i64, i64)` means the first two selected columns, in order. A
  `SELECT *` from a CTE is fragile; list the columns.
- **Nothing checks the SQL.** A misspelled column, a cycle in the recursive term or a missing
  `RECURSIVE` keyword is a database error at run time. `sql!` checks arity, not meaning.
- **The statement is one piece.** There is no way to fold a CTE result back into a typed
  `select`, so joins, aliases and column lists inside it are hand-written SQL.
- **A CTE does not inherit the table prefix.** `Database::prefixed` rewrites table names in
  built statements; raw SQL is not rewritten, so write the prefixed names yourself.
- **The recursion limit is the database's.** SQLite, PostgreSQL and MySQL each have their own
  behaviour for a runaway recursive term; Besu adds no guard.
