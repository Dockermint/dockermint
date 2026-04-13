# Feature: CLI, Daemon, Library, and Commands (cli + commands)

## Context

This spec covers the four entry points into Dockermint and the shared command
execution layer:

1. **dockermint-cli** -- one-shot CLI binary for local or remote builds
2. **dockermint-daemon** -- long-running daemon binary with polling and
   optional gRPC server
3. **dockermint** -- library crate (rlib in Phase 1; cdylib/staticlib
   deferred to Phase 4)
4. **commands module** -- shared command pattern used by all modes

These are not swappable modules behind feature gates. They are structural
components of the project's binary architecture.

Roadmap entry: Phase 0 -- Foundation, spec "CLI architecture and subcommand
design"
Architecture reference: `docs/specs/architecture-overview.md`, sections 1.3
(mode-specific behavior), 6.3 (error strategy by mode), 7 (binary
architecture)

---

## Requirements

1. [confirmed] Three artifacts in Phase 1: `dockermint-cli` (CLI binary),
   `dockermint-daemon` (daemon binary), `dockermint` (library crate, rlib
   only). C-FFI (cdylib/staticlib) deferred to Phase 4.
2. [confirmed] CLI uses Clap for subcommand parsing.
3. [confirmed] CLI exit codes are mapped per error category.
4. [confirmed] CLI args override config.toml values (priority chain from
   architecture-overview.md section 4.1).
5. [confirmed] Daemon poll interval: global default in config.toml, per-chain
   override in config.toml.
6. [confirmed] gRPC supports BOTH mTLS and token-based auth.
7. [confirmed] Secrets ONLY in .env. Non-secret config in config.toml.
8. [confirmed] Unrecoverable errors: CLI = dump/log/exit, Daemon =
   dump/log/notify/persist/continue, RPC = dump/log/return idle.
9. [confirmed] CLI mode uses `indicatif` for progress display.
10. [confirmed] Single Cargo package (not workspace) per
    architecture-overview.md section 7.4.

---

## Architecture

### Module placement

```
src/
    lib.rs                  -- Library crate: re-exports all public modules and traits
    bin/
        cli.rs              -- Binary: dockermint-cli
        daemon.rs           -- Binary: dockermint-daemon
    cli/
        mod.rs              -- Clap app definition, argument parsing
        args.rs             -- Subcommand and argument structs
        exit_codes.rs       -- Exit code constants and mapping
        progress.rs         -- indicatif wrapper for build progress display
    commands/
        mod.rs              -- Command trait + command implementations
        build.rs            -- BuildCommand: orchestrates the full build pipeline
        list_recipes.rs     -- ListRecipesCommand: discover and display recipes
        list_flavors.rs     -- ListFlavorsCommand: display available flavors for a recipe
    daemon/
        mod.rs              -- Daemon orchestration: startup, polling loop, shutdown
        poll.rs             -- Poll cycle logic: check VCS, dispatch builds
        scheduler.rs        -- Per-chain poll interval scheduling
```

Note: the `daemon/` module is a new module not previously listed in
architecture-overview.md. It contains daemon-specific orchestration logic
that does not belong in `cli/` (which is about argument parsing) or
`commands/` (which is about individual operations). The daemon module
depends on `commands` to execute builds, `saver` to persist results,
`notifier` to send alerts, and `metrics` to record events.

### Binary targets

#### Cargo.toml binary declarations (conceptual)

```toml
[lib]
name = "dockermint"
crate-type = ["lib"]              # Phase 1: rlib only
# Phase 4: add "cdylib", "staticlib" for C-FFI surface

[[bin]]
name = "dockermint-cli"
path = "src/bin/cli.rs"

[[bin]]
name = "dockermint-daemon"
path = "src/bin/daemon.rs"
```

---

## 1. CLI Binary (dockermint-cli)

### 1.1 Clap subcommand structure

```
dockermint-cli
    build           -- Execute a build for one or more recipes
    list-recipes    -- List discovered recipes
    list-flavors    -- List available flavors for a recipe
    version         -- Print version and build info
```

### 1.2 Subcommand: build

