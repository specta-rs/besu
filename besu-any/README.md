# besu-any

A Besu database whose backend is chosen at runtime, for when the backend comes from
configuration rather than from code.

```rust
use besu_any::AnyDatabase;

let db = AnyDatabase::connect(&std::env::var("DATABASE_URL")?).await?;
db.push_schema(&schema, false).await?;
db.migrate(&Migrator::from_dir("migrations")?).await?;
```

Typed queries stay per-backend, because the SQL a query produces depends on the dialect. The
crate documentation shows the `match` that gets you back to a concrete database and the generic
helper that keeps one function body for all three.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs)
- [API reference](https://docs.rs/besu-any)

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
