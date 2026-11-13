# besu-sqlite

The SQLite dialect, DDL generation, introspection and drivers for Besu. The default driver is
sqlx; the additive `native` feature swaps in a `rusqlite` driver on a dedicated connection
thread, with no sqlx in its path.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust. The query you write is the SQL that runs.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs) — the full documentation
- [API reference](https://docs.rs/besu-sqlite) — on docs.rs

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
