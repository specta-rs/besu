# SQL comments

> **Not supported yet.**

There is no `comment` on a query builder, so a statement cannot carry an application-level label
the way Drizzle's `.comment("...")` can. Tracking issue:
[#87](https://github.com/specta-rs/besu/issues/87), also on the
[roadmap](../roadmap.md#smaller-items), and listed on [Drizzle parity](../parity.md).

## Putting a comment in a statement

Use [`sql!`](sql.md) and write the comment as part of the text:

```rust
db.execute(sql!(
    r#"-- nightly reconciliation
       UPDATE "users" SET "age" = "age" + 1 WHERE "id" = ?"#,
    1
))
.await?;
```

A `--` comment ends at the newline, so keep the line break. A `/* ... */` comment can sit
anywhere, but it may not contain a `?` — the macro counts that as a placeholder and the arity
check fails. A comment is only possible in one hand-written statement; the typed builders have
nowhere to put one.

## Seeing the statement instead

- `Select::inspect()` and `Sql::inspect()` inline the parameters for a log line; see
  [query utilities](query-utils.md).
- `besu::logged` wraps a driver and logs every statement at `DEBUG` on the `besu::query` target,
  with its duration, plus a `WARN` for anything slower than the `Policy` threshold.

## Gotchas

- **No builder method adds a comment.** `sql!` or nothing.
- **A `?` in a comment is a placeholder** to `sql!`, and a `--` comment without a trailing
  newline comments out the rest of the statement.
