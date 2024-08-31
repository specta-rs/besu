use clap::{Parser, Subcommand};

mod generate;
mod pull;
mod push;
mod studio;

#[derive(Parser)]
#[command(name = env!("CARGO_PKG_NAME"))]
#[command(bin_name = env!("CARGO_PKG_NAME"))]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " - ", env!("GIT_HASH")))]
#[command(disable_version_flag = true)]
#[command(about = env!("CARGO_PKG_DESCRIPTION"))]
pub struct Cli {
    // We wanna support `-v` and that's not a Clap default
    #[arg(short = 'v', short_alias = 'V', long, action = clap::builder::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Generate(generate::Command),
    Pull(pull::Command),
    Push(push::Command),
    Studio(studio::Command),
}
