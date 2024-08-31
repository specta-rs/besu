use tracing::warn;

#[derive(clap::Args)]
#[command(about = "A database viewer.")]
pub struct Command {}

impl Command {
    pub fn run(&self) {
        warn!("Coming soon...");
    }
}