```
dockermint-cli build [OPTIONS]

Required:
    --recipe <NAME>         Recipe name (e.g., "cosmos-gaiad") or "all"

Optional:
    --version <TAG>         Specific version to build (default: latest release)
    --flavor <KEY=VALUE>    Override a flavor dimension (repeatable)
                            e.g., --flavor db_backend=pebbledb
                                  --flavor running_env=distroless
    --push                  Push to registry after build (default: local only)
    --force                 Force rebuild even if tag already exists in registry
    --config <PATH>         Path to config.toml (default: ./config.toml)
    --recipes-dir <PATH>    Path to recipes directory (overrides config)
    --log-level <LEVEL>     Log level: trace, debug, info, warn, error
    --platform <PLATFORM>   Target platform (repeatable, default: all)
                            "all" builds both linux/amd64 and linux/arm64.
                            Specify a single platform to narrow:
                            e.g., --platform linux/amd64
    --dry-run               Generate Dockerfile but do not build
    --keep-builders         Keep BuildKit builder instances after build completes
                            (CLI default: destroy builders at end)
```

#### Builder lifecycle (CLI mode)

CLI default behavior: destroy BuildKit builder instances at the end of the
build. The `--keep-builders` flag overrides this to leave builders running
(useful for iterative development). There is no `--destroy-builders` flag in
CLI mode because destruction is already the default.

#### --recipe all (best practices)

When `--recipe all` is specified, Dockermint builds all discovered recipes
in the recipes directory sequentially. Each recipe uses its resolved flavors
(CLI args > config.toml > defaults). If `--version` is not specified, the
latest release for each recipe is used. If one recipe build fails, the
remaining recipes are still attempted (fail-forward). A summary of
successes and failures is printed at the end. Exit code is the highest
severity code among all builds (0 if all succeed).

#### --dry-run output (best practices)

The `--dry-run` flag generates the Dockerfile and outputs it to stdout.
This allows piping to a file (`> Dockerfile`) or inspection. Additionally,
a summary of resolved variables (template engine substitutions) is printed
to stderr for debugging, so it does not interfere with the Dockerfile
content on stdout.

#### Argument precedence

Per architecture-overview.md section 4.1:

```
CLI args > .env secrets > config.toml > recipe defaults > hardcoded defaults
```

For flavors specifically:

```
--flavor KEY=VALUE > config.toml [flavours.recipes."NAME"] > config.toml [flavours] > recipe [flavours.default]
```

The `--flavor` flag is repeatable and parsed into a `BTreeMap<String, String>`.
If a flavor key appears multiple times, the last occurrence wins.

### 1.3 Subcommand: list-recipes

```
dockermint-cli list-recipes [OPTIONS]

Optional:
    --recipes-dir <PATH>    Path to recipes directory
    --config <PATH>         Path to config.toml
    --format <FORMAT>       Output format: table (default), json
```

Discovers all `*.toml` files in the recipes directory, parses their
`[header]` section, and displays: name, binary name, repo URL, type.

### 1.4 Subcommand: list-flavors

```
dockermint-cli list-flavors [OPTIONS]

Required:
    --recipe <NAME>         Recipe name

Optional:
    --recipes-dir <PATH>    Path to recipes directory
    --config <PATH>         Path to config.toml
    --format <FORMAT>       Output format: table (default), json
```

Parses the specified recipe's `[flavours.available]` and
`[flavours.default]` sections. Displays each dimension, available values,
and the resolved default (after applying config.toml overrides).

### 1.5 Subcommand: version

```
dockermint-cli version
```

Prints: binary name, version (from Cargo.toml), git commit hash (if
available), build target, feature flags compiled in.

### 1.6 Exit code mapping

Exit codes are distinct per error category so that scripts and CI can react
programmatically.

| Code | Category | Description |
| :--- | :------- | :---------- |
| 0 | Success | Operation completed successfully |
| 1 | General | Unclassified error |
| 2 | Config | Configuration file missing, invalid, or version mismatch |
| 3 | Recipe | Recipe file missing, invalid, or incompatible flavors |
| 4 | System | System prerequisites not met (Docker, BuildKit, disk, etc.) |
| 5 | VCS | VCS API error (auth, rate limit, network, version not found) |
| 6 | Build | Docker/BuildKit build failed |
| 7 | Push | Registry push failed (auth, network, manifest) |
| 8 | Store | Database error (save, read, schema mismatch) |
| 9 | Notify | Notification send failed (non-fatal in daemon, informational in CLI) |
| 10 | Internal | Bug in Dockermint (unreachable code, invariant violation) |

Exit code mapping is implemented in `cli/exit_codes.rs` as a function that
maps `anyhow::Error` (which wraps module-specific `thiserror` types) to the
appropriate code. The mapping inspects the error chain using `downcast_ref`.

### 1.7 Progress display

CLI mode uses `indicatif` for long-running operations (per CLAUDE.md).
Progress is displayed for:

- Recipe discovery and parsing
- VCS tag/release fetching
- Template resolution
- Dockerfile generation
- BuildKit build execution (per-platform)
- Registry push (per-platform)

