use std::path::PathBuf;

/// Read the database's schema into a JSON snapshot.
#[derive(clap::Args)]
pub struct Args {
    /// The database to read, for example `postgres://user:pass@host/db`.
    #[arg(long, env = "DATABASE_URL")]
    pub database: String,
    /// Where to write the snapshot. Defaults to standard output.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let db = super::database::AnyDatabase::connect(&args.database).await?;
    let schema = db.introspect().await?;

    let json = serde_json::to_string_pretty(&schema)?;
    match args.out {
        Some(path) => {
            std::fs::write(&path, json)?;
            println!(
                "wrote {} table(s) from {} to {}",
                schema.tables.len(),
                db.name(),
                path.display()
            );
        }
        None => println!("{json}"),
    }

    Ok(())
}
