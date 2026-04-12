# Architecture

Dockermint is a CI/CD pipeline written in Rust that automates Docker image
creation for Cosmos-SDK blockchain nodes. Its design follows two core
principles:

1. **Zero Rust changes for new chains.** Adding support for a new blockchain
   requires only a new TOML recipe file.
2. **Replaceable backends.** Every I/O-bound module (database, notifier, VCS,
   builder, registry, metrics) is defined as a trait and selected at compile
   time via Cargo feature flags.

---

## Operating Modes

Dockermint runs in one of two modes, selected by CLI subcommand:

| Mode   | Subcommand          | Behavior                                                         |
| :----- | :------------------ | :--------------------------------------------------------------- |
| CLI    | `dockermint build`  | One-shot build, then exit                                        |
| Daemon | `dockermint daemon` | Continuous polling for new GitHub releases, builds them          |

The daemon can optionally start an embedded HTTP server alongside its polling
loop by passing `--rpc`:

```bash
# Daemon only:
dockermint daemon

# Daemon + embedded RPC server:
dockermint daemon --rpc --rpc-bind 0.0.0.0:9100
```

### Error Handling by Mode

| Mode   | On unrecoverable error                                                  |
| :----- | :---------------------------------------------------------------------- |
| CLI    | Log, dump context, and exit with non-zero status                        |
| Daemon | Log, dump, send notification, register failure in DB, continue polling  |
| Daemon (RPC active) | Log, dump, and return idle status to the HTTP caller      |

---

## Module Map

```
src/
├── main.rs          Entry point: parse CLI, load config, dispatch mode
├── lib.rs           Crate root: re-exports public API
├── error.rs         Centralized error types (thiserror)
├── cli/             Clap-based CLI definition
│   └── commands/    Subcommand structs: build, daemon
├── config/          config.toml loading + .env secrets
├── logger/          tracing subscriber init with optional file rotation
├── checker/         System tool verification + singleton lock file
├── recipe/          TOML parsing, flavor resolution, validation
│   ├── host_vars.rs Host variable collection (HOST_ARCH, SEMVER_TAG, etc.)
│   ├── types.rs     Recipe structs, FlavorValue, SelectedFlavours
│   └── validation.rs Flavor compatibility checks
├── scrapper/        GitHub API client (VCS trait)
├── builder/         Dockerfile generation, template engine, buildx
│   ├── template.rs  TemplateEngine: {{variable}} interpolation
│   ├── dockerfile.rs Data-driven multi-stage Dockerfile generator
│   ├── buildkit.rs  BuildKitBuilder (ImageBuilder impl)
│   └── go/          Go build script generation (ldflags, tags, shell conversion)
├── push/            Registry authentication and image push (RegistryClient trait)
├── saver/           Build state persistence (Database trait)
├── notifier/        Build notifications (Notifier trait)
├── commands/        tokio::process command execution utilities
└── metrics/         Prometheus metrics server (MetricsCollector trait)
```

---

## Trait-Based Backend Architecture

Each replaceable module exposes a single trait. The concrete type is bound at
compile time via a `type SelectedXxx = concrete::Type;` alias, which the rest
of the codebase imports. Adding an alternative backend means:

1. Implementing the trait in a new submodule.
2. Adding a feature flag to `Cargo.toml`.
3. Adding a conditional `type SelectedXxx` alias.

No other code changes are required.

| Module    | Trait               | Default Implementation    | Feature Flag |
| :-------- | :------------------ | :------------------------ | :----------- |
| saver     | `Database`          | `RedbDatabase`            | `redb`       |
| notifier  | `Notifier`          | `TelegramNotifier`        | `telegram`   |
| scrapper  | `VersionControlSystem` | `GithubClient`         | `github`     |
| builder   | `ImageBuilder`      | `BuildKitBuilder`         | `buildkit`   |
| push      | `RegistryClient`    | `OciRegistry`             | `oci`        |
| metrics   | `MetricsCollector`  | `PrometheusCollector`     | `prometheus` |

