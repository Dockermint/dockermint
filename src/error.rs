//! Central error types for every Dockermint module.
//!
//! Each module owns a dedicated error enum derived with [`thiserror`].
//! The top-level [`Error`] aggregates them via `#[from]` conversions so
//! callers can propagate any module error with `?`.

use std::path::PathBuf;

use thiserror::Error;

// ===========================================================================
// Top-level application error
// ===========================================================================

/// Root error type that unifies all module-specific errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Recipe parsing or flavor resolution failed.
    #[error(transparent)]
    Recipe(#[from] RecipeError),

    /// Image build failed.
    #[error(transparent)]
    Builder(#[from] BuilderError),

    /// Version-control operation failed.
    #[error(transparent)]
    Vcs(#[from] VcsError),

    /// Registry push or query failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),

    /// Database persistence failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),

    /// Notification delivery failed.
    #[error(transparent)]
    Notifier(#[from] NotifierError),

    /// Shell command execution failed.
    #[error(transparent)]
    Command(#[from] CommandError),

    /// System requirements check failed.
    #[error(transparent)]
    Checker(#[from] CheckerError),

    /// Metrics server failed.
    #[error(transparent)]
    Metrics(#[from] MetricsError),

    /// Generic I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ===========================================================================
// Module-specific errors
// ===========================================================================

/// Errors produced by the [`crate::config`] module.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Could not read the configuration file from disk.
    #[error("failed to read config file at {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// TOML deserialization failed.
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    /// A semantic validation rule was violated.
    #[error("invalid configuration: {0}")]
    Invalid(String),

    /// A required field was absent.
    #[error("missing required field: {0}")]
    MissingField(String),

    /// An expected environment variable was not set.
    #[error("environment variable not set: {0}")]
    MissingEnvVar(String),
}

/// Errors produced by the [`crate::recipe`] module.
#[derive(Debug, Error)]
pub enum RecipeError {
    /// Could not read the recipe TOML from disk.
    #[error("failed to read recipe file at {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// TOML deserialization failed.
    #[error("failed to parse recipe: {0}")]
    Parse(#[from] toml::de::Error),

    /// A selected flavor value is not listed in `flavours.available`.
    #[error(
        "incompatible flavour: '{flavour}' value '{value}' not in \
         available options"
    )]
    IncompatibleFlavour { flavour: String, value: String },

    /// A flavor dimension referenced in config/CLI does not exist in the
    /// recipe.
    #[error("unknown flavour dimension: {0}")]
    UnknownFlavour(String),

    /// The recipe requires a schema version this build cannot handle.
    #[error("unsupported recipe schema version: {0}")]
    UnsupportedSchema(u32),

    /// The recipe requires a newer Dockermint release.
    #[error(
        "minimum dockermint version not met: requires {required}, \
         running {running}"
    )]
    VersionMismatch { required: String, running: String },
}

/// Errors produced by the [`crate::builder`] module.
#[derive(Debug, Error)]
pub enum BuilderError {
    /// Dockerfile generation failed.
    #[error("dockerfile generation failed: {0}")]
    DockerfileGeneration(String),

    /// The build command returned a non-zero exit code.
    #[error("build failed for {recipe}:{tag} -- {reason}")]
    BuildFailed {
        recipe: String,
        tag: String,
        reason: String,
    },

    /// A buildx builder instance could not be created or inspected.
    #[error("buildx setup failed: {0}")]
    BuildxSetup(String),

    /// Template variable resolution failed.
    #[error("unresolved template variable: {0}")]
    UnresolvedVariable(String),

    /// Underlying command execution error.
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// Errors produced by the [`crate::scrapper`] module (VCS operations).
#[derive(Debug, Error)]
pub enum VcsError {
    /// HTTP request to the VCS provider failed.
    #[error("VCS request failed: {0}")]
    Request(String),

    /// Response body could not be parsed.
    #[error("failed to parse VCS response: {0}")]
    Parse(String),

    /// Authentication was rejected.
    #[error("VCS authentication failed: {0}")]
    Auth(String),

    /// Rate limit reached.
    #[error("VCS rate limit exceeded, retry after {retry_after_secs}s")]
    RateLimit { retry_after_secs: u64 },
}

/// Errors produced by the [`crate::push`] module (registry operations).
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Authentication against the registry failed.
    #[error("registry authentication failed: {0}")]
    Auth(String),

    /// Image push failed.
    #[error("failed to push {image}:{tag} -- {reason}")]
    Push {
        image: String,
        tag: String,
        reason: String,
    },

    /// Query for existing tags failed.
    #[error("registry query failed: {0}")]
    Query(String),

    /// Underlying command execution error.
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// Errors produced by the [`crate::saver`] module (database operations).
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// Opening or creating the database failed.
    #[error("failed to open database: {0}")]
    Open(String),

    /// A read operation failed.
    #[error("database read failed: {0}")]
    Read(String),

    /// A write operation failed.
    #[error("database write failed: {0}")]
    Write(String),

    /// Serialization or deserialization of a stored record failed.
    #[error("database serialization error: {0}")]
    Serialization(String),
}

/// Errors produced by the [`crate::notifier`] module.
#[derive(Debug, Error)]
pub enum NotifierError {
    /// Sending the notification failed.
    #[error("notification delivery failed: {0}")]
    Send(String),

    /// Notification service configuration is invalid.
    #[error("notifier misconfigured: {0}")]
    Config(String),
}

/// Errors produced by the [`crate::commands`] module (shell execution).
#[derive(Debug, Error)]
pub enum CommandError {
    /// The command could not be spawned.
    #[error("failed to spawn command '{command}': {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    /// The command exited with a non-zero status.
    #[error("command '{command}' exited with status {status}\nstderr:\n{stderr}")]
    NonZeroExit {
        command: String,
        status: i32,
        stderr: String,
    },

    /// Reading stdout/stderr failed.
    #[error("failed to capture output of '{command}': {source}")]
    OutputCapture {
        command: String,
        #[source]
        source: std::io::Error,
    },
}

/// Errors produced by the [`crate::checker`] module.
#[derive(Debug, Error)]
pub enum CheckerError {
    /// A required system tool is missing.
    #[error("required tool not found: {tool}")]
    MissingTool { tool: String },

    /// Another Dockermint instance is already running.
    #[error("another Dockermint instance is running (lock file: {lock_path})")]
    AlreadyRunning { lock_path: PathBuf },

    /// System check could not be performed.
    #[error("system check failed: {0}")]
    CheckFailed(String),

    /// Underlying command execution error.
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// Errors produced by the [`crate::metrics`] module.
#[derive(Debug, Error)]
pub enum MetricsError {
    /// The metrics HTTP server failed to bind or serve.
    #[error("metrics server error: {0}")]
    Server(String),

    /// Metric registration failed.
    #[error("metric registration failed: {0}")]
    Registration(String),
}
