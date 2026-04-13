//! `dockermint daemon` subcommand.

use std::net::SocketAddr;

use clap::Args;

/// Arguments for the `daemon` subcommand.
///
/// The daemon continuously polls VCS providers for new releases and
/// builds Docker images.  Pass `--rpc` to also start an HTTP server
/// that accepts build requests.
#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// Override the polling interval (seconds).
    #[arg(short = 'i', long)]
    pub poll_interval: Option<u64>,

    /// Maximum number of tags to build per polling cycle per recipe.
    #[arg(short, long)]
    pub max_builds: Option<u32>,

    /// Specific recipes to watch (file stems).  If empty, watches all.
    #[arg(short, long)]
    pub recipes: Vec<String>,

    /// Enable the RPC server alongside the daemon.
    ///
    /// When set, the daemon also binds an HTTP endpoint that accepts
    /// build requests, status queries, and health checks.
    #[arg(long)]
    pub rpc: bool,

    /// Socket address for the RPC server (requires `--rpc`).
    #[arg(long, default_value = "127.0.0.1:9100", requires = "rpc")]
    pub rpc_bind: SocketAddr,
}
