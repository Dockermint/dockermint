# Architecture Overview

This document is the foundational architecture reference for Dockermint. All
per-module specs reference this document for system-wide design decisions,
conventions, and data flow.

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)

---

## Table of Contents

1. [System Architecture](#1-system-architecture)
2. [Trait-First Design](#2-trait-first-design)
3. [Feature Gate Strategy](#3-feature-gate-strategy)
4. [Configuration Architecture](#4-configuration-architecture)
5. [Recipe System](#5-recipe-system)
6. [Error Strategy](#6-error-strategy)
7. [Binary Architecture](#7-binary-architecture)
8. [Cross-Compilation](#8-cross-compilation)

---

## 1. System Architecture

### 1.1 High-Level Module Diagram

```
+-------------------------------------------------------------------+
|                        ENTRY POINTS                               |
|                                                                   |
|  dockermint-cli          dockermint-daemon                        |
|  (one-shot build)        (polling loop + optional gRPC server)    |
|       |                       |                                   |
|       v                       v                                   |
|  +----------+           +----------+    +----------+              |
|  | cli      |           | cli      |    | rpc      |              |
|  | (clap)   |           | (daemon) |    | (axum/   |              |
|  +----+-----+           +----+-----+    | tonic)   |              |
|       |                      |          +----+-----+              |
|       |                      |               |                    |
+-------+----------------------+---------------+--------------------+
        |                      |               |
        v                      v               v
+-------------------------------------------------------------------+
|                     SHARED CORE PIPELINE                          |
|                                                                   |
|  +----------+    +----------+    +----------+    +----------+     |
|  | config   |--->| checker  |--->| recipe   |--->| scrapper |     |
|  | (load +  |    | (system  |    | (parse + |    | (VCS     |     |
|  |  merge)  |    |  reqs)   |    |  resolve |    |  fetch)  |     |
|  +----------+    +----------+    |  flavors)|    +----+-----+     |
|                                  +----------+         |           |
|                                                       v           |
|  +----------+    +----------+    +----------+    +----------+     |
|  | push     |<---| builder  |<---| builder  |<---| builder  |     |
|  | (registry|    | (buildx  |    | (Dock-   |    | (template|     |
|  |  push)   |    |  exec)   |    |  erfile  |    |  engine) |     |
|  +----+-----+    +----------+    |  gen)    |    +----------+     |
|       |                          +----------+                     |
|       v                                                           |
|  +----------+    +----------+    +----------+                     |
|  | saver    |    | notifier |    | metrics  |                     |
|  | (persist |    | (status  |    | (export) |                     |
|  |  state)  |    |  alerts) |    +----------+                     |
|  +----------+    +----------+                                     |
|                                                                   |
+-------------------------------------------------------------------+
|                     CROSS-CUTTING CONCERNS                        |
|                                                                   |
|  +----------+    +----------+                                     |
|  | logger   |    | commands |                                     |
|  | (tracing |    | (shell   |                                     |
|  |  + rot.) |    |  exec)   |                                     |
|  +----------+    +----------+                                     |
+-------------------------------------------------------------------+
```

### 1.2 Core Build Pipeline

The core pipeline is shared by all three modes. The modes differ only in how
they invoke the pipeline and how they handle errors at its boundaries.

```
[1. Config Load]
 Load config.toml, merge with CLI args, load .env secrets
         |
         v
[2. System Check]
 Verify Docker, BuildKit, network, disk prerequisites
         |
         v
[3. Recipe Parse]
 Discover recipe TOML files, parse into typed structures
         |
         v
[4. Flavor Resolution]
 CLI args > config.toml overrides > recipe defaults
 Validate flavor compatibility
         |
         v
[5. VCS Fetch]
 Query VCS API for tags/releases, apply include/exclude glob filters
 Determine which versions to build
         |
         v
[6. Template Engine]
 Resolve {{UPPERCASE}} host variables and {{lowercase}} build variables
 Execute shell-type variables to capture outputs
         |
         v
[7. Dockerfile Generation]
 Produce multi-stage Dockerfile from recipe + resolved variables
         |
         v
[8. Build Execution]
 Invoke BuildKit via buildx for each target platform
 Per-platform builders: dockermint-amd64, dockermint-arm64
         |
         v
[9. Push / Save]
 Push to OCI registry (if configured)
 Persist build result to database
         |
         v
[10. Notify / Metrics]
 Send status notification (daemon/RPC modes)
 Update metrics counters
```

### 1.3 Mode-Specific Behavior

| Concern          | CLI                     | Daemon                      | RPC                          |
| :--------------- | :---------------------- | :-------------------------- | :--------------------------- |
| Invocation       | User runs command       | Polling loop on timer       | Incoming gRPC request        |
| Pipeline scope   | Single recipe + version | All configured recipes      | Single recipe + version      |
| Persistence      | Optional                | Required (saver)            | Required (saver)             |
| Notifications    | None                    | Required (notifier)         | Required (notifier)          |
| Metrics          | None                    | Required (metrics)          | Required (metrics)           |
| Error terminal   | Exit with code          | Log + notify + persist + continue | Log + return idle     |
| Progress display | indicatif progress bars | Structured log only         | Structured log only          |

---

## 2. Trait-First Design

Every swappable module is defined as a trait. The trait is the public contract.
Concrete implementations are selected at compile time via feature gates.
This ensures zero-cost abstraction: no dynamic dispatch overhead in the
default path.

### 2.1 Design Pattern

Each swappable module follows the same structure:

```
src/<module>/
    mod.rs          -- Trait definition + re-export of active implementation
    error.rs        -- Module-specific error type (thiserror)
    <default>/      -- Default implementation (behind default feature)
        mod.rs
    <alt>/          -- Alternative implementation (behind optional feature)
        mod.rs
```

The `mod.rs` at the module root defines the trait and uses conditional
compilation to re-export the active implementation:

```
// Conceptual structure (not literal Rust -- see per-module specs for
// exact signatures):
//
// pub trait ModuleName { ... }
//
// #[cfg(feature = "module-default")]
// mod default_impl;
// #[cfg(feature = "module-default")]
// pub use default_impl::DefaultImpl;
//
// #[cfg(feature = "module-alt")]
// mod alt_impl;
// #[cfg(feature = "module-alt")]
// pub use alt_impl::AltImpl;
```

Application code references the trait. The concrete type is resolved at the
binary entrypoint (CLI or daemon main) through a type alias or generic
parameter, keeping the core pipeline generic.

### 2.2 Core Trait Summary

Each trait below is a design-level contract. Exact method signatures, generic
bounds, associated types, and error types will be specified in per-module specs.

#### Database (saver module)

| Aspect | Detail |
| :----- | :----- |
| Trait name | `BuildStore` |
| Purpose | Persist and query build results, failure records, version history |
| Default impl | `RedbStore` (redb -- embedded, zero-config, pure Rust) |
| Key methods | `save_build_result`, `get_build_result`, `list_builds`, `save_failure`, `list_failures` |
| Inputs | Build result struct, query filters (recipe name, version, status) |
| Outputs | `Result<T, StoreError>` for all operations |
| Notes | Must be `Send + Sync` for daemon/RPC shared state. Embedded DB preferred for single-binary deployment |

#### Notifier

| Aspect | Detail |
| :----- | :----- |
| Trait name | `Notifier` |
| Purpose | Send build status notifications (success, failure, error) |
| Default impl | `TelegramNotifier` (Telegram Bot API) |
| Key methods | `notify_success`, `notify_failure`, `notify_error` |
| Inputs | Build context (recipe name, version, duration, error details) |
| Outputs | `Result<(), NotifierError>` |
| Notes | Async trait. Secrets (bot token, chat ID) loaded from .env |

#### VCS (scrapper module)

| Aspect | Detail |
| :----- | :----- |
| Trait name | `VcsClient` |
| Purpose | Fetch tags/releases from a version control hosting service, apply glob filters |
| Default impl | `GithubClient` (GitHub REST API) |
| Key methods | `fetch_releases`, `fetch_tags`, `filter_versions` |
| Inputs | Repository URL, include/exclude glob patterns, auth token |
| Outputs | `Result<Vec<VersionInfo>, VcsError>` |
| Notes | Async trait. PAT loaded from .env. Must handle rate limiting and pagination |

#### Registry (push module)

| Aspect | Detail |
| :----- | :----- |
| Trait name | `RegistryClient` |
| Purpose | Authenticate with and push images to a container registry |
| Default impl | `OciRegistryClient` (OCI Distribution Spec) |
| Key methods | `authenticate`, `push_image`, `tag_exists` |
| Inputs | Registry URL, credentials (from .env), image reference, platform |
| Outputs | `Result<PushResult, RegistryError>` |
| Notes | Async trait. Credentials from .env. Must support multi-arch manifests |

#### Builder

| Aspect | Detail |
| :----- | :----- |
| Trait name | `ImageBuilder` |
| Purpose | Execute the container image build from a generated Dockerfile |
| Default impl | `BuildKitBuilder` (Docker BuildKit via buildx CLI) |
| Key methods | `create_builder`, `build`, `remove_builder` |
| Inputs | Dockerfile content, build context path, target platforms, build args |
| Outputs | `Result<BuildOutput, BuilderError>` |
| Notes | Manages per-platform builder instances (dockermint-amd64, dockermint-arm64). Supports local and remote BuildKit endpoints |

#### Metrics

| Aspect | Detail |
| :----- | :----- |
| Trait name | `MetricsExporter` |
| Purpose | Expose build pipeline metrics for external scraping |
| Default impl | `PrometheusExporter` (Prometheus exposition format via axum endpoint) |
| Key methods | `record_build_start`, `record_build_success`, `record_build_failure`, `record_build_duration` |
| Inputs | Recipe name, version, platform, duration, status |
| Outputs | No return value for recording; HTTP handler for scraping endpoint |
| Notes | Daemon and RPC modes only. Counter and histogram metric types |

### 2.3 Non-Swappable Modules

The following modules are not behind feature gates. They have a single
implementation that may evolve but is not designed for compile-time
replacement:

| Module    | Reason |
| :-------- | :----- |
| `cli`     | Clap command structure is inherent to the binary |
| `config`  | Single config format (TOML) and loading strategy |
| `logger`  | Single logging backend (tracing with rotation) |
| `checker` | System prerequisite checks are deterministic |
| `recipe`  | Recipe format is the core contract of the project |
| `commands`| Shell execution is a low-level utility |

The `builder/go` and `builder/rust` submodules are also not swappable -- they
are recipe-type-specific builder strategies selected at runtime based on the
`[header] type` field in the recipe TOML. Future recipe types would add new
submodules under `builder/` without modifying existing code.

### 2.4 SSL Module

| Aspect | Detail |
| :----- | :----- |
| Feature name | `ssl-openssl` (default), `ssl-rustls` (alternative) |
| Purpose | TLS backend for all outbound HTTPS connections (VCS API, registry, notifications) |
| Design | Not a Dockermint trait. Controlled by selecting the TLS feature on upstream HTTP crates (e.g., reqwest). The feature gate switches which TLS backend is compiled |
| Default | OpenSSL (vendored) -- broadest compatibility, required for some registries |
| Alternative | rustls -- pure Rust, no C dependency, better for musl static builds |

---

## 3. Feature Gate Strategy

### 3.1 Feature Map

| Feature flag      | Module    | Default | What it activates |
| :---------------- | :-------- | :------ | :---------------- |
| `db-redb`         | saver     | Yes     | redb implementation of `BuildStore` |
| `notifier-telegram` | notifier | Yes   | Telegram implementation of `Notifier` |
| `vcs-github`      | scrapper  | Yes     | GitHub implementation of `VcsClient` |
| `ssl-openssl`     | ssl       | Yes     | OpenSSL (vendored) TLS backend |
| `ssl-rustls`      | ssl       | No      | rustls TLS backend |
| `registry-oci`    | push      | Yes     | OCI implementation of `RegistryClient` |
| `builder-buildkit`| builder   | Yes     | BuildKit implementation of `ImageBuilder` |
| `metrics-prometheus` | metrics | Yes   | Prometheus implementation of `MetricsExporter` |

### 3.2 Cargo.toml Feature Declaration (Conceptual)

```toml
[features]
default = [
    "db-redb",
    "notifier-telegram",
    "vcs-github",
    "ssl-openssl",
    "registry-oci",
    "builder-buildkit",
    "metrics-prometheus",
]

# Database backends (exactly one required for daemon/RPC)
db-redb = ["dep:redb"]

# Notification backends (exactly one required for daemon/RPC)
notifier-telegram = ["dep:reqwest"]

# VCS backends (exactly one required)
vcs-github = ["dep:reqwest"]

# TLS backends (exactly one required, mutually exclusive)
ssl-openssl = ["dep:openssl"]
ssl-rustls = ["dep:rustls"]

# Registry backends (exactly one required for push)
registry-oci = ["dep:reqwest"]

# Builder backends (exactly one required)
builder-buildkit = []

# Metrics backends (exactly one required for daemon/RPC)
metrics-prometheus = ["dep:prometheus"]
```

### 3.3 Feature Composition Rules

1. **Mutual exclusion within a category**: Only one implementation per
   swappable module may be active. For example, `ssl-openssl` and `ssl-rustls`
   are mutually exclusive. Compile-time `#[cfg]` assertions enforce this.

2. **Mode-dependent requirements**:
   - CLI mode: `config`, `checker`, `recipe`, `scrapper` (VCS), `builder`,
     `push` (registry). Database, notifier, and metrics are optional.
   - Daemon mode: All modules required. Database, notifier, and metrics
     features must be active.
   - RPC mode: Same as daemon, plus gRPC server dependencies.
   - Library (rlib/cdylib/staticlib): All feature-gated modules are available.
     The consumer activates the features they need.

3. **Default features cover the common case**: Building with `cargo build`
   (no feature flags) produces binaries and libraries with all default
   implementations.

4. **Replacing an implementation**: Build with
   `cargo build --no-default-features --features "db-redb,vcs-github,..."`,
   substituting the desired alternative feature for the module being replaced.

### 3.4 Compile-Time Enforcement

Each swappable module's `mod.rs` includes a compile-time check that exactly one
implementation is selected:

```
// Conceptual pattern:
// #[cfg(not(any(feature = "db-redb")))]
// compile_error!("At least one database backend must be enabled");
//
// For mutually exclusive features (SSL):
// #[cfg(all(feature = "ssl-openssl", feature = "ssl-rustls"))]
// compile_error!("Only one SSL backend may be enabled");
```

---

## 4. Configuration Architecture

### 4.1 Priority Chain

Configuration values are resolved in this order (highest priority first):

```
1. CLI arguments          (--flag value)
2. Environment variables  (.env via dotenvy, for secrets only)
3. config.toml            (file-based configuration)
4. Recipe defaults        ([flavours.default] in recipe TOML)
5. Hardcoded defaults     (compiled into the binary)
```

CLI arguments override config.toml values. config.toml overrides recipe
defaults. Secrets are never in config.toml -- they come exclusively from .env.

### 4.2 config.toml Schema (Design-Level)

```toml
# -------------------------------------------------------
# Dockermint Configuration File
# -------------------------------------------------------

[meta]
config_version = 1                 # REQUIRED: schema version for migration

[general]
mode = "cli"                       # "cli" | "daemon"
log_level = "info"                 # "trace" | "debug" | "info" | "warn" | "error"
log_dir = "/var/log/dockermint"    # Directory for rotated log files
recipes_dir = "./recipes"          # Path to recipes directory

[daemon]
poll_interval_secs = 300           # Seconds between VCS polls (global default)

[daemon.chains."cosmos-gaiad"]     # Per-chain override (overrides global)
poll_interval_secs = 600           # This chain polls every 10 minutes

[daemon.chains."kyve-kyved"]
poll_interval_secs = 180           # This chain polls every 3 minutes

[grpc]
enabled = false                    # Enable gRPC server in daemon mode
listen_address = "0.0.0.0:50051"  # gRPC bind address
auth_mode = "token"                # "token" | "mtls" | "both"
tls_cert_path = ""                 # Path to server TLS cert (mTLS / both)
tls_key_path = ""                  # Path to server TLS key (mTLS / both)
tls_ca_path = ""                   # Path to CA cert for client verification (mTLS / both)
# Token secret in .env: GRPC_AUTH_TOKEN

[builder]
platforms = ["linux/amd64", "linux/arm64"]
docker_host = ""                   # Docker socket URI (e.g., "unix:///var/run/docker.sock",
                                   # "tcp://remote:2376"). Empty = system default.
                                   # On each launch, CLI and Daemon verify the Docker
                                   # context and create/destroy BuildKit builders on
                                   # this endpoint.

[registry]
url = ""                           # Registry URL (e.g., "ghcr.io/dockermint")
# Credentials in .env: REGISTRY_USER, REGISTRY_PASSWORD

[notifier]
enabled = true                     # Enable/disable notifications
# Secrets (bot token, chat ID) in .env: TELEGRAM_BOT_TOKEN, TELEGRAM_CHAT_ID
# Non-secret notifier config lives here in config.toml

[metrics]
enabled = true
listen_address = "0.0.0.0:9100"   # Prometheus scrape endpoint

# -------------------------------------------------------
# Per-recipe flavor overrides
# -------------------------------------------------------
# Override default flavors for specific recipes. If omitted,
# recipe file defaults are used.

[flavours]
# Global overrides (apply to all recipes unless recipe-specific
# section exists below)
db_backend = "goleveldb"
binary_type = "static"

[flavours.recipes."cosmos-gaiad"]
# Recipe-specific overrides
db_backend = "pebbledb"
running_env = "distroless"

[flavours.recipes."kyve-kyved"]
network = "kaon"
```

### 4.3 Config Struct Design

The config.toml maps to a top-level `AppConfig` struct composed of section
structs:

```
AppConfig
  +-- MetaConfig         (config_version)
  +-- GeneralConfig      (mode, log_level, log_dir, recipes_dir)
  +-- DaemonConfig       (poll_interval_secs, chains: HashMap<String, ChainDaemonConfig>)
  +-- GrpcConfig         (enabled, listen_address, auth_mode, tls_cert_path, tls_key_path, tls_ca_path)
  +-- BuilderConfig      (platforms, docker_host)
  +-- RegistryConfig     (url)
  +-- NotifierConfig     (enabled)
  +-- MetricsConfig      (enabled, listen_address)
  +-- FlavoursConfig     (global overrides + per-recipe overrides)
```

Deserialization uses `serde` with `#[serde(deny_unknown_fields)]` to catch
typos in config files early.

### 4.4 Config Versioning

- Every config.toml has a `[meta] config_version` field.
- The binary knows which config versions it supports.
- If the version is unsupported, the binary exits with a clear error message
  explaining the mismatch and any migration steps.
- Recipe TOML files have their own `[meta] schema_version` field, versioned
  independently from config.toml.

### 4.5 Secrets

Secrets are loaded exclusively from `.env` via the `dotenvy` crate:

| Secret | .env variable | Used by |
| :----- | :------------ | :------ |
| GitHub PAT | `GH_PAT` | scrapper (VCS API auth, clone auth) |
| GitHub user | `GH_USER` | scrapper |
| Registry username | `REGISTRY_USER` | push |
| Registry password | `REGISTRY_PASSWORD` | push |
| Telegram bot token | `TELEGRAM_BOT_TOKEN` | notifier |
| Telegram chat ID | `TELEGRAM_CHAT_ID` | notifier |
| gRPC auth token | `GRPC_AUTH_TOKEN` | grpc (token-based auth) |

The `secrecy` crate wraps secret values to prevent accidental logging.
`.env` is declared in `.gitignore`.

---

## 5. Recipe System

### 5.1 Recipe TOML Structure

A recipe is a TOML file in the `recipes/` directory. Each file describes how
to build a specific blockchain binary. The structure is derived from the
existing recipe files (cosmos-gaiad.toml, kyve-kyved.toml):

```
[meta]                     -- Schema version, min Dockermint version
[header]                   -- Chain name, repo URL, type, binary name, glob filters
[flavours.available]       -- All valid values for each flavor dimension
[flavours.default]         -- Default value for each flavor dimension
[scrapper]                 -- Builder image, install commands, env vars, clone method
[variables]                -- Build-time variables (shell commands for dynamic values)
[profiles.<dimension>.<value>]  -- Profile-specific variable overrides (e.g., network)
[builder.install]          -- OS-specific build dependency install commands
[[pre_build]]              -- Conditional pre-build Dockerfile instructions
[build.env]                -- Build environment variables
[build.linker.flags]       -- Linker flag sets keyed by flavor value (dynamic/static)
[build.linker.variables]   -- Go linker -X variables for version injection
[build.path]               -- Go build target path
[user.dockermint]          -- User config for non-root execution
[copy]                     -- Files to copy from builder to runner stage
[copy.<flavor_value>]      -- Conditional copy rules keyed by flavor value
[expose]                   -- Default exposed ports
[labels]                   -- OCI image labels
[image]                    -- Image tag template
```

### 5.2 Flavor System

#### Flavor Dimensions

A flavor dimension is a named axis of variation. Each dimension has a set of
available values and a default. Standard dimensions observed in existing recipes:

| Dimension | Example values | Purpose |
| :-------- | :------------- | :------ |
| `architecture` | `x86_64`, `aarch64` | Target CPU architecture |
| `db_backend` | `goleveldb`, `pebbledb` | Database engine compiled into the binary |
| `binary_type` | `dynamic`, `static` | Linking strategy |
| `running_env` | `alpine3.23`, `bookworm`, `distroless` | Runner stage base image |
| `running_user` | `root`, `custom`, `dockermint` | User the container runs as |
| `build_tags` | `netgo`, `ledger`, `muslc` | Go build tags (array type) |
| `network` | `mainnet`, `kaon` | Chain network (recipe-specific dimension) |

Dimensions are not hardcoded in Rust. Recipes define their own dimensions in
`[flavours.available]`. Dockermint validates that selected values exist in the
available set but does not need to know the semantic meaning of each dimension.

#### Flavor Resolution Order

```
1. CLI argument: --db-backend pebbledb
2. config.toml per-recipe override: [flavours.recipes."cosmos-gaiad"] db_backend
3. config.toml global override: [flavours] db_backend
4. Recipe default: [flavours.default] db_backend
```

If the resolved value is not in `[flavours.available]` for that dimension, the
build fails following the Unrecoverable Error Strategy.

#### Flavor Compatibility

Some flavor combinations are incompatible. Compatibility rules are expressed
in the recipe TOML itself via a `[flavours.incompatible]` section. Rust code
reads and enforces the rules but does not define them. This preserves the core
principle: adding or modifying compatibility constraints requires no Rust code
changes.

```toml
# Example: flavor compatibility rules in recipe TOML
[[flavours.incompatible]]
rule = "static requires muslc build tag"
when = { binary_type = "static" }
requires = { build_tags = ["muslc"] }

[[flavours.incompatible]]
rule = "distroless requires static binary"
when = { running_env = "distroless" }
requires = { binary_type = "static" }

[[flavours.incompatible]]
rule = "pebbledb not available on aarch64"
when = { db_backend = "pebbledb", architecture = "aarch64" }
action = "deny"
```

Each rule is an entry in the `[[flavours.incompatible]]` array of tables.
The `rule` field is a human-readable description. The `when` field specifies
the triggering condition (matching flavor values). The `requires` field
specifies additional constraints that must hold when the condition matches.
The `action = "deny"` variant flatly prohibits the combination. Dockermint
validates these rules after flavor resolution and reports violations following
the Unrecoverable Error Strategy.

### 5.3 Variable System

Recipes define two types of variables, distinguished by case:

| Syntax | Source | Resolution time | Examples |
| :----- | :----- | :-------------- | :------- |
| `{{UPPERCASE}}` | Host environment / Dockermint internals | Before Dockerfile generation | `{{HOST_ARCH}}`, `{{GH_PAT}}`, `{{CREATION_TIMESTAMP}}`, `{{SEMVER_TAG}}`, `{{BUILD_TAGS_COMMA_SEP}}` |
| `{{lowercase}}` | Recipe `[variables]` section or resolved flavors | During Dockerfile generation | `{{repo_commit}}`, `{{golang_version}}`, `{{db_backend}}`, `{{binary_name}}` |

Shell-type variables (`{ shell = "..." }`) are executed inside the builder
container to capture dynamic values (e.g., Go module versions).

### 5.4 Recipe Discovery

Recipes are discovered by scanning the configured `recipes_dir` for files
matching `*.toml`. No registration step is needed. Adding a new chain is:

1. Create a new `.toml` file in `recipes/`.
2. Follow the recipe schema.
3. Restart or trigger a new build.

No Rust code modification required. This is the fundamental design principle.

### 5.5 Recipe Type Dispatch

The `[header] type` field determines which builder submodule handles the build.
Two recipe types are supported:

- `"golang"` -- routed to `builder/go` (Go-based Cosmos SDK chains)
- `"rust"` -- routed to `builder/rust` (Rust-based blockchain projects)

Adding a new build system type (e.g., C++) would require:

1. A new submodule under `builder/` implementing the type-specific build logic.
2. A match arm in the builder dispatch logic.

This is the only case where adding recipe support requires Rust code changes,
and it only applies when introducing an entirely new build system type (not a
new chain).

---

## 6. Error Strategy

### 6.1 Error Type Hierarchy

```
Application Level (anyhow::Error)
  |
  +-- wraps module-level errors via .context()
  |
Module Level (thiserror)
  |
  +-- ConfigError        (config module)
  +-- RecipeError         (recipe module)
  +-- VcsError            (scrapper module)
  +-- BuilderError        (builder module)
  +-- TemplateError       (builder/template engine)
  +-- RegistryError       (push module)
  +-- StoreError          (saver module)
  +-- NotifierError       (notifier module)
  +-- MetricsError        (metrics module)
  +-- CheckerError        (checker module)
  +-- CommandError         (commands module)
```

Each module owns its error type. Module errors use `thiserror` for structured,
displayable errors with source chaining. Application-level code (CLI main,
daemon loop, RPC handler) wraps module errors with `anyhow` to add context
(which recipe, which version, which stage of the pipeline).

### 6.2 Error Propagation Pattern

```
Module function
  -> returns Result<T, ModuleError>        (thiserror)

Pipeline orchestrator
  -> maps ModuleError into anyhow::Error   (via .context("building cosmos-gaiad v19.0.0"))
  -> propagates with ?

Mode-specific handler (CLI main / daemon loop / RPC handler)
  -> catches anyhow::Error
  -> applies mode-specific error strategy
```

### 6.3 Unrecoverable Error Strategy by Mode

#### CLI Mode

```
1. Log the full error chain via tracing::error!
2. Print a user-friendly summary to stderr
3. Exit with category-specific exit code
```

Exit codes are mapped per error category to support scripting and CI
integration:

| Exit code | Category |
| :-------- | :------- |
| 0 | Success |
| 1 | General / unknown error |
| 2 | Configuration error (config.toml, .env, CLI args) |
| 3 | Recipe error (parse failure, invalid flavor, compatibility violation) |
| 4 | System check failure (Docker not found, BuildKit unavailable, disk) |
| 5 | VCS error (GitHub API failure, auth, rate limit) |
| 6 | Build error (Dockerfile generation, BuildKit execution) |
| 7 | Push error (registry auth, upload failure) |
| 8 | Notification error (notifier failure -- non-fatal in CLI, logged only) |

#### Daemon Mode

```
1. Log the full error chain via tracing::error!
2. Send notification via Notifier trait (notify_failure or notify_error)
3. Persist failure record via BuildStore trait (save_failure)
4. Continue to next recipe / next poll cycle
```

The daemon never exits on a per-build failure. It only exits on startup
failures (config load, system check, database initialization).

#### RPC Mode

```
1. Log the full error chain via tracing::error!
2. Return gRPC error status to the calling CLI client
3. Return to idle state, ready for next request
```

### 6.4 Error Context Enrichment

Every error propagated through the pipeline should carry:

- Which recipe (name)
- Which version (tag)
- Which pipeline stage (config, parse, fetch, template, build, push)
- Timestamp

This context is attached via `anyhow::Context` at the pipeline orchestration
layer, not within individual modules.

---

## 7. Binary Architecture

### 7.1 Crate Structure

Dockermint is a single Cargo package (not a workspace) producing three
artifacts from one library crate and two binary targets:

```
Cargo.toml
src/
  lib.rs                -- Library crate: all modules, traits, types
  bin/
    cli.rs              -- Binary: dockermint-cli
    daemon.rs           -- Binary: dockermint-daemon
```

```toml
# Cargo.toml crate-type declaration (conceptual)
[lib]
name = "dockermint"
crate-type = ["rlib", "cdylib", "staticlib"]

[[bin]]
name = "dockermint-cli"
path = "src/bin/cli.rs"

[[bin]]
name = "dockermint-daemon"
path = "src/bin/daemon.rs"
```

### 7.2 Three Artifacts

| Artifact | Type | Purpose |
| :------- | :--- | :------ |
| `dockermint-cli` | Binary (ELF / Mach-O) | One-shot CLI builds, local or via gRPC to remote daemon |
| `dockermint-daemon` | Binary (ELF / Mach-O) | Long-running daemon with polling, optional gRPC server |
| `dockermint` (lib) | rlib + cdylib + staticlib | Rust crate for embedding in other Rust projects, plus C-compatible shared/static library for FFI consumers |

The `rlib` crate-type enables `use dockermint::*` from other Rust code. The
`cdylib` and `staticlib` crate-types produce C-compatible dynamic and static
libraries respectively, allowing non-Rust consumers to link against Dockermint.

The library crate (`dockermint`) contains all shared logic: traits, module
implementations, pipeline orchestration, config loading, recipe parsing.

The two binaries are thin entrypoints that:
1. Parse CLI arguments (clap)
2. Load configuration
3. Initialize logging
4. Wire together the concrete trait implementations (selected by features)
5. Run the mode-specific loop (one-shot or polling)

### 7.3 Binary: dockermint-cli

- One-shot execution: parse args, load config, run pipeline, exit.
- Subcommands via clap (e.g., `dockermint-cli build`, `dockermint-cli list`).
- Progress display via `indicatif`.
- Exit codes mapped per error category (see Section 6.3).
- In Phase 3, gains ability to connect to a remote daemon via gRPC instead of
  building locally.

### 7.4 Binary: dockermint-daemon

- Long-running process with a polling loop.
- On each tick: query VCS for new releases across all configured recipes,
  trigger builds for new versions.
- Poll interval: global default from `[daemon] poll_interval_secs`, overridable
  per-chain via `[daemon.chains."<name>"] poll_interval_secs`.
- Requires database (saver), notifier, and metrics features.
- In Phase 3, optionally starts a gRPC server (controlled by `[grpc] enabled`
  in config.toml) to accept remote build requests from CLI clients.

### 7.5 Library: dockermint (C-lib / Rust crate)

- The `rlib` output allows other Rust projects to depend on `dockermint` as a
  library crate, accessing all public traits and types.
- The `cdylib` output produces a `.so` / `.dylib` for dynamic linking from C,
  Python, Go, or any FFI-capable language.
- The `staticlib` output produces a `.a` for static linking from C or other
  native code.
- FFI surface: TBD -- requires CEO decision on which functions to expose via
  `extern "C"` interface. For now, the library targets Rust consumers. C FFI
  will be designed when needed.

### 7.6 Why Not a Workspace?

A single package with multiple targets is preferred because:
- All modules share the same dependency versions without coordination.
- Feature gates apply uniformly across all three artifacts.
- Simpler CI: one `cargo build`, one `cargo test`.
- The three artifacts are tightly coupled (same traits, same feature gates).
- Workspace separation adds coordination cost without benefit at this scale.

---

## 8. Cross-Compilation

### 8.1 Mandatory Toolchains

Dockermint itself must compile on all five toolchains:

| Target | OS | Arch | Libc | Notes |
| :----- | :- | :--- | :--- | :---- |
| `x86_64-unknown-linux-gnu` | Linux | x86_64 | glibc | Primary development target |
| `x86_64-unknown-linux-musl` | Linux | x86_64 | musl | Static binary for Alpine containers |
| `aarch64-unknown-linux-gnu` | Linux | aarch64 | glibc | ARM64 servers |
| `aarch64-unknown-linux-musl` | Linux | aarch64 | musl | Static ARM64 binary |
| `aarch64-apple-darwin` | macOS | aarch64 | -- | Apple Silicon development |

### 8.2 Cross-Compilation Constraints

Dependencies must be evaluated against all five targets:

- **No `*-sys` crates** without explicit verification that they cross-compile
  on all targets (especially musl and aarch64).
- **OpenSSL vendored** (`openssl` crate with `vendored` feature) is the default
  SSL strategy because it handles cross-compilation by building OpenSSL from
  source for each target.
- **rustls** alternative eliminates the C dependency entirely, simplifying musl
  builds. This is the motivation for the `ssl-rustls` feature gate.
- **redb** is pure Rust with no C dependencies -- ideal for cross-compilation.
- **Avoid platform-specific APIs** in core logic. Any platform-specific code
  must be gated with `#[cfg(target_os = "...")]` or `#[cfg(target_arch = "...")]`.

### 8.3 Docker Image Cross-Compilation (BuildKit)

Dockermint itself cross-compiles *and* it produces cross-compiled Docker images
for the chains it builds. These are separate concerns:

1. **Dockermint binary cross-compilation**: handled by Rust toolchains and CI
   build matrix. Produces Dockermint binaries for each target.

2. **Chain image cross-compilation**: handled by BuildKit with per-platform
   builder instances. Dockermint creates and manages:

```
dockermint-amd64    -- BuildKit builder for linux/amd64 images
dockermint-arm64    -- BuildKit builder for linux/arm64 images
```

These builders are Docker buildx builder instances. Dockermint creates them
if they do not exist, uses them for builds targeting the respective platform,
and can optionally remove them after builds complete.

Multi-arch images are produced by building each platform independently and
then creating a manifest list that references all platform-specific images.

### 8.4 CI Build Matrix

The CI pipeline (managed by @devops) should produce:

| Artifact | Targets | Format |
| :------- | :------ | :----- |
| `dockermint-cli` | All 5 toolchains | Static binary (musl) or dynamic (gnu/darwin) |
| `dockermint-daemon` | All 5 toolchains | Static binary (musl) or dynamic (gnu/darwin) |
| `libdockermint.so` / `.dylib` | All 5 toolchains | cdylib (C-compatible shared library) |
| `libdockermint.a` | All 5 toolchains | staticlib (C-compatible static library) |
| Docker images | `linux/amd64`, `linux/arm64` | Multi-arch OCI images containing Dockermint |

---

## Appendix A: Module Dependency Graph

```
dockermint (lib -- rlib + cdylib + staticlib)
  +-- config
  +-- logger
  +-- checker
  +-- commands
  +-- recipe
  +-- scrapper [VcsClient trait]
  +-- builder [ImageBuilder trait]
  |     +-- builder/go
  |     +-- builder/rust
  +-- push [RegistryClient trait]
  +-- saver [BuildStore trait]
  +-- notifier [Notifier trait]
  +-- metrics [MetricsExporter trait]

cli (bin) -- thin entrypoint
  +-- dockermint (lib)    -- uses config, logger, checker, recipe, scrapper,
                             builder, push, commands

daemon (bin) -- thin entrypoint
  +-- dockermint (lib)    -- uses everything from cli plus saver, notifier,
                             metrics, and optionally gRPC server
```

No circular dependencies. Modules depend downward only. Cross-cutting
concerns (logger, commands) are used by all modules but depend on nothing
within Dockermint. The library crate contains all module implementations;
binaries are thin wiring layers.

---

## Appendix B: Open Questions

These items require CEO decisions before the relevant per-module specs can be
finalized:

| ID | Question | Affects |
| :- | :------- | :------ |
| Q1 | Should the daemon poll interval be configurable per-recipe or only globally? | config, daemon loop | **Decided**: Poll interval defined globally in `[daemon]`, overridable per-chain in `[daemon.chains."<name>"]`. Per-chain value overrides global. |
| Q2 | What authentication mechanism for gRPC? Token-based, mTLS, or both? | grpc module, config | **Decided**: gRPC supports BOTH mTLS and token-based authentication. |
| Q3 | Should remote BuildKit endpoint be configurable in config.toml, or rely on Docker context? | builder config | **Decided**: BuildKit endpoint configurable in `[builder]` section of config.toml. On each launch, CLI and Daemon verify Docker context and create/destroy builders on the specified Docker socket URI. |
| Q4 | Should flavor compatibility rules be expressed in recipe TOML or in Rust validation logic? | recipe module | **Decided**: Flavor compatibility rules expressed in recipe TOML via `[flavours.incompatible]` section. Rust code reads and enforces the rules but does not define them. |
| Q5 | Should CLI exit codes be mapped to specific error categories, or is a single non-zero code sufficient? | cli error handling | **Decided**: CLI exit codes mapped per error category (distinct codes for config, recipe, build, push, system-check errors, etc.). |
| Q6 | Should notifier configuration (beyond secrets) live in config.toml or entirely in .env? | notifier config | **Decided**: Secrets (tokens, passwords) ONLY in `.env`. Non-secret notifier config (enabled flag, format preferences) in `[notifier]` section of config.toml. |
| Q7 | Should this remain a single Cargo package or become a workspace as the project grows? | Cargo.toml, CI | **Decided**: Single repo producing 3 artifacts: `dockermint-cli` (CLI binary), `dockermint-daemon` (daemon binary), `dockermint` (C-lib / usable as Rust crate with cdylib + staticlib + rlib crate-types). NOT a workspace -- single Cargo package with lib + 2 bin targets + cdylib/staticlib crate-type. |

---

## Appendix C: Conventions

These conventions apply to all per-module specs and implementations:

| Convention | Rule |
| :--------- | :--- |
| Module error types | Each module defines its own error enum via `thiserror` in `<module>/error.rs` |
| Application errors | Binary entrypoints use `anyhow::Result` with `.context()` |
| Async runtime | `tokio` for all async code |
| CPU-bound parallelism | `rayon` when beneficial |
| HTTP client | `reqwest` (shared by VCS, registry, notifier) |
| Serialization | `serde` + `toml` for config/recipes, `serde` + `serde_json` for API responses |
| Logging | `tracing` crate with structured fields |
| Secrets | `secrecy` crate wrapping, `dotenvy` for loading |
| Progress bars | `indicatif` (CLI mode only) |
| Config structs | `serde::Deserialize` with `#[serde(deny_unknown_fields)]` |
| Trait objects vs generics | Prefer generics (monomorphization) for the default path. Use `dyn Trait` only if runtime dispatch is required |
