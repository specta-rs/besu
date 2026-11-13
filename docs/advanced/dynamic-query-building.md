# Dynamic query building

A query in Besu *is* a value. A builder is a plain struct with a type parameter for the
executor, so it can be bound to a variable, built conditionally, returned from a function and
handed to another function. There is no separate dynamic mode to opt into.

## Building conditionally

`filter` consumes the builder and returns the same type, so both branches of an `if` agree and
the result stays a normal query.

```rust
let query = db.select(users);

let query = if adult_only {
    query.filter(users.age.gte(18))
} else {
    query
};

let query = if let Some(pattern) = name {
    query.filter(users.name.like(pattern))
} else {
    query
};

let rows: Vec<User> = query.await?;
```

The projection is chosen the same way. Because both branches must have the same `Select<..>`
type, a branch that calls `pick` changes the row type and will not type-check against a branch
that does not — which is the point.

```rust
let query = db.select(users);
let query = if names_only {
    query.pick(users.name)
} else {
    query.pick((users.id, users.name))
};
// This does not compile: Vec<String> and Vec<(i64, String)> are different row types.
```

## A helper generic over `Executor`

The executor is a type parameter, so a function can take a query, adjust it and give it back
without knowing whether it will run on a pool, a transaction or a dry database.

```rust
use besu::{Executor, Select, prelude::*};

fn scope<'a, E: Executor>(
    query: Select<'a, E, (UserTable,), User>,
    min_age: Option<i32>,
    name: Option<&str>,
) -> Select<'a, E, (UserTable,), User> {
    let query = match min_age {
        Some(age) => query.filter(users.age.gte(age)),
        None => query,
    };

    match name {
        Some(pattern) => query.filter(users.name.like(pattern)),
        None => query,
    }
}

let adults: Vec<User> = scope(db.select(users), Some(18), None).await?;
```

The same function works inside a transaction, because `&mut Transaction<'_, Dr>` is an
`Executor` too:

```rust
db.transaction(async |tx| {
    let rows = scope(tx.select(users), Some(18), Some("A%")).await?;
    Ok::<_, sqlx::Error>(rows)
})
.await?;
```

If you only ever pass a database, the concrete signature is simpler:

```rust
async fn find_users<Dr: besu::Driver>(
    db: &besu::Database<Dr>,
    min_age: Option<i32>,
) -> Result<Vec<User>, Dr::Error>
where
    User: besu::Decode<Dr>,
{
    let query = db.select(users);
    let query = match min_age {
        Some(age) => query.filter(users.age.gte(age)),
        None => query,
    };
    query.await
}
```

## Inspecting a query without consuming it

`to_sql` is the one operation that inspects a query without consuming it: it takes `&self`, so
a value can be rendered for a log and then run.

```rust
let query = db.select(users).filter(users.age.gte(18));
let sql = query.to_sql()?;
tracing::debug!(sql = sql.sql(), "running");
let rows: Vec<User> = query.await?;
```

## The dynamic escape hatch

The typed builder is the default: it proves that every column comes from a table the query
selects from and that comparisons line up on both sides. Sometimes the shape of a query is
chosen at runtime in a way the type-state cannot follow — a list of conditions built in a
loop, a filter handed in by another function, a clause that is present only sometimes.
`Select::dynamic` hands back a `DynamicSelect`: the row type `R` is kept, so the result still
decodes into the shape `pick` chose, but the row-set parameter is erased from the clauses.

```rust
use besu::{ExprKind, OrderClause};

let filter: Expr<(UserTable,), Bool> = users.age.gte(18);
let order: OrderClause = users.name.asc().into_order();

let rows: Vec<User> = db
    .select(users)
    .dynamic()
    .filter(filter.into_kind())
    .order_by(order)
    .limit(50)
    .all()
    .await?;
```

`DynamicSelect` takes an already-erased `ExprKind` for `filter`, `having`, `group_by` and
`pick`, and an `OrderClause` for `order_by`. `Expr::into_kind` and `Expr::into_order` are the
way to erase a typed expression; build the expression while its row set is still known, then
hand the erased value to the handle.

```rust
let mut query = db.select(users).dynamic();
for condition in conditions {
    query = query.filter(condition.into_kind());   // each condition: Expr<(UserTable,), Bool>
}
let rows: Vec<User> = query.all().await?;
```

Everything the builder would have checked is the database's problem from here: a column from
a table that is not in `FROM`, a comparison between incompatible types, and a projection whose
columns do not match `R` all reach the server. The handle is an escape hatch, not the happy
path. `into_select()` hands the query back to the typed builder once the dynamic part is done;
its row-set parameter starts empty, so a later `filter` needs `correlate` to name anything.

## Counting a query

`Select::count` and `SetQuery::count` return `SELECT COUNT(*) FROM (<statement>) AS
"besu_count"`, so the filters, joins, grouping and pagination of the statement still apply:

```rust
let total: i64 = db
    .select(users)
    .filter(users.age.gte(18))
    .limit(100)
    .count()
    .first()
    .await?
    .expect("COUNT always returns a row");
```

## When the row shape itself is dynamic

The row type is fixed once it is chosen. `Select<'a, E, M, R>` carries `R`, and `pick` changes
it, so a function that sometimes returns users and sometimes returns counts cannot return one
type. `DynamicSelect` keeps `R`, so it does not help there. A genuinely heterogeneous query —
where the columns are not known at the call site — has to be raw SQL.

```rust
use besu::Fragment;

let mut fragment = Fragment::new("SELECT id, name FROM users WHERE 1 = 1");
if let Some(min_age) = min_age {
    fragment.push(Fragment::with_args(" AND age >= ?", vec![min_age.into()]));
}
if let Some(pattern) = name {
    fragment.push(Fragment::with_args(" AND name LIKE ?", vec![pattern.to_string().into()]));
}

let rows: Vec<(i64, String)> = db.execute(fragment).all().await?;
```

`Fragment` checks the placeholder count at render time rather than compile time, and the row
still decodes positionally into the type you name. Once even the row shape is dynamic, you are
outside the typed builder; see [Raw SQL](../queries/sql.md).

## What is checked

| Mistake | When it fails |
| --- | --- |
| Branches of an `if` with different row types | Compile error at the variable's type |
| A helper that names the wrong executor | Compile error: `Executor` is not implemented |
| A column from a table that is not in the query | Compile error through `Contains` |
| A clause on a `DynamicSelect` that names the wrong table or type | Database error: the row set is erased |
| A `Fragment` whose placeholders and arguments disagree | Runtime, while rendering |
| A raw query whose columns do not match the tuple | Runtime decode error |

## Gotchas

- **A query borrows its executor.** `Select<'a, E, ..>` carries `'a`, so storing one in a
  struct means carrying the lifetime of the database or transaction as well.
- **`to_sql` clones the query.** It is cheap for inspection, but calling it in a loop and then
  running the builder pays for both.
- **`filter` combines with `AND`, always.** A conditionally built `OR` needs one `or(..)`
  expression, not two `filter` calls.
- **A helper that returns `Select` freezes the table set.** `(UserTable,)` will not accept a
  joined query; give the helper the exact row-set type it needs, or take a closure.
- **`dynamic` gives up the compile-time check.** `DynamicSelect` keeps the row type but not
  the row set, so use it for the dynamic part and `into_select` afterwards where you can.