---

## Recipe and Flavor System

A **Recipe** is a TOML file that fully describes how to build a blockchain
binary. It is the primary unit of configuration — no recipe-specific Rust code
is needed.

A **Flavor** is a build dimension (e.g., `db_backend`, `running_env`). Each
dimension has a set of available values and a default. The resolved selection
follows a strict priority chain:

```
CLI --flavor args  >  config.toml [flavours]  >  recipe [flavours.default]
```

The `recipe::resolve` function applies this chain, validates selections against
`flavours.available`, injects profile variables, and returns a `ResolvedRecipe`
ready for the builder.

---

## Template Engine

`builder::template::TemplateEngine` performs `{{variable}}` substitution in
recipe strings. It distinguishes two classes of variables by naming convention:

| Convention     | Source                                | Example           |
| :------------- | :------------------------------------ | :---------------- |
| `{{UPPERCASE}}`| Host environment at Dockermint startup| `{{HOST_ARCH}}`   |
| `{{lowercase}}`| Build-time shell commands or profiles | `{{repo_version}}`|

The engine leaves unrecognised placeholders intact so downstream stages can
detect unresolved variables. The `TemplateEngine::unresolved_vars` helper
scans a string and returns a list of any remaining `{{...}}` names.

---

## Variable Resolution Pipeline

Template variable expansion happens in two distinct passes, at different points
in time.

### Pass 1 — Parse time (host variables)

Executed by `recipe::host_vars::collect` and `recipe::resolve` before the
Docker build starts.

1. `host_vars::collect` populates `HOST_ARCH`, `CREATION_TIMESTAMP`,
   `SEMVER_TAG`, `BUILD_TAGS_COMMA_SEP`, and `repository_path` from the
   local system environment and the tag being built.
2. `recipe::resolve` calls `TemplateEngine::render` on every selected flavor
   default value, so `architecture = "{{HOST_ARCH}}"` becomes `"x86_64"` in
   the `resolved_variables` map before the builder sees it.
3. Profile variables for the active `FlavorValue::Single` dimensions are
   injected into the same map.

After pass 1, `ResolvedRecipe::resolved_variables` contains all host-level
values. Any `{{lowercase}}` placeholders from `[variables]` shell commands
are still present as literal `{{name}}` strings — they are intentionally
left for pass 2.

### Pass 2 — Build time (shell variables)

Executed inside the Dockerfile `RUN` instruction by the shell.

`builder::go::generate_build_script` calls `TemplateEngine::render` a second
time on linker variable templates, resolving any remaining host-variable
references. Then `template_to_shell` converts every surviving `{{name}}`
placeholder to `$name`, producing shell variable interpolation. The final
`RUN` script looks like:

```bash
set -e; \
    repo_version=$(git describe ...); \
    repo_commit=$(git log -1 --format='%H'); \
    go build -mod=readonly \
      -tags=netgo,muslc \
      -ldflags="... -X 'sdk/version.Version=$repo_version'" \
      -o /go/bin/gaiad \
      /workspace/cmd/gaiad
```

The `$repo_version` reference is resolved at container build time by the
shell — never by Dockermint itself.

---

## Dockerfile Generation

`builder::dockerfile::generate` produces a complete multi-stage Dockerfile
from a `ResolvedRecipe`. It is fully data-driven: adding a new blockchain
chain requires only a new TOML file.

The generation flow:

```
ResolvedRecipe
    |
    +-- Stage 1 (builder):
    |       scrapper.image  -> FROM ... AS builder
    |       scrapper.install + builder.install (distro auto-detected) -> RUN
    |       scrapper.env -> ARG declarations
    |       scrapper.method -> RUN git clone (authenticated or public)
    |       scrapper.directory -> WORKDIR
    |       build.env -> ENV
    |       pre_build[*] where has_value(condition) -> ADD/RUN/COPY
    |       header.type dispatch -> RUN <build_script>
    |
    +-- Stage 2 (runner):
            running_env -> running_env_to_image() -> FROM ... AS runner
            running_user + user config -> RUN adduser/useradd
            copy.always_entries() -> COPY --from=builder
            copy.conditional_entries(binary_type) -> COPY --from=builder
            expose.ports -> EXPOSE
            labels -> LABEL
            running_user -> USER
            copy entrypoint -> ENTRYPOINT
```

