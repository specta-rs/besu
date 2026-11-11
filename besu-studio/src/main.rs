//! Command line entry point for `besu-studio`.
//!
//! The process owns nothing but a connection pool and a socket. Everything worth testing
//! lives in [`besu_studio::router`](besu_studio); this file parses arguments, prints the
//! safety notice and serves the router.
#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use besu_any::AnyDatabase;
use clap::Parser;
use tracing_subscriber::EnvFilter;

/// A development-only schema and data browser for Besu databases.
#[derive(Debug, Parser)]
#[command(name = "besu-studio", version, about)]
struct Args {
    /// Connection string for the database to browse, such as `postgres://…` or
    /// `sqlite://app.db`.
    #[arg(long)]
    database: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 4983)]
    port: u16,

    /// Bind every network interface instead of loopback.
    ///
    /// This makes the studio reachable by anyone who can reach the machine, and the studio
    /// has no authentication.
    #[arg(long)]
    allow_remote: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("besu_studio=info")),
        )
        .init();

    let args = Args::parse();
    let db = AnyDatabase::connect(&args.database)
        .await
        .expect("could not connect to the database; check --database");

    let host = if args.allow_remote {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };
    let address = SocketAddr::new(host, args.port);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("could not bind the studio's port; check --port");

    println!(
        "besu-studio {} listening on http://{address}",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("This is a development tool. The query box runs arbitrary SQL against the");
    println!("configured database, and the server has no authentication.");

    if args.allow_remote {
        println!();
        println!(
            "WARNING: --allow-remote binds every interface. Anyone who can reach port {} can \
             read and modify the database.",
            args.port
        );
    } else {
        println!("It is bound to loopback only; pass --allow-remote to expose it.");
    }

    axum::serve(listener, besu_studio::router(db))
        .await
        .expect("the studio's server stopped unexpectedly");
}
