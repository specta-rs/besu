use std::path::PathBuf;

/// Apply migration files.
#[derive(clap::Args)]
pub struct Args {
    /// The database to migrate, for example `postgres://user:pass@host/db`.
    #[arg(long, env = "DATABASE_URL")]
    pub database: String,
    /// The directory holding `NNNN_name.sql` files.
    #[arg(long, default_value = "migrations")]
    pub dir: PathBuf,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let migrator = besu::schema::Migrator::from_dir(&args.dir)?;
    if migrator.is_empty() {
        println!("no migrations found in {}", args.dir.display());
        return Ok(());
    }

    let db = super::database::AnyDatabase::connect(&args.database).await?;
    let applied = db.migrate(&migrator).await?;

    if applied.is_empty() {
        println!("database is up to date");
    } else {
        for migration in &applied {
            println!("applied {:04} {}", migration.version, migration.name);
        }
        println!("applied {} migration(s) to {}", applied.len(), db.name());
    }

    Ok(())
}
