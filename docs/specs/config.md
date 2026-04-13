# Feature: Configuration Loading and Validation

## Context

The `config` module is the first module invoked in every Dockermint pipeline
run. It loads, merges, and validates configuration from multiple sources with
a defined priority chain. It provides the `AppConfig` struct that all other
modules consume.

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)
Architecture reference: docs/specs/architecture-overview.md, Section 4

---

## Requirements

1. [confirmed] Load configuration from `config.toml` with `serde` + `toml` crate
2. [confirmed] Load secrets from `.env` via `dotenvy` crate
3. [confirmed] Wrap secret values with `secrecy` crate to prevent accidental logging
4. [confirmed] Support priority chain: CLI args > env vars (.env) > config.toml > recipe defaults > hardcoded defaults
5. [confirmed] Version all configuration files via `[meta] config_version` field
6. [confirmed] Reject unknown fields via `#[serde(deny_unknown_fields)]`
7. [confirmed] Global daemon poll interval in `[daemon]`, overridable per-chain in `[daemon.chains."<name>"]`
8. [confirmed] BuildKit Docker socket URI configurable in `[builder] docker_host`
9. [confirmed] gRPC supports both mTLS and token-based auth, configured in `[grpc]`
10. [confirmed] Notifier non-secret config in `[notifier]`, secrets in `.env`
11. [confirmed] Secrets ONLY in `.env`, never in `config.toml`
12. [confirmed] Validation must catch errors early, before pipeline starts

---

## Architecture

### Module placement

```
src/config/
    mod.rs              -- Public API: load_config(), AppConfig, re-exports
    error.rs            -- ConfigError enum (thiserror)
    schema.rs           -- All config section structs (serde Deserialize)
    validation.rs       -- Post-deserialization validation rules
    secrets.rs          -- .env loading, Secret<String> wrappers
    merge.rs            -- Priority chain merge logic (CLI args overlay)
```

This module is NOT behind a feature gate. There is a single config format
(TOML) and a single loading strategy. See architecture-overview.md Section 2.3.

### Type design

#### Top-level config struct

```
AppConfig
  +-- meta: MetaConfig
  +-- general: GeneralConfig
  +-- daemon: DaemonConfig
  +-- grpc: GrpcConfig
  +-- builder: BuilderConfig
  +-- registry: RegistryConfig
  +-- notifier: NotifierConfig
  +-- metrics: MetricsConfig
  +-- flavours: FlavoursConfig
```

#### Section structs

```
MetaConfig
  +-- config_version: u32              -- Schema version (REQUIRED)

GeneralConfig
  +-- mode: Mode                       -- Enum: Cli | Daemon
  +-- log_level: LogLevel              -- Enum: Trace | Debug | Info | Warn | Error
  +-- log_dir: PathBuf                 -- Directory for rotated log files
  +-- recipes_dir: PathBuf             -- Path to recipes directory

DaemonConfig
  +-- poll_interval_secs: u64          -- Global poll interval (default: 300)
  +-- chains: HashMap<String, ChainDaemonConfig>  -- Per-chain overrides

ChainDaemonConfig
  +-- poll_interval_secs: Option<u64>  -- Override global poll interval for this chain

GrpcConfig
  +-- enabled: bool                    -- Enable gRPC server in daemon mode
  +-- listen_address: SocketAddr       -- gRPC bind address
  +-- auth_mode: GrpcAuthMode          -- Enum: Token | Mtls | Both
  +-- tls_cert_path: Option<PathBuf>   -- Server TLS cert (mTLS / both)
  +-- tls_key_path: Option<PathBuf>    -- Server TLS key (mTLS / both)
  +-- tls_ca_path: Option<PathBuf>     -- CA cert for client verification (mTLS / both)

BuilderConfig
  +-- platforms: Vec<String>           -- Target platforms (e.g., "linux/amd64")
  +-- docker_host: Option<String>      -- Docker socket URI, empty = system default

RegistryConfig
  +-- url: String                      -- Registry URL (e.g., "ghcr.io/dockermint")

NotifierConfig
  +-- enabled: bool                    -- Enable/disable notifications

MetricsConfig
  +-- enabled: bool                    -- Enable/disable metrics server
  +-- listen_address: SocketAddr       -- Prometheus scrape endpoint

FlavoursConfig
  +-- global: HashMap<String, toml::Value>              -- Global flavor overrides
  +-- recipes: HashMap<String, HashMap<String, toml::Value>>  -- Per-recipe overrides
```

#### Enums

```
Mode           -- Cli | Daemon
LogLevel       -- Trace | Debug | Info | Warn | Error
GrpcAuthMode   -- Token | Mtls | Both
```

All structs derive: `Debug`, `Clone`, `PartialEq`, `serde::Deserialize`.
All structs use `#[serde(deny_unknown_fields)]`.
`GrpcAuthMode` and `Mode` additionally derive `serde::Serialize` for
round-trip diagnostics.

#### Secrets struct

