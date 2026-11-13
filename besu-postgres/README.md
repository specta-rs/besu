# besu-postgres

The PostgreSQL dialect and drivers, DDL generation and introspection for Besu. The default
driver is `Native`, a `tokio-postgres` pool with no sqlx in its path. The optional `sqlx`
feature adds a `sqlx::PgPool` driver for code that wants sqlx's pool and TLS story.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust. The query you write is the SQL that runs.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs) — the full documentation
- [API reference](https://docs.rs/besu-postgres) — on docs.rs

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
