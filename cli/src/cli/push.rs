use tracing::warn;

#[derive(clap::Args)]
#[command(about = "Push the Rust schema to a remote database.")]
pub struct Command {}

impl Command {
    pub fn run(&self) {
        warn!("Coming soon...");
    }
}