```
AppSecrets
  +-- gh_pat: Option<Secret<String>>
  +-- gh_user: Option<Secret<String>>
  +-- registry_user: Option<Secret<String>>
  +-- registry_password: Option<Secret<String>>
  +-- telegram_bot_token: Option<Secret<String>>
  +-- telegram_chat_id: Option<Secret<String>>
  +-- grpc_auth_token: Option<Secret<String>>
```

`Secret<String>` comes from the `secrecy` crate. It implements `Debug` as
`"[REDACTED]"` and requires explicit `.expose_secret()` to access the value,
preventing accidental logging.

### Configuration file: config.toml

Full schema with all sections:

```toml
# ---------------------------------------------------------------
# Dockermint Configuration File
# ---------------------------------------------------------------

[meta]
config_version = 1                    # REQUIRED: schema version

[general]
mode = "cli"                          # "cli" | "daemon"
log_level = "info"                    # "trace" | "debug" | "info" | "warn" | "error"
log_dir = "/var/log/dockermint"       # Directory for rotated log files
recipes_dir = "./recipes"             # Path to recipes directory

# ---------------------------------------------------------------
# Daemon mode configuration
# ---------------------------------------------------------------

[daemon]
poll_interval_secs = 300              # Global default: seconds between VCS polls

[daemon.chains."cosmos-gaiad"]        # Per-chain poll interval override
poll_interval_secs = 600

[daemon.chains."kyve-kyved"]
poll_interval_secs = 180

# ---------------------------------------------------------------
# gRPC server (daemon mode only)
# ---------------------------------------------------------------

[grpc]
enabled = false                       # Enable gRPC server
listen_address = "0.0.0.0:50051"      # Bind address
auth_mode = "token"                   # "token" | "mtls" | "both"
tls_cert_path = ""                    # Server TLS cert path (mTLS / both)
tls_key_path = ""                     # Server TLS key path (mTLS / both)
tls_ca_path = ""                      # CA cert for client verification (mTLS / both)
# Token secret in .env: GRPC_AUTH_TOKEN

# ---------------------------------------------------------------
# Builder configuration
# ---------------------------------------------------------------

[builder]
platforms = ["linux/amd64", "linux/arm64"]
docker_host = ""                      # Docker socket URI. Empty = system default.
                                      # Examples: "unix:///var/run/docker.sock"
                                      #           "tcp://remote:2376"

# ---------------------------------------------------------------
# Registry configuration
# ---------------------------------------------------------------

[registry]
url = ""                              # e.g., "ghcr.io/dockermint"
# Credentials in .env: REGISTRY_USER, REGISTRY_PASSWORD

# ---------------------------------------------------------------
# Notifier configuration
# ---------------------------------------------------------------

[notifier]
enabled = true                        # Enable/disable notifications
# Secrets in .env: TELEGRAM_BOT_TOKEN, TELEGRAM_CHAT_ID

# ---------------------------------------------------------------
# Metrics configuration
# ---------------------------------------------------------------

[metrics]
enabled = true
listen_address = "0.0.0.0:9100"      # Prometheus scrape endpoint

# ---------------------------------------------------------------
# Flavor overrides
# ---------------------------------------------------------------

[flavours]
# Global overrides (apply to all recipes unless recipe-specific section exists)
db_backend = "goleveldb"
binary_type = "static"

[flavours.recipes."cosmos-gaiad"]
# Recipe-specific overrides
db_backend = "pebbledb"
running_env = "distroless"

[flavours.recipes."kyve-kyved"]
network = "kaon"
```

### Priority chain

The merge logic resolves each configuration value by checking sources in
order. The first source that provides a value wins:

```
Priority 1: CLI arguments          (--flag value)
    |
    v
Priority 2: Environment variables  (.env via dotenvy, secrets only)
    |
    v
Priority 3: config.toml            (file-based configuration)
    |
    v
Priority 4: Recipe defaults        ([flavours.default] in recipe TOML)
    |
    v
Priority 5: Hardcoded defaults     (compiled into Rust structs via Default trait)
```

- Priority 1 applies to operational flags (mode, log level, recipe filter,
  specific flavors). CLI args produce an "overlay" struct with `Option` fields.
  Non-None values override config.toml.
- Priority 2 applies exclusively to secrets. Secrets are never in config.toml.
- Priority 3 is the base configuration file.
- Priority 4 applies only to flavor values, resolved during recipe parsing
  (handled by the `recipe` module, not `config`).
- Priority 5 is the `Default` trait implementation on each config section struct.

The merge function signature (conceptual):

```
fn merge_config(
    cli_overrides: CliOverrides,
    file_config: AppConfig,
    secrets: AppSecrets,
) -> Result<AppConfig, ConfigError>
```

### Config versioning strategy

- `[meta] config_version` is a monotonically increasing integer.
- The binary defines a constant `SUPPORTED_CONFIG_VERSIONS: &[u32]` listing
  all versions it can read.
- On load, if the file's version is not in the supported set, the binary
  exits with `ConfigError::UnsupportedVersion` providing:
  - The file's version
  - The supported range
  - A migration hint (if applicable)
- When the config schema changes:
  1. Increment the version constant.
  2. Add a migration note in the spec.
  3. The old version may remain supported for one minor release cycle.