---

## Build Pipeline

The complete pipeline for a single recipe and tag. Steps marked **(CLI)** are
implemented in `run_build()`; steps marked **(Daemon)** are part of the
planned `run_daemon()` loop.

| Step | Module | Status | Action |
| :--- | :----- | :----- | :----- |
| 1 | `checker` | CLI + Daemon | Verify `docker`, `docker buildx`, and `git` are present |
| 2 | `scrapper` | Daemon only | Fetch releases from GitHub, apply include/exclude glob filters |
| 3 | `recipe` | CLI + Daemon | Load TOML, resolve flavors (pass 1 variable expansion), inject profile variables; produce `ResolvedRecipe` |
| 4 | `builder` | CLI + Daemon | Generate Dockerfile, run pass 2 shell variable conversion, execute `docker buildx build --platform` |
| 5 | `push` | CLI + Daemon | Authenticate with registry, push image (CLI: when `--push` is set) |
| 6 | `saver` | Daemon only | Persist build record to the database |
| 7 | `notifier` | Daemon only | Send build status notification |
| 8 | `metrics` | Daemon only | Update Prometheus counters and histograms |

### CLI pipeline (`run_build`)

`run_build()` executes steps 1, 3–5. Step 2 is skipped because the tag is
provided directly via `--tag`. Steps 6–8 are daemon concerns.

Builders are created with `persist=false`: they are set up before the build
and torn down after, even on failure.

### Daemon pipeline (`run_daemon`)

`run_daemon()` executes all eight steps in a polling loop. Builders are
created with `persist=true` so they survive across polling cycles. Individual
build failures are logged and persisted (step 6), a notification is sent
(step 7), and the loop continues — the daemon never exits on a per-recipe
failure.

---

## Configuration Loading Pipeline

```
1. CLI parses arguments (clap)
2. config::load(path)  or  config::load_default()
      -> reads file, toml::from_str, validate()
3. config::load_secrets()
      -> dotenvy::dotenv(), reads env vars into Secrets struct
4. config::apply_daemon_overrides()   [daemon mode only]
      -> CLI flags (--poll-interval, --max-builds, --rpc, --rpc-bind)
         overwrite corresponding config.toml values
```

---

## Concurrency Model

Dockermint uses Tokio as the async runtime. CPU-bound work (template
rendering, argument construction) is synchronous and cheap enough to stay on
the async executor. I/O-bound operations (HTTP, process spawning, file I/O)
use `async/await`. The `rayon` crate is available for any future CPU-parallel
work.

Each backend struct is `Send + Sync` — enforced by a compile-time trait bound
test in every backend module.

---

## Toolchain Targets

The codebase compiles cleanly on all five targets:

- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`
- `aarch64-apple-darwin`

---

## Dependency Highlights

| Crate               | Purpose                              |
| :------------------ | :----------------------------------- |
| `clap`              | CLI parsing with derive macros       |
| `tokio`             | Async runtime                        |
| `serde` + `toml`    | Configuration and recipe parsing     |
| `thiserror`         | Typed error definitions              |
| `anyhow`            | Application-level error propagation  |
| `tracing`           | Structured logging                   |
| `tracing-appender`  | Daily-rolling log file rotation      |
| `reqwest`           | HTTP client (TLS backend selectable) |
| `redb`              | Embedded key-value database (optional)|
| `axum`              | HTTP server for RPC and metrics      |
| `globset`           | Glob pattern matching for tag filters|
| `semver`            | Semantic version parsing             |
| `secrecy`           | Zero-on-drop secret string wrapper   |
| `dotenvy`           | `.env` file loading                  |
| `indicatif`         | Progress bars for long-running ops   |
