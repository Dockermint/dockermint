//! Structured logging with optional file rotation.
//!
//! Built on [`tracing`] + [`tracing_subscriber`] with env-filter support
//! and optional JSON output.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::config::types::LogConfig;
use crate::error::Error;

/// Initialize the global tracing subscriber.
///
/// Must be called **once** at startup before any `tracing` macros fire.
///
/// # Arguments
///
/// * `config` - Logging configuration from `config.toml`
///
/// # Errors
///
/// Returns [`Error`] if subscriber installation fails (e.g. called
/// twice).
///
/// # Examples
///
/// ```no_run
/// use dockermint::config::types::LogConfig;
/// dockermint::logger::init(&LogConfig::default())?;
/// # Ok::<(), dockermint::error::Error>(())
/// ```
pub fn init(config: &LogConfig) -> Result<(), Error> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    match (&config.directory, config.json) {
        // JSON to file with rotation
        (Some(dir), true) => {
            let file_appender = tracing_appender::rolling::daily(dir, &config.file_prefix);
            let layer = fmt::layer()
                .json()
                .with_writer(file_appender)
                .with_target(true)
                .with_thread_ids(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .try_init()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        },
        // Plain text to file with rotation
        (Some(dir), false) => {
            let file_appender = tracing_appender::rolling::daily(dir, &config.file_prefix);
            let layer = fmt::layer()
                .with_writer(file_appender)
                .with_target(true)
                .with_thread_ids(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .try_init()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        },
        // JSON to stdout
        (None, true) => {
            let layer = fmt::layer().json().with_target(true).with_thread_ids(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .try_init()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        },
        // Plain text to stdout (default)
        (None, false) => {
            let layer = fmt::layer().with_target(true).with_thread_ids(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(layer)
                .try_init()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        },
    }

    Ok(())
}
