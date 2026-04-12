//! Clap-based CLI definition with subcommands.

pub mod commands;

use std::path::PathBuf;

use clap::Parser;

use crate::cli::commands::Commands;

/// Dockermint -- CI/CD pipeline for Cosmos SDK blockchains.
///
/// Automates Docker image creation for blockchain nodes and sidecars
/// with multi-architecture support.
#[derive(Debug, Parser)]
#[command(
    name = "dockermint",
    version,
    about = "The first CI/CD Pipeline for Cosmos SDK.",
    long_about = None,
)]
pub struct Cli {
    /// Path to `config.toml`.
    #[arg(short, long, env = "DOCKERMINT_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    /// Log level override (trace, debug, info, warn, error).
    #[arg(short, long, global = true)]
    pub log_level: Option<String>,

    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Commands,
}
