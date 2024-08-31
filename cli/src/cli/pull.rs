use tracing::warn;

#[derive(clap::Args)]
#[command(about = "Generate the Rust schema from an existing remote database.")]
pub struct Command {}

impl Command {
    pub fn run(&self) {
        warn!("Coming soon...");
    }
}
