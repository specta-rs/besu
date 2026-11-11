# Updating and deleting

Updates and deletes are the statements that quietly destroy data when a `WHERE` clause is
missing. Besu makes the missing clause a compile error rather than a code-review problem.

```rust
db.update(users)
    .set(users.name, "Alicia")
    .filter(users.id.eq(1))
    .await?;

db.delete(users)
    .filter(users.id.eq(1))
    .await?;
```

## The `all_rows()` safety mechanism

`db.update(table)` and `db.delete(table)` return a builder with a `const SAFE: bool`
parameter set to `false`:

```rust
pub struct Update<'a, E: Executor, T, const SAFE: bool = false>;
pub struct Delete<'a, E: Executor, T, const SAFE: bool = false>;
```

The `IntoFuture` implementation — the thing that makes `.await` work — exists only for
`SAFE = true`. So this does not compile:

```rust
db.delete(users).await?;
```

```text
error[E0277]: `Delete<'_, &Database<Sqlx>, UserTable, false>` is not a future
```

There are exactly two ways to flip the flag:

```rust
// A filter proves the statement is targeted.
db.delete(users).filter(users.id.eq(1)).await?;

// Or you say, in the source, that every row is intended.
db.delete(users).all_rows().await?;
```

`all_rows()` is not a bypass that hides in a helper; it is a method call with a name that
spells out the consequence. It is the only way to write a full-table update or delete.

## `set` versus `set_expr`

`set` binds a Rust value. It is checked against the column's Rust type, including nullability:

```rust
db.update(users)
    .set(users.name, "Alicia")          // String column
    .set(users.age, 22)                 // i32 column
    .set(users.bio, Some("Rust".into()))// Option<String> column
    .filter(users.id.eq(1))
    .await?;
```

A nullable column takes an `Option<T>`, because the Rust type of the column is `Option<T>`.
Passing a bare `String` to a nullable column does not compile; pass `Some(value)` or
`None::<String>`.

`set_expr` computes the new value in the database instead of in Rust:

```rust
db.update(users)
    .set_expr(users.age, users.age.as_expr().plus(1))
    .all_rows()
    .await?;
```

```sql
UPDATE "users" SET "age" = ("users"."age" + ?)
```

That is the difference that matters for counters and timestamps: `set` reads the old value in
Rust and writes a constant, while `set_expr` sends an expression the database evaluates
against the row it is updating. The expression must have the column's SQL type; use
`.as_expr().cast::<...>()` if it does not.

Both methods can be mixed in one statement, and `set_expr` can be called more than once:

```rust
db.update(users)
    .set(users.name, "Alicia")
    .set_expr(users.age, users.age.as_expr().plus(1))
    .filter(users.id.eq(1))
    .await?;
```

## Writing a whole row

`set_model` sets every column from a model, skipping the primary key and any generated column:

```rust
db.update(users)
    .set_model(&changed_user)
    .filter(users.id.eq(changed_user.id))
    .await?;
```

The filter is still yours to write, because `set_model` has no opinion about which rows you
meant — and the type-state that requires a `WHERE` clause is what keeps that honest.

## `RETURNING`

`returning` is available only once the statement is safe, which keeps the unsafe case from
sneaking a result type in:

```rust
let updated: Vec<User> = db
    .update(users)
    .set(users.age, 22)
    .filter(users.id.eq(1))
    .returning(users)
    .await?;

let deleted_ids: Vec<i64> = db
    .delete(users)
    .filter(users.age.lt(18))
    .returning(users.id)
    .await?;
```

It takes the same selections as `pick`: a column, an expression, a tuple or the table handle.
As with inserts, `RETURNING` is dialect-dependent. SQLite and PostgreSQL support it; MySQL
does not, and the failure is a runtime build error rather than a compile error.

## Deleting with a limit

`delete` has a `limit` method:

```rust
db.delete(users)
    .filter(users.age.lt(18))
    .limit(100)
    .await?;
```

It binds the limit as a parameter and emits `DELETE FROM ... LIMIT ?`. SQLite and MySQL
support this; PostgreSQL has no `DELETE ... LIMIT`, so do not use it there — filter the rows
instead. A dialect that cannot express it sets `SUPPORTS_DELETE_LIMIT = false`, which turns
the statement into an `unsupported` build error. A limit on its own is **not** a safety proof:
`db.delete(users).limit(1).await` still does not compile, because a limit bounds the damage
without expressing intent.

`update` has no `limit`.

## Generated SQL

```rust
let sql = db.update(users)
    .set(users.name, "Alicia")
    .filter(users.id.eq(1))
    .to_sql()?;

assert_eq!(
    sql.sql(),
    r#"UPDATE "users" SET "name" = ? WHERE ("users"."id" = ?)"#
);
```

```rust
let sql = db.delete(users).filter(users.age.lt(18)).to_sql()?;

assert_eq!(
    sql.sql(),
    r#"DELETE FROM "users" WHERE ("users"."age" < ?)"#
);
```

## What is checked

| Mistake | When it fails |
| --- | --- |
| `.await` on an update or delete with no `filter`/`all_rows` | Compile error: no `IntoFuture` |
| Setting a column to the wrong type | Compile error at the argument |
| Setting a nullable column to a non-`Option` value | Compile error |
| A `set_expr` whose SQL type does not match the column | Compile error |
| Referencing a column from another table in the filter | Compile error: the row set is only `(T,)` |
| An update with no `set` | Runtime build error |
| `RETURNING` on MySQL | Runtime build error |
| `DELETE ... LIMIT` where the dialect sets `SUPPORTS_DELETE_LIMIT = false` | Runtime build error |

## Gotchas

- **`filter` can only be called once.** The first call returns `SAFE = true`, and the method
  exists only on the unsafe type. Combine conditions with `and(..)` / `or(..)` instead of
  chaining `.filter(..).filter(..)`.
- **An update or delete has no joins.** Its row set is `(T,)`, so you cannot reference a
  joined table in the filter, and there is no `UPDATE ... FROM` / `DELETE ... USING`. Filter
  on a subquery with `raw`, or restructure the statement.
- **`delete(..).limit(..)` is still unsafe.** Add a filter or `all_rows`.
- **`set` on a nullable column needs an `Option`.** `Some(value)` and `None::<String>` are the
  two spellings; `None` without a type annotation usually needs the turbofish.
- **`all_rows` is not required for `to_sql`.** You can render the SQL of an unsafe statement;
  only execution needs the flag.
- **MySQL cannot return the modified rows.** Select them before or after the write, or read
  the driver output's row count.
