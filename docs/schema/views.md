# Views

A view is a named `SELECT` that the database stores. Besu introspects views so a snapshot
names them, but the differ ignores them: a view is created, altered and dropped by migration
SQL, never by `push` or `generate`.

```rust
let schema = db.introspect().await?;

for view in &schema.views {
    println!("{}: {}", view.name, view.definition);
}
```

`Schema::views` is a `Vec<View>`, and each `View` is a name plus the dialect's own
description of the view, verbatim:

```rust
pub struct View {
    pub name: String,
    pub definition: String,
}
```

## What `besu pull` records

`besu pull` introspects the live database and writes the whole `Schema` as JSON, views
included. Names follow the same rule as tables: a view in the connection's `current_schema()`
is unqualified, and one anywhere else is `schema.view`.

```sh
besu pull --database postgres://user:pass@host/db --out schema.json
# wrote 4 table(s) from postgres to schema.json
```

The message counts tables only, so a database of views and no base tables reports `0`. The
snapshot still contains the views.

The definition is not portable:

- PostgreSQL and MySQL report the `SELECT` body.
- SQLite reports the whole `CREATE VIEW` statement.

A view whose definition cannot be read, because the connected role cannot see it, is skipped
rather than recorded with an empty body.

## A view is migration SQL

`push` never creates, alters or drops a view. Neither does `besu generate`: the diff has no
view variant, so a difference between the database and the snapshot never produces a
statement. A view belongs in a numbered migration, where it is reviewed like any other change.

```sql
-- migrations/0003_active_users.sql
CREATE VIEW active_users AS
SELECT id, email FROM users WHERE active;
```

Write the inverse yourself if the view has to be revertable:

```sql
-- migrations/0003_active_users.down.sql
DROP VIEW active_users;
```

To change a view you have to drop and recreate it, or use `CREATE OR REPLACE VIEW`. Neither
statement can be generated, because the differ cannot compare two definitions.

## Materialised views

> **Not supported yet.** A materialised view is not introspected and has no IR type. Create it
> in a migration; refreshing it is ordinary SQL through `sql!`. See the
> [roadmap](../roadmap.md) and [Drizzle parity](../parity.md).

## What is checked

- A view is never reported as a table, so `Schema::find` will not return one and `push` will
  not try to drop it.
- `Schema::sort` orders views by name, so two equivalent snapshots serialise identically.
- The snapshot round-trips: a view recorded by `besu pull` loads back as the same `View`.

## Gotchas

- **The differ ignores views in both directions.** Adding a view to a snapshot does not
  create it, and removing one does not drop it. This is deliberate: a view the IR cannot
  describe would otherwise be dropped by a `push` that was never told about it.
- **A migration that drops a column a view uses will fail on PostgreSQL.** The statement is
  refused because the view depends on the column, and `push` is not transactional, so earlier
  statements have already run. Drop the view first, in a migration of its own.
- **Snapshots and databases can disagree about views and nothing will say so.** The JSON is a
  description, not a checked expectation.
- **The name is the only identity.** A view in another schema is stored as `schema.view`;
  there is no separate `schema` field to query, and no lookup helper for views.
