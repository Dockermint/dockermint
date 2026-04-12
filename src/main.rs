//! Dockermint entry point.
//!
//! Parses CLI arguments, loads configuration, initializes logging, and
//! dispatches to the appropriate mode handler.

use clap::Parser;

use dockermint::cli::Cli;
use dockermint::cli::commands::Commands;
use dockermint::config;
use dockermint::error::Error;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    // Load configuration
    let mut cfg = match &cli.config {
        Some(path) => config::load(path)?,
        None => config::load_default()?,
    };

    // CLI log level override
    if let Some(level) = &cli.log_level {
        cfg.log.level = level.clone();
    }

    // Initialize logging
    dockermint::logger::init(&cfg.log)?;

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "dockermint starting");

    // Apply CLI overrides for daemon mode before dispatch
    if let Commands::Daemon(ref args) = cli.command {
        config::apply_daemon_overrides(
            &mut cfg,
            args.poll_interval,
            args.max_builds,
            args.rpc,
            args.rpc_bind,
        );
    }

    // Dispatch to mode handler
    match cli.command {
        Commands::Build(args) => dockermint::run_build(cfg, args).await,
        Commands::Daemon(args) => dockermint::run_daemon(cfg, args).await,
    }
}
