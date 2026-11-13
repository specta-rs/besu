# besu-sqlite

The SQLite dialect, DDL generation, introspection and drivers for Besu. The default driver is
`Native`, a `rusqlite` connection on a dedicated thread with no sqlx in its path. The optional
`sqlx` feature adds a `sqlx::SqlitePool` driver for code that wants a pool.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust. The query you write is the SQL that runs.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs) — the full documentation
- [API reference](https://docs.rs/besu-sqlite) — on docs.rs

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