### Daemon poll interval resolution

For a given chain `C`, the effective poll interval is:

```
if daemon.chains["C"].poll_interval_secs is Some(v):
    use v
else:
    use daemon.poll_interval_secs  (global default)
```

This is resolved at daemon startup and stored in the per-chain runtime config.

### Error types

```
ConfigError (thiserror)
  +-- FileNotFound { path: PathBuf }
  +-- ParseError { source: toml::de::Error }
  +-- UnsupportedVersion { found: u32, supported: Vec<u32> }
  +-- ValidationError { field: String, reason: String }
  +-- EnvLoadError { source: dotenvy::Error }
  +-- MissingSecret { name: String }
  +-- InvalidSocketAddr { value: String, source: std::net::AddrParseError }
  +-- GrpcTlsConfigIncomplete { missing_field: String }
  +-- InvalidMode { value: String }
```

### Validation rules

Post-deserialization validation (in `validation.rs`):

1. `config_version` is in `SUPPORTED_CONFIG_VERSIONS`
2. `mode` is a valid enum variant
3. `log_dir` is a valid directory path (existence checked at runtime, not parse time)
4. `recipes_dir` exists and is a directory
5. `poll_interval_secs` > 0 (global and per-chain)
6. If `grpc.enabled` and `auth_mode` is `Mtls` or `Both`:
   - `tls_cert_path`, `tls_key_path`, `tls_ca_path` must all be non-empty
7. If `grpc.enabled` and `auth_mode` is `Token` or `Both`:
   - `GRPC_AUTH_TOKEN` must be present in `.env`
8. `platforms` is non-empty and contains valid platform strings
9. `metrics.listen_address` and `grpc.listen_address` do not conflict
10. If `notifier.enabled`, required secrets (`TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`)
    must be present in `.env`

### Dependencies

External crates needed:

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `serde` | Deserialization framework | With `derive` feature |
| `toml` | TOML parser | config.toml and recipe files |
| `dotenvy` | `.env` file loading | Secrets |
| `secrecy` | Secret value wrapping | Prevents accidental logging |
| `thiserror` | Error type definitions | `ConfigError` |

Delegate to @lead-dev: evaluate each crate for latest version, API surface,
musl/aarch64 compatibility.

---

## Interface contract

```rust
// -- Public API that the implementation must satisfy --

/// Load configuration from all sources, merge, validate, and return.
///
/// # Arguments
///
/// * `config_path` - Path to config.toml file
/// * `cli_overrides` - Optional overrides from CLI arguments
///
/// # Returns
///
/// Fully merged and validated AppConfig
///
/// # Errors
///
/// Returns ConfigError for any loading, parsing, or validation failure
pub fn load_config(
    config_path: &Path,
    cli_overrides: Option<&CliOverrides>,
) -> Result<AppConfig, ConfigError>;

/// Load secrets from .env file.
///
/// # Errors
///
/// Returns ConfigError::EnvLoadError if .env cannot be read,
/// or ConfigError::MissingSecret if a required secret is absent
pub fn load_secrets() -> Result<AppSecrets, ConfigError>;

/// Resolve the effective poll interval for a specific chain.
///
/// # Arguments
///
/// * `daemon_config` - The daemon configuration section
/// * `chain_name` - The chain identifier (e.g., "cosmos-gaiad")
///
/// # Returns
///
/// The poll interval in seconds (per-chain override or global default)
pub fn resolve_poll_interval(
    daemon_config: &DaemonConfig,
    chain_name: &str,
) -> u64;
```

---

## Module interaction diagram

```
                          .env file
                             |
                             v
CLI args ---> [config/merge.rs] <--- config.toml
                     |
                     v
              [config/schema.rs]   (deserialize into typed structs)
                     |
                     v
              [config/validation.rs]  (post-deserialization checks)
                     |
                     v
               AppConfig + AppSecrets
                     |
        +------------+------------+
        |            |            |
        v            v            v
    [checker]    [recipe]     [builder]    ... (all downstream modules)
```

All downstream modules receive `&AppConfig` (borrowed). The config module
owns the `AppConfig` instance; it is created once at startup and passed by
reference throughout the pipeline.

---

## Testing strategy

- **Unit tests**: Each config section struct deserializes correctly from valid
  TOML fragments. Unknown fields are rejected. Missing required fields produce
  clear errors.
- **Unit tests**: Priority chain merge logic correctly overlays CLI args onto
  file config. Non-None CLI values win. None CLI values pass through.
- **Unit tests**: Validation catches all invalid states (zero poll interval,
  mTLS without cert paths, missing secrets for enabled features).
- **Unit tests**: `resolve_poll_interval` returns per-chain value when present,
  global when absent.
- **Unit tests**: Config version check rejects unsupported versions with the
  correct error variant.
- **Integration tests**: Load a complete config.toml + .env fixture, verify the
  resulting `AppConfig` matches expectations.
- **Mock**: File system for config path resolution. Environment for .env loading.

---

## Open questions

None. All questions resolved by CEO decisions (Q1, Q2, Q3, Q5, Q6, Q7 from
architecture-overview.md Appendix B).
