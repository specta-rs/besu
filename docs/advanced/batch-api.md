# Batch API

> **Not supported yet.** Besu has no `db.batch([..])`. One builder is one statement, and
> awaiting it is one round trip.

Drizzle's `db.batch([..])` takes an array of queries, sends them in a single request and
returns a typed tuple of their results. It is a latency feature: atomicity still comes from a
transaction. Besu has no batch, so the honest options are to await each statement or to put
them in one transaction.

## Await each statement

Two independent statements are two round trips. That is the same SQL Drizzle would send,
without the request batching.

```rust
let users = db.select(users).filter(users.age.gte(18)).await?;
let posts = db.select(posts).order_by(posts.created_at.desc()).limit(20).await?;
```

This is the right shape when the statements are independent: one failing does not have to undo
the other, and the second does not need the first's result.

## Atomic: one transaction

When the statements must land together, wrap them in `db.transaction(..)` and return a tuple.
That is the closest thing to a batch Besu has: it is still one round trip per statement, plus
`BEGIN` and `COMMIT`.

```rust
let (user, post) = db
    .transaction(async |tx| {
        let user = tx
            .insert(users)
            .value(users.insert("oscar@example.com", "Oscar", 21))
            .returning(users)
            .await?
            .remove(0);

        let post = tx
            .insert(posts)
            .value(posts.insert("Hello, world", user.id))
            .returning(posts)
            .await?
            .remove(0);

        Ok::<_, sqlx::Error>((user, post))
    })
    .await?;
```

The closure is where the dependency lives: the post needs `user.id`, which a batch of
pre-built statements could not express. MySQL has no `RETURNING`, so on MySQL select the rows
back by key after the insert instead.

## Bulk inserts are a different problem

`values` emits one multi-row `INSERT`, which is one round trip for the whole set:

```rust
db.insert(users)
    .values([
        users.insert("a@example.com", "Alice", 30),
        users.insert("b@example.com", "Bob", 20),
    ])
    .await?;
```

A very large iterator can exceed the dialect's parameter limit, so `values_chunked` splits it
into several statements of a fixed row count:

```rust
let outputs = db
    .insert(users)
    .values_chunked(rows, 1_000)
    .await?;
```

The chunks are **not** atomic; each statement commits on its own unless the whole call runs
inside a transaction. The parameter limit is enforced per statement, and `values_chunked` is
how a bulk load gets past it.

No batch API is tracked. It would need a driver method that accepts several statements and
returns several results in one exchange; `Driver` has only `query`, `execute` and
`query_stream` today.

## Gotchas

- **Batching is a latency feature, not a correctness one.** Each `await` here is a full round
  trip, so a loop of inserts over a network is slow by construction; move the work into one
  multi-row insert or one transaction.
- **There is no pipelining.** The next statement is not sent until the previous future
  resolves, even on a pool with idle connections. Awaiting concurrently with `join!` uses
  several connections, which is a different guarantee and can exhaust a small pool.
- **A transaction is not one round trip.** It is `BEGIN`, then each statement, then `COMMIT`.
  It buys atomicity and a single connection, not fewer messages.
- **`values_chunked` is not a batch.** The statements are independent, and a failure part-way
  leaves the earlier chunks committed.
- **`RETURNING` is not portable.** PostgreSQL and SQLite have it; MySQL does not, so a batch
  that needs generated ids is written differently there.
