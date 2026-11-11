use std::path::PathBuf;

use besu::schema::{AppliedMigration, Migrator};

/// Apply migration files, or revert them.
#[derive(clap::Args)]
pub struct Args {
    /// The database to migrate, for example `postgres://user:pass@host/db`.
    #[arg(long, env = "DATABASE_URL")]
    pub database: String,
    /// The directory holding `NNNN_name.sql` and `NNNN_name.down.sql` files.
    #[arg(long, default_value = "migrations")]
    pub dir: PathBuf,
    /// Revert migrations instead of applying them. On its own, reverts the newest one.
    #[arg(long)]
    pub down: bool,
    /// The version to stop at when reverting; `0` reverts everything. Requires `--down`.
    #[arg(long, value_name = "VERSION", requires = "down")]
    pub to: Option<i64>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let migrator = Migrator::from_dir(&args.dir)?;
    if migrator.is_empty() {
        println!("no migrations found in {}", args.dir.display());
        return Ok(());
    }

    let db = super::database::AnyDatabase::connect(&args.database).await?;

    if args.down {
        let reverted = match args.to {
            Some(to) => migrate_down(&db, &migrator, to).await?,
            None => migrate_down_one(&db, &migrator)
                .await?
                .into_iter()
                .collect(),
        };

        if reverted.is_empty() {
            println!("nothing to revert");
        } else {
            for migration in &reverted {
                println!("reverted {:04} {}", migration.version, migration.name);
            }
            println!(
                "reverted {} migration(s) from {}",
                reverted.len(),
                db.backend()
            );
        }
        return Ok(());
    }

    let applied = db.migrate(&migrator).await?;

    if applied.is_empty() {
        println!("database is up to date");
    } else {
        for migration in &applied {
            println!("applied {:04} {}", migration.version, migration.name);
        }
        println!("applied {} migration(s) to {}", applied.len(), db.backend());
    }

    Ok(())
}

/// Revert down to `to`, dispatching over the backend the same way `AnyDatabase` does.
///
/// The CLI is the only place where the driver is chosen at runtime, so the down path needs the
/// same match as the up path. Both return `SchemaError<sqlx::Error>`, which is why the arms
/// unify.
async fn migrate_down(
    db: &super::database::AnyDatabase,
    migrator: &Migrator,
    to: i64,
) -> anyhow::Result<Vec<AppliedMigration>> {
    let result = match db {
        super::database::AnyDatabase::Sqlite(db) => db.migrate_down(migrator, to).await,
        super::database::AnyDatabase::Postgres(db) => db.migrate_down(migrator, to).await,
        super::database::AnyDatabase::MySql(db) => db.migrate_down(migrator, to).await,
    };
    result.map_err(|err| anyhow::anyhow!("{err}"))
}

/// Revert the newest migration, if there is one.
async fn migrate_down_one(
    db: &super::database::AnyDatabase,
    migrator: &Migrator,
) -> anyhow::Result<Option<AppliedMigration>> {
    let result = match db {
        super::database::AnyDatabase::Sqlite(db) => db.migrate_down_one(migrator).await,
        super::database::AnyDatabase::Postgres(db) => db.migrate_down_one(migrator).await,
        super::database::AnyDatabase::MySql(db) => db.migrate_down_one(migrator).await,
    };
    result.map_err(|err| anyhow::anyhow!("{err}"))
}
