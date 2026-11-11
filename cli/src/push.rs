use std::path::PathBuf;

use besu::schema::Safety;

use super::database::CliDatabase;

/// Apply a schema snapshot to the database.
#[derive(clap::Args)]
pub struct Args {
    /// The database to change, for example `postgres://user:pass@host/db`.
    #[arg(long, env = "DATABASE_URL")]
    pub database: String,
    /// The schema snapshot to apply.
    #[arg(long)]
    pub schema: PathBuf,
    /// Also drop columns and tables that the snapshot no longer declares.
    #[arg(long)]
    pub destructive: bool,
    /// Print the planned changes, with their safety, instead of applying them.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let desired = besu::schema::Schema::load(&args.schema)?;
    let db = super::database::AnyDatabase::connect(&args.database).await?;

    if args.dry_run {
        let current = db.introspect().await?;
        let changes = CliDatabase::diff(&db, &current, &desired);
        if changes.is_empty() {
            println!("schema is up to date");
            return Ok(());
        }
        for change in &changes {
            let safety = match change.safety() {
                Safety::Safe => "safe",
                Safety::Risky => "risky",
                Safety::Destructive => "DESTRUCTIVE",
            };
            println!("[{safety}] {change}");
        }
        return Ok(());
    }

    let result = db.push_schema(&desired, args.destructive).await?;
    if result.is_empty() {
        println!("schema is up to date");
    } else {
        for statement in &result.statements {
            println!("{statement};");
        }
        println!(
            "applied {} statement(s) to {}",
            result.statements.len(),
            db.backend()
        );
    }

    Ok(())
}