Each stage gets a spinner with a contextually descriptive message. Build
execution (the longest stage) uses a progress bar if BuildKit provides
progress output, otherwise a spinner.

Progress bars are managed by a `ProgressTracker` wrapper in `cli/progress.rs`
that creates and manages `indicatif::MultiProgress` for parallel platform
builds.

Progress display is CLI-only. Daemon mode uses structured logging (`tracing`)
instead.

### 1.8 CLI binary entrypoint (src/bin/cli.rs)

Pseudocode for the CLI binary's main function:

```
fn main():
    1. Parse CLI args via Clap
    2. Load .env secrets via dotenvy
    3. Load config.toml, merge with CLI args
    4. Initialize logger (tracing + rotation)
    5. Initialize progress tracker (indicatif)
    6. Match on subcommand:
        build:
            a. Run system checker
            b. Parse recipe(s) (single or all)
            c. Resolve flavors (CLI args > config > defaults)
            d. Create BuildCommand with resolved config (incl. --force, --keep-builders)
            e. If --dry-run: generate Dockerfile to stdout, variables to stderr, exit 0
            f. Execute BuildCommand (for each recipe if --recipe all)
            g. On success: exit 0 (or highest severity if --recipe all)
            h. On error: map to exit code, log, exit
        list-recipes:
            a. Run ListRecipesCommand
            b. Display output
        list-flavors:
            a. Run ListFlavorsCommand
            b. Display output
        version:
            a. Print version info
```

---

## 2. Daemon Binary (dockermint-daemon)

### 2.1 Startup sequence

#### Daemon CLI args

```
dockermint-daemon [OPTIONS]

Optional:
    --config <PATH>         Path to config.toml (default: ./config.toml)
    --log-level <LEVEL>     Log level: trace, debug, info, warn, error
    --retry-failed          Retry previously failed builds (default: skip them)
    --keep-builders         Keep BuildKit builder instances between builds
                            (daemon default: keep builders)
    --destroy-builders      Destroy BuildKit builder instances after each build
```

Builder lifecycle (daemon mode): daemon default behavior is to keep BuildKit
builder instances running between builds (opposite of CLI default). The
`--destroy-builders` flag overrides this to destroy after each build. The
two flags are mutually exclusive.

The `--retry-failed` flag causes the daemon to re-attempt builds for versions
that previously had a `Failure` status in the saver. Without this flag, failed
versions are skipped on subsequent poll cycles.

#### Startup sequence

```
fn main():
    1. Parse daemon CLI args
    2. Load .env secrets via dotenvy
    3. Load config.toml
    4. Initialize logger (tracing + rotation)
    5. Initialize tokio runtime
    6. Run system checker (prerequisites)
    7. Initialize BuildStore (saver module)
       - Open or create database
       - Run schema migration if needed
       - If fails: exit (startup failure)
    8. Initialize Notifier (notifier module)
       - Validate secrets available
       - Spawn background send task
       - If fails: exit (startup failure)
    9. Initialize MetricsExporter (metrics module)
       - Register all metrics
       - Start metrics HTTP server
       - If fails: exit (startup failure)
    10. If grpc.enabled: initialize gRPC server (Phase 3)
    11. Parse all recipes from recipes_dir
    12. Set metrics.set_recipes_configured(count)
    13. Register signal handlers (SIGTERM, SIGINT for shutdown; SIGHUP for config reload)
    14. Enter polling loop
```

Startup failures (steps 7-9) are fatal: the daemon logs the error and exits.
This is distinct from per-build failures, which are handled by the daemon
error strategy (log, notify, persist, continue).

### 2.2 Polling loop architecture

```
loop:
    poll_start = now()

    for each recipe in configured_recipes:
        if shutdown_signal_received: break

        1. Determine poll interval for this recipe:
           - Per-chain override if present in config
           - Otherwise global poll_interval_secs
        2. Check if this recipe is due for polling (last_poll + interval < now)
        3. If not due: skip
        4. Fetch new releases from VCS (scrapper)
        5. For each version to process:
           - New versions not in BuildStore (always)
           - Previously failed versions (only if --retry-failed flag is active)
           - Skip versions with Success or InProgress status
            a. metrics.record_build_start(chain)
            b. notifier.notify_start(event)
            c. build_start = now()
            d. Execute BuildCommand
            e. On success:
                - saver.save_build(record with status=Success)
                - notifier.notify_success(event)
                - metrics.record_build_success(chain, duration)
            f. On failure:
                - saver.save_build(record with status=Failure)
                - saver.save_failure(failure_record)
                - notifier.notify_failure(event)
                - metrics.record_build_failure(chain, stage, duration)
                - Continue to next version/recipe (DO NOT exit)
        6. Update last_poll_time for this recipe

    poll_duration = now() - poll_start
    metrics.record_poll_complete(poll_duration)

    if shutdown_signal_received: break
    sleep(min_remaining_interval)
```

