# Batch API

`db.batch((..))` runs several statements in one transaction and returns a typed tuple of what
each one reported.

```rust
let (inserted, everyone) = db
    .batch((
        db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
        // Runs after the insert, in the same transaction, so it sees the new row.
        db.select(users).filter(users.age.gte(18)),
    ))
    .await?;
```

The first element is the insert's driver output — usually the number of affected rows — and
the second is the select's decoded rows, `Vec<User>`.

## What can be in a batch

| Element | Example | Output |
| --- | --- | --- |
| `INSERT`, `UPDATE`, `DELETE` | `db.insert(users).value(..)` | the driver's output, usually affected rows |
| `SELECT` | `db.select(users).filter(..)` | `Vec<R>` |
| `SetQuery` | `db.select(a).union(db.select(b))` | `Vec<R>` |
| `DynamicSelect` | `db.select(users).dynamic()` | `Vec<R>` |
| `INSERT` / `UPDATE` / `DELETE ... RETURNING` | `db.insert(users).returning(users)` | `Vec<R>` |

Each element keeps its own output type, so one tuple can mix them. A select and a write in
the same batch return `(Dr::Output, Vec<User>)`; two selects return `(Vec<User>, Vec<User>)`;
nothing is boxed or erased. The tuple takes one to eight elements, and every element is
rendered before the transaction is opened.

## Results come back in tuple order

The tuple has one slot per statement, in the order they were listed. A select is decoded when
its turn comes — one `Vec<R>` per select, the same rows `await`ing it on its own would give —
so a select after a write sees what that write wrote, even though nothing has committed yet.

Decoding happens statement by statement: the driver collects a select's rows, the batch decodes
them into `R`, and moves on. Rows are never cloned or buffered across statements.

## Atomicity

A batch is atomic by default, matching Drizzle. If any statement fails, the transaction is
rolled back and the error is returned; nothing the batch wrote stays, including the rows the
earlier statements inserted or changed — and, just as importantly, the rows a select earlier in
the tuple read. If every statement succeeds, the transaction commits once.

That is the difference between a batch and awaiting the statements individually:

```rust
// If the second insert fails, the first one stays.
db.insert(users).value(users.insert("a@example.com", "Alice", 30)).await?;
db.insert(users).value(users.insert("b@example.com", "Bob", 31)).await?;
```

```rust
// If the second insert fails, neither row exists.
db.batch((
    db.insert(users).value(users.insert("a@example.com", "Alice", 30)),
    db.insert(users).value(users.insert("b@example.com", "Bob", 31)),
))
.await?;
```

A failed statement is reported as the driver's own error type, the same one every other
statement returns, so there is no second error type to handle. On failure the tuple is not
returned at all: a partial result would invite a caller to act on a read that was rolled back.

## One `await`, not one round trip

This is the part worth being honest about. PostgreSQL has a pipeline protocol, but neither
`tokio-postgres` nor sqlx exposes it, and the SQLite and MySQL drivers do not pipeline at all.
So a batch sends its statements **sequentially** over one connection, exactly as a sequence of
`await`s would.

What a batch buys is:

- one `await` instead of one per statement,
- one transaction, without a closure or a manual `BEGIN`/`COMMIT`,
- a typed tuple of outputs, with a select's rows included.

What it does not buy is fewer messages to the server. If the round-trip count is the problem, a
multi-row `INSERT` is the tool — see [Bulk inserts](#bulk-inserts-are-a-different-problem).

The statements run in the order the tuple lists them. A later statement can read a row an
earlier one wrote, which is why they cannot be reordered or run concurrently.

## A batch is a tuple, not a list

The arity is fixed: one tuple implementation per size, one to eight. That is what lets each
element name its own output type, so a write and a select can share the tuple and each slot
keeps the type it should have. A dynamically built list of statements would have to give every
element the same output type, which erases exactly the information a mixed read/write batch
exists to preserve.

When the number of statements is decided at run time, build them in a loop and use
[`db.transaction`](transactions.md) instead; the closure can hold a `Vec` of results.

## Reads can be in a batch, but their results cannot shape it

A batch is built before anything runs, so a select's output cannot decide what the next
statement is:

```rust
// Does not compile: the insert cannot read `rows`, because `rows` only exists after the
// batch has run.
db.batch((
    db.select(users).filter(users.email.eq("a@example.com")),
    db.insert(posts).value(..rows[0].id..),
)).await?;
```

Use [`db.transaction`](transactions.md) when one statement needs a value an earlier one
produces: the closure can use each statement's result. A batch is for statements that are
independent except for what you want to read back.

## The safe-statement rule still applies

`UPDATE` and `DELETE` need a `WHERE` clause, or an explicit `all_rows()`, before they can run.
A batch does not bypass that check, so `db.update(users)` with no filter does not compile as a
batch element.

## `RETURNING` inside a batch

`returning(..)` is allowed: it reports rows rather than an affected-row count, which the tuple
records as `Vec<R>`. It is only available where the dialect supports it — PostgreSQL and
SQLite do, MySQL does not, and on MySQL the batch fails while rendering, before a transaction
is opened. A MySQL batch that needs to read what it wrote uses a plain insert followed by a
select in the same tuple; the select sees the uncommitted row.

## When to use a batch, and when not to

A batch is for statements that belong together and do not need each other's results to be
built:

- creating a user and their first settings row,
- moving a row and writing the audit entry that explains the move,
- writing a row and reading back the rows that changed, in one transaction,
- seeding a fixed set of rows.

It is the wrong shape when one statement needs an id or a value that an earlier statement
generates — a batch is built before any of it runs, so there is nothing to read at build time.
Use [`db.transaction`](transactions.md) there: the closure can use each statement's result.

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
inside a transaction. A batch does not help here either, because the split is within one
statement rather than across several. The parameter limit is enforced per statement, and
`values_chunked` is how a bulk load gets past it.

## Gotchas

- **A batch is not fewer round trips.** It is one transaction and one `await`. See above.
- **A batch is not a pipeline.** The next statement is not sent until the previous one
  resolves.
- **A select in a batch is collected, not streamed.** It decodes into one `Vec<R>` when its
  turn comes; a batch cannot hand you a cursor.
- **A transaction is not one round trip either.** It is `BEGIN`, then each statement, then
  `COMMIT`. It buys atomicity and a single connection, not fewer messages.
- **Order matters.** Statements run in tuple order, and a failing one stops the rest.
- **`RETURNING` needs a dialect that has it.** PostgreSQL and SQLite support `returning`;
  MySQL does not, so a MySQL batch reads its row back with a select instead.
- **Eight is the limit.** Longer tuples are not implemented; split the work into two batches
  or use a transaction.
