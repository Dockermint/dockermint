//! Configuration loading, validation, and CLI override merging.
//!
//! Configuration sources (highest priority first):
//! 1. CLI arguments (`--log-level`, `--poll-interval`, etc.)
//! 2. `config.toml` file
//! 3. Built-in defaults
//!
//! Secrets live in `.env` and are loaded separately via
//! [`load_secrets()`](crate::config::load_secrets).

pub mod types;

use std::path::Path;

use secrecy::SecretString;

use crate::config::types::{Config, DaemonConfig, RpcConfig, Secrets};
use crate::error::ConfigError;

/// Supported configuration file schema version.
const SUPPORTED_VERSION: u32 = 1;

/// Load configuration from a TOML file.
///
/// # Arguments
///
/// * `path` - Path to `config.toml`
///
/// # Returns
///
/// Parsed and validated [`Config`].
///
/// # Errors
///
/// - [`ConfigError::ReadFile`] if the file cannot be read.
/// - [`ConfigError::Parse`] if deserialization fails.
/// - [`ConfigError::Invalid`] if validation fails.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|e| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let config: Config = toml::from_str(&contents)?;
    validate(&config)?;
    Ok(config)
}

/// Load a default configuration when no config file is provided.
///
/// # Returns
///
/// A [`Config`] with all default values.
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if the defaults are somehow invalid
/// (should not happen).
pub fn load_default() -> Result<Config, ConfigError> {
    let config: Config = toml::from_str("")?;
    Ok(config)
}

/// Validate a loaded configuration.
///
/// # Arguments
///
/// * `config` - The configuration to validate
///
/// # Errors
///
/// Returns [`ConfigError::Invalid`] on semantic violations.
pub fn validate(config: &Config) -> Result<(), ConfigError> {
    if config.version != SUPPORTED_VERSION {
        return Err(ConfigError::Invalid(format!(
            "unsupported config version {}, expected {SUPPORTED_VERSION}",
            config.version
        )));
    }

    if !config.recipes_dir.exists() {
        return Err(ConfigError::Invalid(format!(
            "recipes directory does not exist: {}",
            config.recipes_dir.display()
        )));
    }

    if let Some(daemon) = &config.daemon {
        if daemon.poll_interval_secs == 0 {
            return Err(ConfigError::Invalid(
                "daemon.poll_interval_secs must be > 0".to_owned(),
            ));
        }
        if daemon.max_builds_per_cycle == 0 {
            return Err(ConfigError::Invalid(
                "daemon.max_builds_per_cycle must be > 0".to_owned(),
            ));
        }
    }

    Ok(())
}

/// Apply CLI daemon arguments as overrides on top of the loaded config.
///
/// Values present in `args` take precedence over `config.toml`.
///
/// # Arguments
///
/// * `config` - Mutable reference to the loaded config
/// * `poll_interval` - CLI `--poll-interval` override
/// * `max_builds` - CLI `--max-builds` override
/// * `rpc` - Whether `--rpc` was passed
/// * `rpc_bind` - CLI `--rpc-bind` override
pub fn apply_daemon_overrides(
    config: &mut Config,
    poll_interval: Option<u64>,
    max_builds: Option<u32>,
    rpc: bool,
    rpc_bind: std::net::SocketAddr,
) {
    let daemon = config.daemon.get_or_insert(DaemonConfig {
        poll_interval_secs: 60,
        max_builds_per_cycle: 1,
    });

    if let Some(interval) = poll_interval {
        daemon.poll_interval_secs = interval;
    }
    if let Some(max) = max_builds {
        daemon.max_builds_per_cycle = max;
    }

    if rpc {
        let rpc_cfg = config.rpc.get_or_insert(RpcConfig { bind: rpc_bind });
        rpc_cfg.bind = rpc_bind;
    }
}