The loop is async (`tokio`). Build execution may involve
`tokio::task::spawn_blocking` for CPU-bound BuildKit operations (per
CLAUDE.md's axum guidelines about offloading CPU-bound work).

### 2.3 Poll interval: global default + per-chain override

Per CEO decision, the poll interval is configurable at two levels:

```toml
[daemon]
poll_interval_secs = 300                  # Global default: 5 minutes

[daemon.chains."cosmos-gaiad"]
poll_interval_secs = 600                  # Override for this chain: 10 minutes

[daemon.chains."kyve-kyved"]
poll_interval_secs = 120                  # Override for this chain: 2 minutes
```

Resolution:

```
Per-chain override > global default > hardcoded default (300)
```

The scheduler (`daemon/scheduler.rs`) tracks the last poll time per recipe
and computes the next poll time based on the resolved interval.

#### DaemonConfig update

The architecture-overview.md section 4.2 showed a minimal `[daemon]` section.
With the CEO's per-chain decision, the config struct expands:

```
DaemonConfig
    poll_interval_secs: u64                             -- Global default
    chains: Option<BTreeMap<String, ChainDaemonConfig>> -- Per-chain overrides

ChainDaemonConfig
    poll_interval_secs: Option<u64>                     -- Override for this chain
```

Note: the `--retry-failed`, `--keep-builders`, and `--destroy-builders` flags
are daemon CLI args (not config.toml values). They are parsed by Clap and
passed into the daemon's runtime state, not into `DaemonConfig`.

### 2.4 gRPC server mode (Phase 3 -- design now, implement later)

When `[grpc] enabled = true` in config.toml, the daemon additionally starts
a gRPC server that accepts build requests from remote CLI clients.

Per CEO decision, the gRPC server supports BOTH mTLS and token-based auth:

```toml
[grpc]
enabled = true
listen_address = "0.0.0.0:50051"
auth_mode = "both"                  # "mtls" | "token" | "both"
tls_cert = "/etc/dockermint/server.crt"
tls_key = "/etc/dockermint/server.key"
tls_ca = "/etc/dockermint/ca.crt"   # CA for client cert verification (mTLS)
```

Secret for token auth:

```
.env: GRPC_AUTH_TOKEN=<token>
```

Non-secret TLS file paths live in config.toml. The actual TLS private key
file is on disk (not in .env), but its path is non-secret config.

Per CEO decision (follow best practices): the gRPC server supports multiple
auth tokens for multi-client environments. Tokens are stored in `.env` as a
comma-separated list:

```
GRPC_AUTH_TOKENS=token1,token2,token3
```

This allows per-client tokens for auditability (logs which token was used)
and revocation (remove a single client's token without affecting others).
Each token is wrapped with `secrecy::SecretString`.

#### gRPC service definition (Phase 3 -- proto sketch)

```
service Dockermint {
    rpc Build (BuildRequest) returns (BuildResponse);
    rpc ListRecipes (Empty) returns (RecipeList);
    rpc ListBuilds (BuildQueryRequest) returns (BuildList);
    rpc GetBuildStatus (BuildId) returns (BuildStatus);
}
```

This is a Phase 3 deliverable. The spec is included here for architectural
completeness so the daemon's module structure accounts for it.

### 2.5 Shutdown

The daemon listens for SIGTERM, SIGINT, and SIGHUP. On SIGTERM or SIGINT:

1. Set shutdown flag (atomic bool or `tokio::sync::watch`).
2. Stop accepting new poll cycles.
3. Stop immediately -- do NOT wait for in-progress builds to complete.
   No shutdown timeout. Per CEO decision, just stop.
4. Close database (redb flush best-effort).
5. Stop metrics server.
6. Stop gRPC server (if running).
7. Exit with code 0.

In-progress builds are abandoned. The saver marks them as `InProgress` --
on next daemon start, these stale records can be detected and cleaned up
(status remains `InProgress` with no `finished_at`).

### 2.6 SIGHUP config reload

Per CEO decision, the daemon supports SIGHUP for live config reload without
restart:

1. Receive SIGHUP signal.
2. Re-read `config.toml` from disk.
3. Re-read `.env` from disk (secrets may have changed).
4. Validate the new configuration.
5. If valid: apply changes to the running daemon:
   - Update poll intervals (global and per-chain).
   - Update notifier settings (enabled, level, rate limits).
   - Update metrics settings (if applicable without server restart).
   - Update recipe list (re-scan recipes directory).
   - Update flavor overrides.
   - Log the reload event via `tracing::info!`.
6. If invalid: reject the reload, keep running with previous config, log
   the error via `tracing::error!`.

Config reload is atomic: either all changes apply or none do. The daemon
never enters a half-updated state.

Note: some settings cannot be reloaded without restart (e.g., database path,
metrics listen address, gRPC listen address). These are logged as warnings
if they differ from the running config.

---

## 3. Library Crate (dockermint)

### 3.1 Public API surface

The library crate (`src/lib.rs`) re-exports the public API that both binaries
and external consumers use:

```
pub mod config;        -- AppConfig, config loading
pub mod checker;       -- System prerequisites
pub mod recipe;        -- Recipe parsing, flavor resolution
pub mod scrapper;      -- VcsClient trait + default impl
pub mod builder;       -- ImageBuilder trait + default impl, template engine
pub mod push;          -- RegistryClient trait + default impl
pub mod saver;         -- BuildStore trait + default impl
pub mod notifier;      -- Notifier trait + default impl
pub mod metrics;       -- MetricsExporter trait + default impl
pub mod commands;      -- Command pattern for pipeline operations
pub mod logger;        -- Logger initialization
pub mod cli;           -- Clap structures (for consumers that want to embed)
```

### 3.2 C-FFI surface (cdylib / staticlib) -- Phase 4

**Deferred to Phase 4** per CEO decision. The library crate (rlib) is Phase 1
scope, but the C-FFI surface (cdylib/staticlib) is Phase 4.

The architectural sketch is preserved here for completeness so the library
design accounts for future FFI exposure:

```
// Conceptual C-FFI signatures (Phase 4 deliverable):

extern "C" fn dockermint_build(config_path: *const c_char, recipe_name: *const c_char) -> i32;
extern "C" fn dockermint_list_recipes(config_path: *const c_char, out: *mut *mut c_char) -> i32;
extern "C" fn dockermint_free_string(ptr: *mut c_char);
extern "C" fn dockermint_last_error(out: *mut *mut c_char) -> i32;
```

The FFI layer is minimal and delegates immediately to the Rust API. Error
handling uses a thread-local last-error pattern: the function returns a
status code, and the caller retrieves the error message via
`dockermint_last_error`.

Phase 1 action: keep the public Rust API surface FFI-friendly (simple types,
clear ownership) to minimize Phase 4 adaptation cost.

### 3.3 What is public vs internal

| Item | Visibility | Rationale |
| :--- | :--------- | :-------- |
| Trait definitions (BuildStore, Notifier, etc.) | `pub` | External consumers may implement alternative backends |
| Data types (BuildRecord, BuildEvent, etc.) | `pub` | Needed to interact with traits |
| Config structs (AppConfig, MetricsConfig, etc.) | `pub` | External consumers need to configure the library |
| Error types (StoreError, NotifierError, etc.) | `pub` | External consumers need to handle errors |
| Default implementations (RedbStore, TelegramNotifier, etc.) | `pub` | External consumers may use them directly |
| Internal helpers (template parsing internals, etc.) | `pub(crate)` | Implementation details |
| Module-internal types | `pub(super)` or private | Module encapsulation |

---

## 4. Commands Module

### 4.1 Command pattern

The `commands` module implements the Command pattern: each operation is a
struct that encapsulates its parameters and exposes an `execute` method.
Commands are the shared execution layer used by both CLI and daemon modes.

This is not a swappable module (per architecture-overview.md section 2.3).
It is a structural pattern for code reuse.

### 4.2 Command trait

```
trait Command {
    type Output;
    async fn execute(&self) -> Result<Self::Output, anyhow::Error>;
}
```

The trait uses `anyhow::Error` at the application level because commands
orchestrate multiple modules and need to chain diverse error types with
context.

### 4.3 BuildCommand

The primary command. Orchestrates the full build pipeline for a single
recipe + version combination.

```
BuildCommand
    config: AppConfig               -- Merged configuration
    recipe: ParsedRecipe            -- Parsed recipe with resolved flavors
    version: String                 -- Version tag to build
    push: bool                      -- Whether to push to registry
    force: bool                     -- Force rebuild even if tag exists in registry
    keep_builders: bool             -- Keep BuildKit builders after build
    progress: Option<ProgressTracker>  -- CLI progress display (None in daemon)
    store: Option<Arc<dyn BuildStore>> -- Persistence (None in CLI, Some in daemon)
    notifier: Option<Arc<dyn Notifier>> -- Notifications (None in CLI, Some in daemon)
    metrics: Option<Arc<dyn MetricsExporter>> -- Metrics (None in CLI, Some in daemon)
```

The optional fields are `None` in CLI mode and `Some` in daemon/RPC mode.
This allows the same command to operate in both contexts without conditional
compilation.

The `force` flag skips the registry tag existence check and rebuilds
regardless. Without it, builds that already have a matching tag in the
registry are skipped.

The `keep_builders` flag controls whether BuildKit builder instances are
destroyed after the build. CLI default: `false` (destroy). Daemon default:
`true` (keep).

#### BuildCommand::execute pipeline

```
1. Resolve template variables (TemplateEngine)
2. Generate Dockerfile
3. For each target platform:
    a. Create or reuse BuildKit builder instance
    b. Execute build
    c. If push: push image to registry
4. If multi-arch and push: create manifest list
5. Return BuildOutput (image refs, duration, per-platform results)
```

### 4.4 ListRecipesCommand

```
ListRecipesCommand
    recipes_dir: PathBuf
```

Scans the directory, parses `[header]` from each TOML file, returns a list
of recipe summaries.

### 4.5 ListFlavorsCommand

```
ListFlavorsCommand
    recipes_dir: PathBuf
    recipe_name: String
    config_overrides: FlavoursConfig  -- From config.toml
```

Parses the specified recipe, resolves flavor defaults with config overrides,
returns the flavor matrix.

### 4.6 Error propagation per mode

Commands return `anyhow::Error`. The calling mode handles it according to
the Unrecoverable Error Strategy:

```
                     +------------------+
                     | BuildCommand     |
                     | .execute()       |
                     | -> Result<T, E>  |
                     +--------+---------+
                              |
               +--------------+--------------+
               |              |              |
               v              v              v
        [CLI main]     [daemon loop]   [RPC handler]
               |              |              |
               v              v              v
        Map to exit    Log + notify    Log + return
        code, log,     + persist +     gRPC error
        exit           continue        status, idle
```

The command itself does not know which mode it is running in. It propagates
errors via `?`. The calling code (bin/cli.rs or daemon/poll.rs) catches the
error and applies mode-specific handling.

---

## Type design (shared across sections)

### ParsedRecipe

(Defined by the recipe module spec, referenced here for completeness.)

The output of recipe parsing with resolved flavors. Contains all information
needed to execute a build.

### BuildOutput

The return type of `BuildCommand::execute`.

```
BuildOutput
    recipe_name: String
    version: String
    platforms: Vec<PlatformResult>
    total_duration_secs: u64
    image_ref: Option<String>       -- Final manifest list reference (if pushed)

PlatformResult
    platform: String                -- e.g., "linux/amd64"
    image_id: String                -- Local image ID
    duration_secs: u64
    pushed: bool
```

### AppError

(For axum handlers in metrics and gRPC modules.)

Application-level error type that maps to HTTP/gRPC responses. Implements
`axum::response::IntoResponse`.

---

## Configuration updates

This spec introduces configuration changes to the architecture-overview.md
section 4.2 schema:

### Updated config.toml sections

```toml
[daemon]
poll_interval_secs = 300           # Global default poll interval

[daemon.chains."cosmos-gaiad"]     # Per-chain overrides
poll_interval_secs = 600

[daemon.chains."kyve-kyved"]
poll_interval_secs = 120

[grpc]
enabled = false
listen_address = "0.0.0.0:50051"
auth_mode = "both"                 # "mtls" | "token" | "both"
tls_cert = ""                      # Path to server certificate
tls_key = ""                       # Path to server private key
tls_ca = ""                        # Path to CA certificate for client verification
```

### Updated .env variables

| Variable | Used by | Description |
| :------- | :------ | :---------- |
| `GRPC_AUTH_TOKENS` | gRPC server (Phase 3) | Comma-separated list of tokens for token-based gRPC auth (supports multiple clients) |

### Updated AppConfig struct tree

```
AppConfig
    +-- MetaConfig
    +-- GeneralConfig
    +-- DaemonConfig
    |     +-- poll_interval_secs: u64
    |     +-- chains: Option<BTreeMap<String, ChainDaemonConfig>>
    +-- GrpcConfig
    |     +-- enabled: bool
    |     +-- listen_address: SocketAddr
    |     +-- auth_mode: GrpcAuthMode      (enum: Mtls, Token, Both)
    |     +-- tls_cert: PathBuf
    |     +-- tls_key: PathBuf
    |     +-- tls_ca: PathBuf
    +-- BuilderConfig
    +-- RegistryConfig
    +-- NotifierConfig
    +-- MetricsConfig
    +-- SaverConfig                        (NEW -- from saver spec)
    +-- FlavoursConfig
```

---

## Error types

### CliError (cli/exit_codes.rs -- not a thiserror enum)

Not a separate error type. The CLI uses the exit code mapping function to
convert `anyhow::Error` (wrapping module errors) into integer exit codes.

### CommandError (commands/error.rs -- not needed)

Commands return `anyhow::Error` directly because they orchestrate multiple
modules. There is no `CommandError` enum. Module-specific errors are wrapped
with `.context("building cosmos-gaiad v19.0.0 at stage: build")`.

### DaemonError (daemon/error.rs)

```
enum DaemonError {
    /// Startup failed: could not initialize a required module
    Startup { module: String, source: anyhow::Error },

    /// Polling cycle encountered a fatal error (not a per-build failure)
    Poll { source: anyhow::Error },

    /// Shutdown failed
    Shutdown { detail: String },

    /// Signal handler registration failed
    Signal { source: std::io::Error },

    /// Config reload (SIGHUP) failed: invalid config
    Reload { detail: String, source: anyhow::Error },
}
```

`DaemonError::Startup` causes the daemon to exit. `DaemonError::Reload` is
logged but the daemon continues running with the previous config. All other
variants are logged and the daemon attempts recovery or shutdown.

---

## Interface contract

```rust
/// Command pattern: each operation implements this trait.
///
/// Commands are the shared execution layer between CLI and daemon modes.
/// They encapsulate parameters and orchestrate module calls.
pub trait Command {
    /// The output type on success.
    type Output;

    /// Execute the command.
    ///
    /// Returns the output on success, or an anyhow::Error with context
    /// on failure. The caller (CLI main or daemon loop) applies
    /// mode-specific error handling.
    async fn execute(&self) -> Result<Self::Output, anyhow::Error>;
}

// --- CLI exit code mapping ---

/// Map an anyhow::Error to a CLI exit code.
///
/// Inspects the error chain via downcast_ref to find the root module
/// error type and maps it to the corresponding exit code.
///
/// Returns exit code 1 (General) if no specific mapping is found.
pub fn exit_code_from_error(error: &anyhow::Error) -> i32;

// --- Exit code constants ---

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_GENERAL: i32 = 1;
pub const EXIT_CONFIG: i32 = 2;
pub const EXIT_RECIPE: i32 = 3;
pub const EXIT_SYSTEM: i32 = 4;
pub const EXIT_VCS: i32 = 5;
pub const EXIT_BUILD: i32 = 6;
pub const EXIT_PUSH: i32 = 7;
pub const EXIT_STORE: i32 = 8;
pub const EXIT_NOTIFY: i32 = 9;
pub const EXIT_INTERNAL: i32 = 10;
```

---

## Module interaction diagram

```
+-------------------------------+     +-------------------------------+
| src/bin/cli.rs                |     | src/bin/daemon.rs             |
| (dockermint-cli)              |     | (dockermint-daemon)           |
+---------------+---------------+     +---------------+---------------+
                |                                     |
                v                                     v
        +-------+-------+                    +--------+--------+
        | cli/           |                    | daemon/         |
        | (Clap parse)   |                    | (startup,       |
        |                |                    |  poll loop,     |
        +-------+-------+                    |  shutdown)      |
                |                             +--------+--------+
                |                                      |
                +---------------+----------------------+
                                |
                                v
                    +-----------+-----------+
                    | commands/             |
                    | BuildCommand          |
                    | ListRecipesCommand    |
                    | ListFlavorsCommand    |
                    +-----------+-----------+
                                |
                +---------------+---------------+
                |               |               |
                v               v               v
          +---------+    +-----------+    +---------+
          | config  |    | recipe    |    | checker |
          +---------+    +-----------+    +---------+
                |               |
                v               v
          +-----------+   +-----------+
          | scrapper  |   | builder   |
          | (VCS)     |   | (build +  |
          +-----------+   |  template)|
                          +-----------+
                                |
                                v
                          +-----------+
                          | push      |
                          | (registry)|
                          +-----------+
                                |
                +---------------+---------------+
                |               |               |
                v               v               v
          +-----------+   +-----------+   +-----------+
          | saver     |   | notifier  |   | metrics   |
          | (daemon)  |   | (daemon)  |   | (daemon)  |
          +-----------+   +-----------+   +-----------+
```

---

## Testing strategy

### Unit tests

- Clap argument parsing: valid and invalid combinations for each subcommand.
- Flavor override parsing (`--flavor KEY=VALUE` repeated).
- Exit code mapping: each module error type maps to the correct code.
- Exit code mapping: unknown error type maps to EXIT_GENERAL.
- DaemonConfig: per-chain poll interval resolution logic.
- GrpcAuthMode deserialization.
- Scheduler: next-poll-time calculation per recipe.
- BuildCommand: verify optional fields (store, notifier, metrics) are None-safe.

### Integration tests

- CLI binary: `dockermint-cli version` exits with code 0 and prints version.
- CLI binary: `dockermint-cli list-recipes --recipes-dir ./recipes` discovers
  real recipe files.
- CLI binary: `dockermint-cli build --recipe nonexistent` exits with
  EXIT_RECIPE.
- CLI binary: `dockermint-cli build` with missing required args exits with
  EXIT_GENERAL (Clap error).
- Daemon startup: fails gracefully with missing config.toml.
- Daemon startup: fails gracefully with unreachable database path.
- Daemon polling: mock VCS returns new version, build is triggered.
- Daemon polling: version already in BuildStore is skipped.
- Daemon polling: failed version is skipped without `--retry-failed`.
- Daemon polling: failed version is retried with `--retry-failed`.
- Daemon SIGHUP: valid config reload applies new poll intervals.
- Daemon SIGHUP: invalid config reload is rejected, daemon continues.
- Daemon shutdown: SIGTERM causes immediate exit.
- CLI build: `--force` flag rebuilds even if tag exists in registry.
- CLI build: `--keep-builders` prevents builder destruction.
- CLI build: `--recipe all` builds all recipes, fail-forward behavior.

### Mocking

- Mock `BuildStore`, `Notifier`, `MetricsExporter` traits for command tests.
- Mock `VcsClient` for daemon polling tests.
- Mock `ImageBuilder` and `RegistryClient` for BuildCommand tests.
- Use `tempdir` for config and recipe files in integration tests.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| C1 | Should the gRPC auth token support multiple tokens (one per client), or is a single static token sufficient? | RESOLVED | Follow best practices: support multiple tokens (comma-separated in .env as `GRPC_AUTH_TOKENS`) for per-client auditability and revocation. |
| C2 | What is the shutdown timeout for in-progress builds before force-kill? Should it be configurable? | RESOLVED | No shutdown timeout. Just stop immediately. In-progress builds are abandoned; stale `InProgress` records are detected on next startup. |
| C3 | What is the priority of the C-FFI surface (cdylib/staticlib)? Phase 1 or deferred? | RESOLVED | C-FFI is Phase 4. The library crate (rlib) is Phase 1, but cdylib/staticlib are deferred to Phase 4. |
| C4 | Should `dockermint-cli build --recipe all` be supported in Phase 1, or only single-recipe builds? | RESOLVED | Follow best practices: support `--recipe all` in Phase 1. Builds all recipes sequentially, fail-forward (continue on failure), summary at end, exit code = highest severity. |
| C5 | Should the daemon support config reload (SIGHUP) without restart, or is restart required for config changes? | RESOLVED | YES, daemon supports SIGHUP for live config reload. Reload is atomic (all or nothing). Some settings (db path, listen addresses) require restart -- logged as warnings if changed. |
| C6 | Should `--dry-run` output the generated Dockerfile to stdout, to a file, or both? | RESOLVED | Follow best practices: Dockerfile content to stdout (pipeable), resolved variable summary to stderr (does not interfere with stdout pipe). |

---

## Dependencies

| Crate | Use case | Status |
| :---- | :------- | :----- |
| `clap` | CLI argument parsing with derive macros | Delegate to @lead-dev: evaluate clap (v4) for CLI, derive feature, musl/aarch64 compatibility |
| `indicatif` | Progress bars for CLI mode | Already listed in CLAUDE.md preferred tools |
| `tokio` | Async runtime for daemon, signal handling | Already listed in architecture-overview.md conventions |
| `tonic` | gRPC server and client (Phase 3) | Delegate to @lead-dev: evaluate tonic for gRPC, mTLS support, musl/aarch64 compatibility |
| `prost` | Protocol buffer code generation (Phase 3) | Delegate to @lead-dev: evaluate prost (tonic dependency) |
| `dotenvy` | Load .env secrets | Already listed in architecture-overview.md section 4.5 |
| `anyhow` | Application-level error wrapping in commands | Already listed in architecture-overview.md conventions |
| `thiserror` | DaemonError definition | Already listed in architecture-overview.md conventions |
| `ctrlc` or `tokio::signal` | Signal handling for graceful shutdown | Delegate to @lead-dev: evaluate whether tokio::signal is sufficient or ctrlc is needed |
