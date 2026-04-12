//! Configuration types loaded from `config.toml` and environment variables.
//!
//! The [`Config`] struct is the top-level configuration for Dockermint.
//! It controls logging, daemon behavior, RPC settings, and per-recipe
//! flavor overrides.  Secrets (API keys, tokens) are never stored here;
//! they live in `.env` and are loaded via [`dotenvy`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use secrecy::SecretString;
use serde::Deserialize;

use crate::recipe::types::FlavorValue;

// ===========================================================================
// Top-level config
// ===========================================================================

/// Top-level Dockermint configuration deserialized from `config.toml`.
///
/// # Examples
///
/// ```no_run
/// let raw = std::fs::read_to_string("config.toml")?;
/// let cfg: dockermint::config::types::Config = toml::from_str(&raw)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Configuration file schema version.
    #[serde(default = "default_version")]
    pub version: u32,

    /// Directory containing recipe TOML files.
    #[serde(default = "default_recipes_dir")]
    pub recipes_dir: PathBuf,

    /// Logging configuration.
    #[serde(default)]
    pub log: LogConfig,

    /// Daemon-mode settings (polling interval, etc.).
    #[serde(default)]
    pub daemon: Option<DaemonConfig>,

    /// RPC-mode settings (bind address, etc.).
    #[serde(default)]
    pub rpc: Option<RpcConfig>,

    /// Per-recipe flavor overrides.
    ///
    /// Keys are recipe file stems (e.g. `"cosmos-gaiad"`).  Values map
    /// flavor dimensions to the desired selection, overriding recipe
    /// defaults.
    #[serde(default)]
    pub flavours: HashMap<String, RecipeFlavourOverride>,

    /// Docker engine connection and builder settings.
    #[serde(default)]
    pub docker: DockerConfig,

    /// Database backend configuration.
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Notification backend configuration.
    #[serde(default)]
    pub notifier: NotifierConfig,

    /// VCS backend configuration.
    #[serde(default)]
    pub vcs: VcsConfig,

    /// Registry backend configuration.
    #[serde(default)]
    pub registry: RegistryConfig,

    /// Metrics server configuration.
    #[serde(default)]
    pub metrics: MetricsConfig,
}

fn default_version() -> u32 {
    1
}

fn default_recipes_dir() -> PathBuf {
    PathBuf::from("recipes")
}

// ===========================================================================
// Sub-configs
// ===========================================================================

/// Logging configuration.
#[derive(Debug, Deserialize)]
pub struct LogConfig {
    /// Minimum log level (e.g. `"info"`, `"debug"`).
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Directory for rotated log files.  `None` means stdout only.
    #[serde(default)]
    pub directory: Option<PathBuf>,

    /// Log file name prefix.
    #[serde(default = "default_log_prefix")]
    pub file_prefix: String,

    /// Whether to emit logs in JSON format.
    #[serde(default)]
    pub json: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            directory: None,
            file_prefix: default_log_prefix(),
            json: false,
        }
    }
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_log_prefix() -> String {
    "dockermint".to_owned()
}

/// Daemon-mode configuration.
#[derive(Debug, Deserialize)]
pub struct DaemonConfig {
    /// Seconds between VCS polling cycles.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Maximum number of tags to build per polling cycle per recipe.
    #[serde(default = "default_max_builds_per_cycle")]
    pub max_builds_per_cycle: u32,
}

fn default_poll_interval() -> u64 {
    60
}

fn default_max_builds_per_cycle() -> u32 {
    1
}

/// RPC-mode configuration.
#[derive(Debug, Deserialize)]
pub struct RpcConfig {
    /// Socket address for the RPC server to bind to.
    #[serde(default = "default_rpc_bind")]
    pub bind: SocketAddr,
}

fn default_rpc_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9100))
}

/// Docker engine and buildx configuration.
#[derive(Debug, Deserialize)]
pub struct DockerConfig {
    /// Docker daemon socket URI.
    ///
    /// Examples: `unix:///var/run/docker.sock`, `tcp://192.168.1.10:2376`
    #[serde(default = "default_docker_socket")]
    pub socket_uri: String,

