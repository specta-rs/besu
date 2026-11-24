# besu-sqlite

The SQLite dialect, DDL generation, introspection and drivers for Besu. The default driver is
`Native`, a `rusqlite` connection on a dedicated thread with no sqlx in its path. The optional
`sqlx` feature adds a `sqlx::SqlitePool` driver for code that wants a pool.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust. The query you write is the SQL that runs.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs) — the full documentation
- [API reference](https://docs.rs/besu-sqlite) — on docs.rs

## Schema rebuilds

SQLite column and constraint changes rebuild the affected table inside one transaction. The
rebuild preserves `STRICT`, `WITHOUT ROWID`, partial indexes, index collation and sort order,
and uses a unique scratch name without deleting a pre-existing object. Compatible views stay
usable; a view broken by the change rolls the rebuild back and is named in the error.

Before committing, Besu runs `PRAGMA foreign_key_check` for the rebuilt table, even when the
connection started with foreign-key enforcement disabled. This validates the table's outgoing
foreign keys. SQLite's table-scoped check does not detect a parent key change orphaning
incoming child rows, so callers changing referenced keys should run a full
`PRAGMA foreign_key_check` as part of their own migration verification.

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
