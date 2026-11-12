# besu-studio

The server behind `besu studio`: a development-only schema and data browser for Besu
databases. Browse a table, page through its rows, and run a statement from a single page.
Drizzle Studio is the model, without the writes-oriented features.

This crate is a library — the command lives in the CLI, so there is nothing extra to install:

```console
$ besu studio --database postgres://user:password@localhost/app
besu studio 0.1.0 listening on http://127.0.0.1:4983
```

`--allow-remote` binds every interface; it is off by default because the studio runs arbitrary
SQL and has no authentication. Use it against a development database, never production.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs)
- [API reference](https://docs.rs/besu-studio)

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
