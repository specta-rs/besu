# Transactions

A transaction runs several statements on one connection and makes them visible together, or
not at all. Besu exposes the driver's transaction directly: it does not invent a second
transaction model on top of it.

## Begin, commit, rollback

`Database::begin` is available when the driver implements `TransactionDriver`:

```rust
let mut tx = db.begin().await?;

tx.insert(users)
    .value(users.insert("oscar@example.com", "Oscar", 21))
    .await?;

tx.update(users)
    .set(users.age, 22)
    .filter(users.email.eq("oscar@example.com"))
    .await?;

tx.commit().await?;
```

While the transaction is alive, it has the same query surface as the database:

```rust
tx.select(users).filter(users.age.gte(18)).await?;
tx.insert(users).value(..).await?;
tx.update(users).set(..).all_rows().await?;
tx.delete(users).filter(..).await?;
tx.execute(sql!("SELECT 1")).all::<(i64,)>().await?;
```

Every one of those calls runs on the transaction's connection, which is the whole point: a
statement issued on `db` while the transaction is open uses a different connection from the
pool and will not see the uncommitted rows.

`commit` and `rollback` consume the transaction, so a committed transaction cannot be used
again and a rolled-back one cannot be committed by accident:

```rust
tx.commit().await?;
// tx is gone
```

Dropping a `Transaction` without committing rolls back. The underlying `sqlx` transaction
does this on drop, and Besu's wrapper does not change that. It is a safety net, not a
substitute for saying what you mean: an explicit `rollback` reports its error, while a drop
cannot.

## The `transaction()` helper

The common shape — begin, run a closure, commit on success, roll back on error — is built in:

```rust
let user_id: i64 = db
    .transaction(async |tx| {
        let user = tx
            .insert(users)
            .value(users.insert("oscar@example.com", "Oscar", 21))
            .returning(users)
            .await?
            .remove(0);

        Ok(user.id)
    })
    .await?;
```

The closure is an `AsyncFnOnce(&mut Transaction<'_, Dr>) -> Result<T, Dr::Error>`. Write it as
an async closure:

```rust
db.transaction(async |tx| {
    tx.insert(users).value(..).await?;
    Ok(())
})
.await?;
```

A plain closure returning an `async move` block does not satisfy the higher-ranked lifetime
bound: the `&mut Transaction` borrow cannot be captured that way. Use the async-closure form,
which is what the bound was written for.

The error type is the driver's error, not a Besu error, so `?` inside the closure works
whenever the surrounding function returns that type (or boxes it).

On `Ok`, the helper commits and returns the value. On `Err`, it rolls back and returns the
original error. If the rollback itself fails, that failure is discarded — the error you asked
about is the one you get, which is the useful one.

## Nested transactions

Every supported database spells a nested transaction as a *savepoint*, and so does Besu.
`Transaction::transaction` opens one with a generated name and releases it on success; on
error it rolls back to the savepoint and returns the error, leaving everything before it in
place:

```rust
db.transaction(async |tx| {
    tx.insert(users).value(users.insert(..)).await?;

    // Swallowing the error keeps the insert above and discards this one.
    let _ = tx
        .transaction(async |tx| {
            tx.insert(audit).value(audit.insert(..)).await?;
            Err::<(), _>(some_error)
        })
        .await;

    Ok(())
})
.await?;
```

`Transaction::savepoint(name, ..)` is the same with a name you choose, which is what you want
when several nest. The name is written into `SAVEPOINT`, so it is checked for ASCII letters,
digits and underscores rather than quoted; an invalid name is an error before a statement
runs.

A savepoint needs no guard object and nothing to drop. One that is neither released nor
rolled back is released when the enclosing transaction commits.

## What is not here

- **No isolation-level API of its own.** `begin()` uses the driver's default. Set it in the
  connection options or execute `SET TRANSACTION` yourself with `tx.execute(sql!(..))`.
- **No implicit retry.** A serialisation failure is returned as the driver's error; retrying
  is the application's decision.
- **Migrations are already transactional.** `Database::migrate` begins a transaction, runs the
  whole set on it, and commits or rolls back. `Migrator::run` is generic over any `Executor`
  if you need a different boundary.

## Gotchas

- **Do not use `db` for work that belongs to `tx`.** On a pool, `db` checks out a different
  connection and cannot see uncommitted data. On SQLite in-memory, `Sqlx::memory()` pins the
  pool to one connection, so the two may accidentally work — do not rely on that.
- **A `Transaction` borrows the database.** You cannot move the `Database` while a transaction
  is alive; the borrow checker enforces it.
- **Rollback on drop is best-effort.** If you care whether the rollback succeeded, call
  `rollback().await` and handle the error.
- **The helper's closure is `AsyncFnOnce`.** It runs once and consumes its captures; it is
  not a retry loop.
- **`commit` can fail.** A deferred constraint or a connection failure can surface at commit
  time, after every statement succeeded. Handle the error from `commit`, not just the ones
  inside the transaction.
