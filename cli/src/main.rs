use clap::Parser;

mod cli;

fn main() {
    // TODO: Determine the local project's version and ensure we are invoking the correct besu-cli.

    let cli = cli::Cli::parse();

    tracing_subscriber::fmt().init();
    std::panic::set_hook(Box::new(move |panic| tracing::error!("{panic}")));

    match cli.command {
        cli::Commands::Generate(cmd) => cmd.run(),
        cli::Commands::Pull(cmd) => cmd.run(),
        cli::Commands::Push(cmd) => cmd.run(),
        cli::Commands::Studio(cmd) => cmd.run(),
    }
}
