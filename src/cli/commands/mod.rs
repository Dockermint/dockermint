//! CLI subcommand definitions.

pub mod build;
pub mod daemon;

use clap::Subcommand;

use crate::cli::commands::build::BuildArgs;
use crate::cli::commands::daemon::DaemonArgs;

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Build a Docker image from a recipe (one-shot).
    Build(BuildArgs),

    /// Start the daemon: continuously poll for new releases and build.
    ///
    /// Pass `--rpc` to also start an HTTP server that accepts build
    /// requests alongside the polling loop.
    Daemon(DaemonArgs),
}
