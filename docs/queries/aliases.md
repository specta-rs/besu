# Aliases

`alias(table, "name")` gives a table a second name inside one statement. The reason to want
one is the self-join: "employees and their managers" reads the same table twice, and the two
occurrences need different names in the SQL.

```rust
#[derive(Table)]
#[besu(table = "employees")]
pub struct Employee {
    #[besu(primary_key)] pub id: i64,
    pub name: String,
    pub manager_id: Option<i64>,
}

let manager = alias(employees, "manager");
let pairs: Vec<(Employee, Option<Employee>)> = db
    .select(employees)
    .left_join(&manager, manager.column(employees.id).eq(employees.manager_id))
    .all()
    .await?;
```

```sql
SELECT "employees"."id", "employees"."name", "employees"."manager_id",
       "manager"."id", "manager"."name", "manager"."manager_id"
FROM "employees"
LEFT JOIN "employees" AS "manager" ON ("manager"."id" = "employees"."manager_id")
```

## Reading from an alias

An alias is a `TableRef`, so it is a source as well as a join target:

```rust
let manager = alias(employees, "manager");
let everyone: Vec<Employee> = db.select(&manager).all().await?;
```

```sql
SELECT "manager"."id", "manager"."name", "manager"."manager_id"
FROM "employees" AS "manager"
```

`db.select(&manager)` yields `Employee`, because an alias renames a table rather than
projecting a new shape, so the default projection is the table's own columns in the same
order.

## Columns

`#[derive(Table)]` gives a table handle one field per column, but it can only do that for a
name known at compile time. A derived table solves the same problem with
[`Derived::column`], and an alias solves it the same way:

```rust
manager.column(employees.email)      // "manager"."email"
manager.column(employees.manager_id) // "manager"."manager_id"
```

The argument is the base table's column handle, so the Rust type and the SQL type are the
ones the schema declares and a column of another table is rejected. The result is an
expression, qualified by the alias; it can be compared, filtered, ordered, grouped and
projected like any other expression.

The row set carries `Alias<Employee, 0>` (or the tag you passed to `alias_as`) rather than
`Employee`, so `manager.column(...)` only compiles in a query that actually joined that alias
— and `employees.email` and `manager.column(employees.email)` are different columns even
though they are one column of one table.

## Two aliases of one table

A self-join can need the table more than twice: employees, their manager, and the manager's
manager. [`alias`] always produces tag `0`, so a second alias uses [`alias_as`] with another
tag:

```rust
let report = alias(employees, "report");              // Alias<Employee, 0>
let manager = alias_as::<_, 1>(employees, "manager"); // Alias<Employee, 1>

let rows: Vec<(Employee, Employee)> = db
    .select(report)
    .inner_join(
        &manager,
        manager.column(employees.id).eq(report.column(employees.manager_id)),
    )
    .all()
    .await?;
```

```sql
SELECT "report"."id", "report"."name", "report"."manager_id",
       "manager"."id", "manager"."name", "manager"."manager_id"
FROM "employees" AS "report"
INNER JOIN "employees" AS "manager" ON ("manager"."id" = "report"."manager_id")
```

The tag is a type-level marker and nothing else: it is not written to the SQL, and both
aliases could even share a tag if they were never in the same statement. What it buys is that
the row set holds two entries instead of one, so `Contains` finds exactly one path for each
alias. `manager.column(...)` then only compiles in a query that joined the tag-`1` handle;
using it where the report's column was meant is a compile error rather than a query that
silently compares the wrong side.

The tag cannot be inferred, so name it at the call site — `alias_as::<_, 1>(employees,
"manager")`. The `_` is the table handle, which the argument supplies.

Any `u64` works, and the tags only have to differ within one statement. A named constant
reads better than a literal when the handle crosses a function boundary:

```rust
const MANAGER: u64 = 1;
let manager = alias_as::<_, MANAGER>(employees, "manager");
```

An existing handle can be re-tagged with `alias(..).tag::<N>()`; the name stays the same and
the result is a different alias as far as the row set is concerned, so choose the tag before
joining it.

[`alias`]: https://docs.rs/besu/latest/besu/fn.alias.html
[`alias_as`]: https://docs.rs/besu/latest/besu/fn.alias_as.html

## Gotchas

- **The name is written into the SQL, not bound.** It is quoted, so it may be any identifier,
  but passing a value from another source would be an injection.
- **The name must be unique in the statement.** The builder does not rename colliding
  aliases, because silently changing a name would change which table a column reference
  meant.
- **An alias is read-only.** `insert`, `update` and `delete` take the declared table handle,
  which is the only thing that knows the physical name.
- **A prefix applies to the table, not the alias.** On a
  [`Database::prefixed`](../schema/schemas.md) database the statement reads
  `FROM "tenant_1_employees" AS "manager"` and refers to `"manager"."id"`, because the alias
  is a name in the statement rather than a table in the database.
- **A column used only in `order_by` or `group_by` may need its row set pinned.** Those
  builders do not tie the expression's row set to the query's, so
  `db.select(&manager).order_by(manager.column(users.id))` as the *first* mention of the
  alias is ambiguous. Using the expression in a `filter`, a `pick` or a join condition first
  — or binding it to a variable that is used there — resolves it. The same applies to
  [`Derived::column`] and [`Cte::column`].
- **`#[besu(table = "..")]` is not an alias.** It names the table the struct maps to; two
  structs cannot share one name and be told apart in a query.

[`Derived::column`]: select.md
[`Cte::column`]: ../advanced/ctes.md
