use tracing::warn;

#[derive(clap::Args)]
#[command(about = "Generate the Rust code, instead of using the besu::Table macro.")]
pub struct Command {}

impl Command {
    pub fn run(&self) {
        warn!("Coming soon...");
    }
}
