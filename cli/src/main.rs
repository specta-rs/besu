//! Command line tooling for [Besu](https://docs.rs/besu).
//!
//! The CLI works from a *schema snapshot*: a JSON file describing the tables your Rust code
//! declares. Emit one from your application with
//! [`Database::schema_snapshot`](https://docs.rs/besu/latest/besu/struct.Database.html#method.schema_snapshot),
//! then use these commands to apply it, diff it or turn it into migration files.
//!
//! ```text
//! besu pull     --database postgres://… --out schema.json
//! besu push     --database postgres://… --schema schema.json
//! besu generate --database postgres://… --schema schema.json --out migrations
//! besu migrate  --database postgres://… --dir migrations
//! ```

use clap::{Parser, Subcommand};

mod database;
mod generate;
mod migrate;
mod pull;
mod push;

#[derive(Parser)]
#[command(name = "besu")]
#[command(version, about = "Schema tooling for Besu")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the database's schema into a JSON snapshot.
    Pull(pull::Args),
    /// Apply a schema snapshot to the database.
    Push(push::Args),
    /// Turn the difference between a snapshot and the database into migration files.
    Generate(generate::Args),
    /// Apply migration files.
    Migrate(migrate::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Pull(args) => pull::run(args).await,
        Command::Push(args) => push::run(args).await,
        Command::Generate(args) => generate::run(args).await,
        Command::Migrate(args) => migrate::run(args).await,
    }
}
