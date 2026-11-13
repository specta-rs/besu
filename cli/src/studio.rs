//! `besu studio`: a development-only schema and data browser.
//!
//! The router and the endpoints live in [`besu_studio`]; this module owns the process — the
//! flags, the socket and the safety notice. Keeping the split is what lets the router be
//! tested without binding a port.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::database::AnyDatabase;

/// Serve the studio for a database.
#[derive(clap::Args)]
pub struct Args {
    /// The database to browse, for example `postgres://user:pass@host/db`.
    #[arg(long, env = "DATABASE_URL")]
    pub database: String,
    /// The port to listen on.
    #[arg(long, default_value_t = 4983)]
    pub port: u16,
    /// Bind every interface instead of loopback.
    ///
    /// The studio has no authentication and its query box runs arbitrary SQL, so this makes
    /// it reachable by anyone who can reach the machine.
    #[arg(long)]
    pub allow_remote: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    // A server is only useful with its logs, and the router's `TraceLayer` emits spans for
    // every request. The other subcommands are one-shot and do not install a subscriber.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("besu_studio=info")),
        )
        .init();

    let db = AnyDatabase::connect(&args.database)
        .await
        .map_err(|err| anyhow::anyhow!("could not connect to `{}`: {err}", args.database))?;

    let host = if args.allow_remote {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };
    let address = SocketAddr::new(host, args.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|err| anyhow::anyhow!("could not bind {address}: {err}"))?;

    println!(
        "besu studio {} listening on http://{address}",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("This is a development tool. The query box runs arbitrary SQL against the");
    println!("configured database, and the server has no authentication.");

    if args.allow_remote {
        println!();
        println!(
            "WARNING: --allow-remote binds every interface. Anyone who can reach port {} \
             can read and modify the database.",
            args.port
        );
    } else {
        println!("It is bound to loopback only; pass --allow-remote to expose it.");
        println!();
        println!("https://studio.besu.specta.dev can connect to it, if you would rather use");
        println!("that copy of the frontend than the one served here.");
    }

    besu_studio::serve(listener, db, args.database)
        .await
        .map_err(|err| anyhow::anyhow!("the studio stopped unexpectedly: {err}"))
}

#[cfg(test)]
mod tests {
    // `panic!` in a test reports a parse that produced the wrong variant; it is not a
    // production path.
    #![allow(clippy::panic)]

    use clap::Parser;

    #[test]
    fn the_studio_is_a_subcommand_with_its_own_flags() {
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: crate::Command,
        }

        let cli = Cli::try_parse_from([
            "besu",
            "studio",
            "--database",
            "sqlite://app.db",
            "--port",
            "5000",
            "--allow-remote",
        ])
        .expect("`besu studio` must parse");
        match cli.command {
            crate::Command::Studio(args) => {
                assert_eq!(args.database, "sqlite://app.db");
                assert_eq!(args.port, 5000);
                assert!(args.allow_remote);
            }
            _ => panic!("parsed the wrong subcommand"),
        }
    }
}
