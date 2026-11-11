# Raw SQL

The typed builders cover the statements Besu models. When you need something they do not —
a window function, a database-specific `EXPLAIN`, a DDL statement — `sql!` gives you raw SQL
that still binds its parameters and still checks its placeholder count at compile time.

```rust
use besu::prelude::*;

let rows: Vec<User> = db
    .execute(sql!("SELECT * FROM users WHERE age > ? ORDER BY age", 18))
    .all()
    .await?;
```

## The `sql!` macro

`sql!` takes a string literal followed by zero or more arguments:

```rust
let fragment = sql!(
    "SELECT * FROM users WHERE age > ? AND name LIKE ?",
    18,
    "%oscar%",
);
```

It produces a `Fragment`: SQL text plus a `Vec<Value>`. The macro enforces two things:

1. **The SQL must be a literal.** The placeholder count is computed in a `const` block, so the
   compiler has to know the text.
2. **The number of `?` placeholders must equal the number of arguments.** A mismatch is a
   compile error, not a runtime surprise:

   ```text
   error[E0080]: evaluation of constant value failed
   ...
   besu::sql!: the SQL has a different number of `?` placeholders than arguments
   ```

Each argument is converted with `Into<Value>`, so the usual Rust types work: integers, floats,
`bool`, `String`, `&str`, `Vec<u8>`, `Uuid`, the `chrono` types, `serde_json::Value`, and
`Option<T>` (where `None` becomes `NULL`).

## Placeholder rewriting

`sql!` always uses `?`, because that is the one spelling every dialect's source form can
share. When the fragment is rendered for a dialect, each placeholder is rewritten:

| Dialect | `?` becomes |
| --- | --- |
| SQLite | `?` |
| MySQL | `?` |
| PostgreSQL | `$1`, `$2`, ... |

The rewriting scanner understands quoting: a `?` inside `'...'`, `"..."` or `` `...` `` is
left alone, and a doubled quote inside a quoted section is treated as an escape rather than
the end of the string. So this is a single-bound-parameter query with a literal `?` in the
pattern:

```rust
sql!("SELECT * FROM posts WHERE title LIKE ?", "%why?%")
```

There is no escape for a `?` *outside* quotes. That is a real limitation for operators such as
PostgreSQL's JSON `?`; see the [roadmap](../roadmap.md).

## Running a fragment

`Database::execute` wraps a fragment in a `RawQuery`:

```rust
// Awaiting runs the statement and returns the driver's output.
db.execute(sql!("DELETE FROM sessions WHERE expires_at < ?", cutoff))
    .await?;

// `.all()` decodes every row into the type you name.
let rows: Vec<User> = db.execute(sql!("SELECT * FROM users")).all().await?;

// `.first()` returns Option<R>.
let row: Option<User> = db.execute(sql!("SELECT * FROM users")).first().await?;
```

The row type is the same `Decode` machinery the typed builders use. A model decodes one
column per field in declaration order, a tuple decodes one column per element, and a scalar
decodes one column. That means a hand-written `SELECT *` only works if the column order
matches the struct; a `SELECT` of a subset has to be decoded into a tuple of the right shape.

`RawQuery::to_sql()` renders the fragment without running it, which is useful in tests:

```rust
let sql = db.execute(sql!("SELECT 1")).to_sql()?;
assert_eq!(sql.sql(), "SELECT 1");
```

## When to reach for the escape hatches

There are several, and they trade checking for reach:

| Escape hatch | What is checked | Use it when |
| --- | --- | --- |
| `sql!` | Placeholder count at compile time | You need raw SQL with bound parameters |
| `Fragment::new` / `with_args` | Placeholder count at render time only | The SQL text is genuinely dynamic, or you need to build a fragment in pieces |
| `besu::raw(..)` / `Expr::raw(..)` | Nothing | A single expression Besu does not model |
| `Sql::raw(..)` | Nothing | DDL or a statement with no parameters, inside the schema layer |
| `#[besu(default_sql = "..")]` | Nothing | A database-specific column default |
| A hand-written `TableDef` | Nothing | Composite indexes, composite foreign keys or custom types |

`Fragment` is public and can be assembled:

```rust
use besu::Fragment;

let mut fragment = Fragment::new("SELECT * FROM users WHERE 1 = 1");
fragment.push(Fragment::with_args(" AND age > ?", vec![18.into()]));
```

`Fragment::with_args` does not check the count at compile time, but `render` does check it at
runtime and records an `Error::InvalidQuery` if they disagree. Prefer `sql!` unless the text
really is not a literal.

## The risks, stated plainly

`sql!` checks arity, not meaning. It cannot know that `users` exists in the database, that
`age` is an integer, or that the column order matches `User`. Those mistakes become database
or decode errors at runtime:

| Mistake | Result |
| --- | --- |
| A column or table that does not exist | Database error |
| A parameter of the wrong type | Database error, or a lossy encode in a permissive dialect |
| Selecting columns in a different order than the model | Decode error, or silently swapped fields if the types happen to match |
| A `?` in the wrong place | Wrong parameter count at compile time or a malformed statement |
| User input interpolated into the SQL string | SQL injection |

The last row is the important one. `sql!` binds each argument; it never interpolates it. But
the SQL *text* is used verbatim, so building that text with `format!` and user input
reintroduces the problem the rest of the library is designed to prevent:

```rust
// Do not do this.
let table = user_supplied_table_name;
db.execute(Fragment::new(format!("SELECT * FROM {table}"))).await?;
```

There is no API that formats a value into SQL, and `Fragment` is the only place where raw text
enters the query path. Keep it literal.

## Which raw paths exist

- `sql!` → `Fragment` → `db.execute(..)` for a whole statement.
- `besu::raw("..")` / `Expr::raw("..")` for one boolean expression, usually inside `filter`.
- `Expr` has no public constructor from an `ExprKind`; the expression escape hatch is the
  string form.
- `Sql::raw` is the schema layer's tool. It is not exposed through `Database::execute`, which
  takes a `Fragment`.
- `DefaultValue::Raw` reaches DDL only.

## What is checked

| Check | `sql!` | `Fragment::with_args` | `raw()` |
| --- | --- | --- | --- |
| Placeholder/argument count | Compile time | Render time | n/a |
| Arguments are bound, not interpolated | Yes | Yes | n/a |
| Column names exist | No | No | No |
| Column types match | No | No | No |
| Dialect supports the syntax | No | No | No |

## Gotchas

- **A `?` outside quotes is always a placeholder.** There is no escape sequence, so operators
  like PostgreSQL's JSON `?` cannot be expressed in `sql!`.
- **The macro only accepts a literal.** For a runtime string, use `Fragment::with_args` and
  accept that the arity check moves to render time.
- **`SELECT *` is order-sensitive.** The model decodes by position, not by name.
- **`RawQuery::first` currently returns the last row.** Its implementation collects all rows
  and calls `Vec::pop`, so with more than one result it gives you the final row. Use `.all()`
  and take the element you want until this is fixed.
- **Awaiting a `RawQuery` does not decode anything.** It returns the driver output. You need
  `.all()` or `.first()` to get rows.
- **`sql!` does not validate the dialect.** A statement valid on SQLite may be a syntax error
  on MySQL; the macro cannot see the connection.
