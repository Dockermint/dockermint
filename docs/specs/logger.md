# Feature: Structured Logging with Rotation

## Context

The `logger` module provides structured logging throughout the Dockermint
pipeline. It initializes the `tracing` subscriber, configures output targets,
log levels, and file rotation. Different modes (CLI, Daemon, RPC) require
different logging configurations.

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)
Architecture reference: docs/specs/architecture-overview.md, Sections 1.3, 2.3

---

## Requirements

1. [confirmed] Use the `tracing` crate ecosystem for structured logging
2. [confirmed] Support log rotation (file-based) for daemon and RPC modes
3. [confirmed] Support multiple output targets: stdout, file, or both
4. [confirmed] Log levels configurable via `[general] log_level` in config.toml
5. [confirmed] CLI args override config.toml log level
6. [confirmed] Contextual structured fields: recipe name, chain, version, build stage
7. [confirmed] Secrets must never appear in log output (enforced by `secrecy` crate)
8. [confirmed] CLI mode: stdout with optional file output, human-readable format
9. [confirmed] Daemon mode: file output with rotation, JSON format for machine parsing
10. [confirmed] RPC mode: same as daemon mode

---

## Architecture

### Module placement

```
src/logger/
    mod.rs              -- Public API: init_logger(), re-exports
    error.rs            -- LoggerError enum (thiserror)
```

This module is NOT behind a feature gate. There is a single logging backend
(`tracing` with `tracing-subscriber` layers). See architecture-overview.md
Section 2.3.

The logger module is intentionally small. It configures and initializes the
tracing subscriber at startup. After initialization, all other modules use
`tracing` macros (`tracing::info!`, `tracing::error!`, etc.) directly -- they
do not depend on the logger module.

### Type design

#### Logger configuration (derived from AppConfig)

```
LoggerConfig
  +-- level: LogLevel                  -- Trace | Debug | Info | Warn | Error
  +-- log_dir: PathBuf                 -- Directory for log files
  +-- mode: LogOutputMode              -- Enum: Stdout | File | Both
  +-- format: LogFormat                -- Enum: Pretty | Json
```

`LoggerConfig` is not a separate config.toml section. It is constructed from
`GeneralConfig` fields at initialization time:

```
CLI mode:
  mode = Both (stdout for user, file for debug)
  format = Pretty (human-readable on stdout), Json (in file)

Daemon mode:
  mode = File
  format = Json

RPC mode:
  mode = File
  format = Json
```

#### Enums

```
LogOutputMode  -- Stdout | File | Both
LogFormat      -- Pretty | Json
```

These are internal to the logger module. They are derived from the `Mode`
enum in the config module, not directly configurable by the user. The mapping
is deterministic:

| AppConfig mode | Output | Stdout format | File format |
| :------------- | :----- | :------------ | :---------- |
| Cli | Both | Pretty | Json |
| Daemon | File | N/A | Json |

### Structured logging architecture

The `tracing` ecosystem is composed of layers. The logger module constructs
a layered subscriber at startup:

```
tracing_subscriber::Registry
  +-- EnvFilter layer           (level filtering from config)
  +-- IndicatifLayer            (CLI mode only, tracing-indicatif progress bars)
  +-- fmt::Layer (stdout)       (CLI mode only, Pretty format, writes through indicatif)
  +-- fmt::Layer (file)         (all modes, Json format, with rotation)
```

#### Layer details

1. **EnvFilter layer**: Controls which spans and events are recorded based on
   the configured log level. Supports per-module overrides via `RUST_LOG` env
   var for debugging.

2. **Stdout layer** (CLI mode):
   - `tracing_subscriber::fmt::Layer` with `Pretty` formatter
   - Includes timestamps, level, target module, span context
   - Disabled in daemon/RPC modes (no terminal attached)

3. **File layer** (all modes):
   - `tracing_subscriber::fmt::Layer` writing to a file
   - JSON format for machine parsing and log aggregation
   - Combined with `tracing-appender` for rotation

### Log rotation strategy

File-based rotation uses `tracing-appender`:

| Parameter | Value | Notes |
| :-------- | :---- | :---- |
| Rotation period | Daily | New file per day |
| File prefix | `dockermint` | Produces `dockermint.YYYY-MM-DD.log` |
| Directory | `log_dir` from config | Default: `/var/log/dockermint` |
| Max files | 5 | Retain up to 5 rotated log files before cleanup |

Note: `tracing-appender` handles rotation but does not natively support max
file retention. If retention is needed, a cleanup mechanism must be added
(periodic task in daemon, or delegated to external logrotate).

### Log levels and filtering

| Level | Usage |
| :---- | :---- |
| `trace` | Extremely verbose: variable resolution steps, TOML parse details |
| `debug` | Detailed operational info: recipe loaded, flavor resolved, API call made |
| `info` | Key events: build started, build completed, push succeeded |
| `warn` | Non-fatal issues: deprecated config field, rate limit approaching |
| `error` | Failures: build failed, push failed, config invalid |

The level from config.toml sets the minimum. `RUST_LOG` env var can override
for debugging (e.g., `RUST_LOG=dockermint::builder=trace`).

### Contextual fields

Structured logging fields are attached via `tracing::Span`. Key spans in the
pipeline:

| Span | Fields | Created by |
| :--- | :----- | :--------- |
| `pipeline` | `recipe`, `version` | Pipeline orchestrator |
| `build` | `platform`, `builder_name` | Builder module |
| `push` | `registry`, `image_ref` | Push module |
| `poll_cycle` | `cycle_number`, `timestamp` | Daemon loop |
| `grpc_request` | `method`, `client_addr` | RPC handler |

Example log output (JSON format):

```json
{
  "timestamp": "2026-04-13T15:30:00.000Z",
  "level": "INFO",
  "target": "dockermint::builder",
  "message": "build completed",
  "recipe": "cosmos-gaiad",
  "version": "v19.0.0",
  "platform": "linux/amd64",
  "duration_secs": 142
}
```

### Mode-specific logging behavior

| Concern | CLI | Daemon | RPC |
| :------ | :-- | :----- | :-- |
| Stdout output | Yes (Pretty format) | No | No |
| File output | Yes (Json format) | Yes (Json format) | Yes (Json format) |
| Rotation | Yes | Yes | Yes |
| Progress bars | Yes (`indicatif`) | No | No |
| Span context | Per-build | Per-cycle + per-build | Per-request + per-build |

In CLI mode, `indicatif` progress bars and `tracing` stdout output must not
interleave. The `tracing-indicatif` crate is used to integrate progress bars
with the tracing subscriber, so that log lines and progress bars coexist
without corruption. The tracing subscriber layer stack in CLI mode becomes:

```
tracing_subscriber::Registry
  +-- EnvFilter layer
  +-- IndicatifLayer              (tracing-indicatif: manages progress bars)
  +-- fmt::Layer (stdout)         (Pretty format, writes through indicatif)
  +-- fmt::Layer (file)           (Json format, with rotation)
```

The `IndicatifLayer` is only added in CLI mode. Daemon and RPC modes do not
use progress bars and omit this layer.

### Error types

```
LoggerError (thiserror)
  +-- DirectoryCreationFailed { path: PathBuf, source: std::io::Error }
  +-- SubscriberInitFailed { reason: String }
  +-- InvalidLogLevel { value: String }
```

`LoggerError` is minimal because logger initialization is a startup-only
operation. If it fails, the application exits immediately (no logging is
possible without a functioning logger).

### Dependencies

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `tracing` | Structured logging facade | Macros used by all modules |
| `tracing-subscriber` | Subscriber implementation | With `fmt`, `env-filter`, `json` features |
| `tracing-appender` | File output with rotation | Non-blocking writer, daily rotation |
| `tracing-indicatif` | Progress bar integration with tracing | CLI mode only; IndicatifLayer in subscriber stack |

Delegate to @lead-dev: evaluate `tracing`, `tracing-subscriber`,
`tracing-appender`, `tracing-indicatif` for latest version, API surface,
musl/aarch64 compatibility.

---

## Interface contract

```rust
/// Initialize the global tracing subscriber based on configuration.
///
/// This function MUST be called exactly once at application startup, before
/// any tracing macros are used. It sets the global default subscriber.
///
/// # Arguments
///
/// * `config` - Logger configuration derived from AppConfig
///
/// # Returns
///
/// A guard that must be held for the lifetime of the application.
/// Dropping the guard flushes pending log writes.
///
/// # Errors
///
/// Returns LoggerError if the log directory cannot be created or the
/// subscriber cannot be initialized.
pub fn init_logger(config: &LoggerConfig) -> Result<LoggerGuard, LoggerError>;

/// Opaque guard type. Holds the non-blocking writer handle.
/// Dropping this guard flushes buffered log entries to disk.
pub struct LoggerGuard { /* private fields */ }
```

---

## Module interaction diagram

```
[config] ---> LoggerConfig (derived from GeneralConfig + Mode)
                   |
                   v
              [logger/mod.rs]
                   |
         +--------+--------+
         |                  |
         v                  v
   stdout layer        file layer
   (CLI mode)     (tracing-appender)
         |                  |
         v                  v
     terminal          log files
                   (dockermint.YYYY-MM-DD.log)


After initialization, all modules use tracing macros directly:

[recipe] --tracing::info!--> subscriber ---> stdout / file
[builder] --tracing::error!--> subscriber ---> stdout / file
[scrapper] --tracing::debug!--> subscriber ---> stdout / file
```

---

## Testing strategy

- **Unit tests**: `LoggerConfig` correctly derived from each `Mode` variant
  (CLI -> Both+Pretty, Daemon -> File+Json).
- **Unit tests**: Log level filtering works -- events below threshold are
  discarded.
- **Unit tests**: `LoggerError` variants format with meaningful messages.
- **Integration tests**: Initialize logger with a temp directory, emit events,
  verify log file is created with expected JSON content.
- **Integration tests**: Verify `Secret<String>` values log as `[REDACTED]`
  (test that secrecy integration prevents leaks).
- **Mock**: File system for log directory creation.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| L1 | How many rotated log files should be retained before cleanup? | RESOLVED | 5 files. A cleanup mechanism removes files beyond the 5 most recent. |
| L2 | Should `tracing-indicatif` be used for CLI mode to integrate progress bars with logging, or should they be coordinated manually? | RESOLVED | YES -- use `tracing-indicatif`. The `IndicatifLayer` is added to the subscriber stack in CLI mode to integrate progress bars with structured logging output. |