/// Load secrets from environment variables.
///
/// Reads `.env` if present (via [`dotenvy`]), then extracts known
/// secret variables.  Missing variables become `None` rather than
/// errors.
///
/// # Returns
///
/// Populated [`Secrets`] struct.
pub fn load_secrets() -> Secrets {
    // Load .env file if present (ignore errors)
    let _ = dotenvy::dotenv();

    Secrets {
        gh_user: env_secret("GH_USER"),
        gh_pat: env_secret("GH_PAT"),
        telegram_token: env_secret("TELEGRAM_TOKEN"),
        telegram_chat_id: env_secret("TELEGRAM_CHAT_ID"),
        registry_user: env_secret("REGISTRY_USER"),
        registry_password: env_secret("REGISTRY_PASSWORD"),
    }
}

/// Read an environment variable into a [`SecretString`].
fn env_secret(key: &str) -> Option<SecretString> {
    std::env::var(key).ok().map(SecretString::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn load_default_succeeds() {
        let config = load_default().expect("default should parse");
        assert_eq!(config.version, 1);
        assert_eq!(config.recipes_dir, PathBuf::from("recipes"));
    }

    #[test]
    fn load_project_config_toml() {
        let path = Path::new("config.toml");
        if path.exists() {
            let config = load(path).expect("should parse config.toml");
            assert_eq!(config.version, 1);
            assert!(config.daemon.is_some());
            assert!(config.rpc.is_some());
        }
    }

    #[test]
    fn validate_rejects_zero_poll_interval() {
        let raw = r#"
            version = 1
            [daemon]
            poll_interval_secs = 0
        "#;
        let config: Config = toml::from_str(raw).expect("parse");
        let err = validate(&config).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_nonexistent_relative_recipes_dir() {
        let raw = r#"
            version = 1
            recipes_dir = "nonexistent_dir_abc123"
        "#;
        let config: Config = toml::from_str(raw).expect("parse");
        let err = validate(&config).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err:?}"
        );
    }

    #[test]
    fn validate_rejects_bad_version() {
        let raw = r#"version = 99"#;
        let config: Config = toml::from_str(raw).expect("parse");
        let err = validate(&config).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err:?}"
        );
    }

    #[test]
    fn apply_daemon_overrides_creates_daemon_section() {
        let mut config = load_default().expect("default");
        assert!(config.daemon.is_none());

        apply_daemon_overrides(
            &mut config,
            Some(120),
            Some(5),
            false,
            "127.0.0.1:9100".parse().expect("addr"),
        );

        let daemon = config.daemon.expect("should exist now");
        assert_eq!(daemon.poll_interval_secs, 120);
        assert_eq!(daemon.max_builds_per_cycle, 5);
    }

    #[test]
    fn apply_daemon_overrides_enables_rpc() {
        let mut config = load_default().expect("default");
        assert!(config.rpc.is_none());

        let addr = "0.0.0.0:8080".parse().expect("addr");
        apply_daemon_overrides(&mut config, None, None, true, addr);

        let rpc = config.rpc.expect("should exist now");
        assert_eq!(rpc.bind, addr);
    }

    #[test]
    fn full_config_roundtrip() {
        let raw = r#"
            version = 1
            recipes_dir = "recipes"

            [log]
            level = "debug"
            json = true

            [daemon]
            poll_interval_secs = 30
            max_builds_per_cycle = 3

            [rpc]
            bind = "0.0.0.0:9100"

            [flavours.cosmos-gaiad]
            db_backend = "pebbledb"

            [database]
            path = "data/test.redb"

            [notifier]
            enabled = true

            [vcs]
            max_concurrent_requests = 8

            [registry]
            url = "ghcr.io"

            [metrics]
            enabled = true
            bind = "0.0.0.0:9200"
        "#;

        let config: Config = toml::from_str(raw).expect("parse");
        validate(&config).expect("valid");

        assert_eq!(config.log.level, "debug");
        assert!(config.log.json);
        assert_eq!(
            config.daemon.as_ref().expect("daemon").poll_interval_secs,
            30
        );
        assert!(config.flavours.contains_key("cosmos-gaiad"));
        assert!(config.notifier.enabled);
        assert_eq!(config.vcs.max_concurrent_requests, 8);
        assert_eq!(config.registry.url.as_deref(), Some("ghcr.io"));
        assert!(config.metrics.enabled);
    }
}
