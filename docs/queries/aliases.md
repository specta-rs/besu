# Aliases

> **Not supported yet.**

A self-join needs two names for one table — `employees AS a JOIN employees AS b` — and there is
no `alias()` method to give a handle a second name. Joining the same handle twice does not even
compile, because the row set contains the table twice and the containment check cannot tell
which copy a column refers to:

```rust
#[derive(Table)]
#[besu(table = "employees")]
pub struct Employee {
    #[besu(primary_key)] pub id: i64,
    pub name: String,
    pub manager_id: i64,
}

// Does not compile.
db.select(employees).inner_join(employees, eq(employees.id, employees.manager_id));
```

The error is `E0283: type annotations needed`, because several `Contains` implementations match.

## The workaround

Write the self-join with [`sql!`](sql.md) and explicit aliases, then decode each side as the
model it is. `SELECT a.*, b.*` puts the two rowsets one after the other, which is exactly what
`(Employee, Employee)` expects.

```rust
let pairs: Vec<(Employee, Employee)> = db
    .execute(sql!(r#"SELECT a.*, b.* FROM "employees" a
                     JOIN "employees" b ON a."manager_id" = b."id""#))
    .all()
    .await?;
```

The other workaround is two differently-named tables: a `managers` table, or a view over
`employees`, lets the join stay typed — `inner_join(managers, eq(employees.manager_id,
managers.id))`. That is a schema change, not a query change, so it is only reasonable when the
second name means something on its own.

Aliases are on the [roadmap](../roadmap.md#smaller-items) as
[#77](https://github.com/specta-rs/besu/issues/77), because they are the first thing a Drizzle
user reaches for and cannot express. The [Drizzle parity](../parity.md) page lists the same gap.

## Gotchas

- **`sql!` is not checked.** The aliases, the column names and the join condition are all text;
  a mistake is a database error or a decode error.
- **`SELECT *` is order-sensitive.** `a.*, b.*` decodes correctly because the model reads
  columns by position.
- **`#[besu(table = "..")]` is not an alias.** It names the table the struct maps to; two structs
  cannot share one name and be told apart in a query.
