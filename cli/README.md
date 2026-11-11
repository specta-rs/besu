# besu-cli

Schema tooling for [Besu](https://github.com/specta-rs/besu): read a database's schema into a
snapshot, apply a snapshot, turn the difference into migration files, and apply migrations.

```sh
besu pull     --database postgres://user:pass@host/db --out schema.json
besu push     --database postgres://user:pass@host/db --schema schema.json
besu generate --database postgres://user:pass@host/db --schema schema.json --out migrations
besu migrate  --database postgres://user:pass@host/db --dir migrations
```

The CLI works from a *schema snapshot*, a JSON file describing the tables your Rust code
declares. Emit one from your application with `Database::schema_snapshot`.

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