    /// Prefix for buildx builder instance names.
    ///
    /// Builders are named `{prefix}-amd64`, `{prefix}-arm64`.
    #[serde(default = "default_builder_prefix")]
    pub builder_prefix: String,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket_uri: default_docker_socket(),
            builder_prefix: default_builder_prefix(),
        }
    }
}

fn default_docker_socket() -> String {
    "unix:///var/run/docker.sock".to_owned()
}

fn default_builder_prefix() -> String {
    "dockermint".to_owned()
}

/// Per-recipe flavor overrides from `config.toml`.
///
/// Maps flavor dimension names to their desired value(s), overriding
/// the recipe's `[flavours.default]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct RecipeFlavourOverride(pub HashMap<String, FlavorValue>);

// ---------------------------------------------------------------------------
// Module-backend configs
// ---------------------------------------------------------------------------

/// Database backend configuration.
#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    /// Path to the database file/directory.
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

fn default_db_path() -> PathBuf {
    PathBuf::from("data/dockermint.redb")
}

/// Notifier backend configuration.
#[derive(Debug, Default, Deserialize)]
pub struct NotifierConfig {
    /// Whether notifications are enabled.
    #[serde(default)]
    pub enabled: bool,
}

/// VCS backend configuration.
#[derive(Debug, Deserialize)]
pub struct VcsConfig {
    /// Maximum concurrent API requests.
    #[serde(default = "default_vcs_concurrency")]
    pub max_concurrent_requests: u32,
}

impl Default for VcsConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: default_vcs_concurrency(),
        }
    }
}

fn default_vcs_concurrency() -> u32 {
    4
}

/// Registry backend configuration.
#[derive(Debug, Default, Deserialize)]
pub struct RegistryConfig {
    /// Registry URL override (uses Docker Hub when absent).
    #[serde(default)]
    pub url: Option<String>,
}

/// Metrics server configuration.
#[derive(Debug, Deserialize)]
pub struct MetricsConfig {
    /// Whether the metrics endpoint is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Socket address for the metrics server.
    #[serde(default = "default_metrics_bind")]
    pub bind: SocketAddr,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_metrics_bind(),
        }
    }
}

fn default_metrics_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9200))
}

// ===========================================================================
// Secrets (loaded from .env, never from config.toml)
// ===========================================================================

/// Secrets loaded from environment variables via [`dotenvy`].
///
/// Fields use [`SecretString`] so values are never accidentally logged
/// or serialized.
#[derive(Debug)]
pub struct Secrets {
    /// GitHub username for API authentication.
    pub gh_user: Option<SecretString>,

    /// GitHub personal access token.
    pub gh_pat: Option<SecretString>,

    /// Telegram bot token.
    pub telegram_token: Option<SecretString>,

    /// Telegram chat ID for notifications.
    pub telegram_chat_id: Option<String>,

    /// Container registry username.
    pub registry_user: Option<SecretString>,

    /// Container registry password/token.
    pub registry_password: Option<SecretString>,
}

/// Operating mode derived from the CLI subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One-shot build, then exit.
    Cli,
    /// Continuous polling for new releases (optionally with RPC).
    Daemon,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_deserializes() {
        let raw = r#"
            version = 1
            recipes_dir = "recipes"
        "#;
        let cfg: Config = toml::from_str(raw).expect("parse");
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.recipes_dir, PathBuf::from("recipes"));
    }

    #[test]
    fn flavour_override_deserializes() {
        let raw = r#"
            db_backend = "pebbledb"
            build_tags = ["netgo", "muslc", "ledger"]
        "#;
        let ov: RecipeFlavourOverride = toml::from_str(raw).expect("parse");
        assert!(ov.0.contains_key("db_backend"));
        assert!(ov.0.contains_key("build_tags"));
    }
}
