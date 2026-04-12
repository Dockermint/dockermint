# Dockermint

![CI](https://github.com/Dockermint/dockermint/actions/workflows/build.yml/badge.svg)
![Lint](https://github.com/Dockermint/dockermint/actions/workflows/lint.yml/badge.svg)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)
![Rust](https://img.shields.io/badge/rust-1.94.1%2B-orange)

The first CI/CD pipeline for Cosmos SDK blockchain Docker images. Dockermint
automates and standardizes multi-architecture Docker image creation for
Cosmos-SDK nodes and their sidecars without requiring any Rust code changes to
add a new chain.

<p align="center">
  <img src="assets/Logo.svg" alt="Dockermint logo" width="200" />
</p>

---

## Overview

Dockermint is data-driven: every build is controlled by a **recipe** TOML file.
Adding support for a new blockchain is a matter of writing a TOML file, not
modifying Rust source code.

**Three key concepts:**

| Concept  | Description                                                                 |
| :------- | :-------------------------------------------------------------------------- |
| Recipe   | TOML file that defines the complete build schema for one blockchain binary  |
| Flavor   | A named dimension within a recipe (e.g. `db_backend`, `binary_type`)       |
| Mode     | How Dockermint runs: CLI (one-shot), Daemon (polling), or RPC (HTTP server) |

**Three operating modes:**

- **CLI** -- one-shot build triggered from the command line; exits on completion or error.
- **Daemon** -- continuously polls a VCS provider for new releases and builds images automatically.
- **RPC** -- daemon mode that also starts an HTTP server accepting remote build requests.

---

## Features

- Data-driven builds: new chains added with a TOML file, zero Rust changes required
- Multi-architecture support: `linux/amd64` and `linux/arm64` via Docker buildx
- Flavor system: per-recipe dimensions (db backend, binary type, target env, network, etc.)
- Priority chain for flavor resolution: CLI args > `config.toml` > recipe defaults
- Template engine: `{{UPPERCASE}}` host variables and `{{lowercase}}` build variables
- Multi-stage Dockerfiles generated fully from recipe data
- Go build script generation with `-ldflags`, `-X` variable injection, and `-tags`
- Profile tables for network-specific variable injection (e.g. Kyve mainnet vs kaon)
- Conditional `[[pre_build]]` steps and conditional `[copy.*]` sections
- Structured logging with optional rotation and JSON output
- System requirements verification (docker, buildx, git) at startup
- Singleton enforcement via lock file to prevent concurrent daemon instances
- Compile-time module selection via Cargo feature flags
- All secrets kept in `.env`, never in `config.toml`

---

## Quick Start

### Prerequisites

- Rust 1.94.1 or later
- Docker with buildx plugin
- Git

### Build

```bash
git clone https://github.com/Dockermint/dockermint.git
cd dockermint
cargo build --release
```

### Configure

Copy the secrets template and fill in your credentials:

```bash
cp .env.example .env
# Edit .env with your GitHub credentials and registry tokens
```

The default `config.toml` at the repository root is ready to use. Key settings:

```toml
version = 1
recipes_dir = "recipes"

[daemon]
poll_interval_secs = 60
max_builds_per_cycle = 1

[rpc]
bind = "127.0.0.1:9100"

[docker]
socket_uri      = "unix:///var/run/docker.sock"
builder_prefix  = "dockermint"
```

To override flavors for a specific recipe, add a section to `config.toml`:

```toml
[flavours.cosmos-gaiad]
db_backend = "pebbledb"
build_tags = ["netgo", "muslc", "ledger"]
```

### Run a build (CLI mode)

```bash
# Build using recipe defaults
./target/release/dockermint build \
    --recipe recipes/cosmos-gaiad.toml \
    --tag v21.0.1

# Override a flavor from the command line
./target/release/dockermint build \
    --recipe recipes/cosmos-gaiad.toml \
    --tag v21.0.1 \
    --flavor db_backend=pebbledb \
    --platform linux/amd64,linux/arm64

# Build and push to registry
./target/release/dockermint build \
    --recipe recipes/cosmos-gaiad.toml \
    --tag v21.0.1 \
    --push
```

### Run the daemon

```bash
# Daemon mode (polls VCS for new releases)
./target/release/dockermint daemon

# Daemon with RPC server
./target/release/dockermint daemon --rpc --rpc-bind 0.0.0.0:9100

# Watch only specific recipes
./target/release/dockermint daemon --recipes cosmos-gaiad kyve-kyved
```

---

## Architecture

Dockermint is organized around a central trait for each replaceable module. The
active implementation is selected at compile time via Cargo feature flags.

```
src/
├── main.rs            -- entry point: parse CLI, load config, dispatch
├── lib.rs             -- run_build() and run_daemon() (both implemented)
├── error.rs           -- unified error hierarchy (thiserror)
├── cli/               -- Clap definitions: Cli, Commands, BuildArgs, DaemonArgs
├── config/            -- config.toml loading, validation, CLI override merging
├── logger/            -- tracing-subscriber init (stdout/file, plain/JSON)
├── checker/           -- system requirements (docker, buildx, git) + lock file
├── recipe/            -- TOML parsing, flavor resolution, host_vars, validation
├── builder/
│   ├── template.rs    -- TemplateEngine: {{variable}} interpolation
│   ├── dockerfile.rs  -- multi-stage Dockerfile generation (data-driven)
│   ├── go/            -- Go build script generator (ldflags, -X, -tags)
│   └── buildkit.rs    -- BuildKitBuilder: full buildx implementation
├── scrapper/          -- VersionControlSystem trait + GithubClient (implemented)
├── push/              -- RegistryClient trait + OciRegistry (implemented)
├── saver/             -- Database trait + RedbDatabase (implemented)
├── notifier/          -- Notifier trait + TelegramNotifier (implemented)
├── metrics/           -- MetricsCollector trait + PrometheusCollector (implemented)
└── commands/          -- tokio::process wrapper with structured error capture
```

**Trait-based design:** every external integration implements a trait. To swap
the database from redb to SQLite, implement `saver::Database` and gate it behind
a new feature flag.

| Module   | Trait                  | Default feature | Default impl       |
| :------- | :--------------------- | :-------------- | :----------------- |
| Database | `saver::Database`      | `redb`          | `saver::redb`      |
| Notifier | `notifier::Notifier`   | `telegram`      | `notifier::telegram` |
| VCS      | `scrapper::VersionControlSystem` | `github` | `scrapper::github` |
| Registry | `push::RegistryClient` | `oci`           | `push::oci`        |
| Builder  | `builder::ImageBuilder` | `buildkit`     | `builder::buildkit` |
| Metrics  | `metrics::MetricsCollector` | `prometheus` | `metrics::prometheus` |

Full documentation: [docs/markdown/architecture.md](docs/markdown/architecture.md)

---

## Configuration

Dockermint is configured via `config.toml` (structure) and `.env` (secrets).
CLI arguments always take highest priority.

**config.toml** controls:
- Logging level, format, and output directory
- Daemon polling interval and maximum builds per cycle
- RPC server bind address
- Per-recipe flavor overrides
- Database path, notifier toggle, VCS concurrency, registry URL, metrics endpoint

**`.env`** holds all secrets (never committed):

```bash
GH_USER=your-github-username
GH_PAT=ghp_...
TELEGRAM_TOKEN=123456:ABC-...
TELEGRAM_CHAT_ID=-100123456789
REGISTRY_USER=your-registry-user
REGISTRY_PASSWORD=your-registry-token
```

All configuration files are versioned. The current schema version is `1`.

Full reference: [docs/markdown/configuration.md](docs/markdown/configuration.md)

---

## Recipes

A recipe is a TOML file that fully describes how to build one blockchain binary
into a Docker image. Adding a new chain requires only a new recipe file.

**Minimal recipe structure:**

```toml
[meta]
schema_version = 1
min_dockermint_version = "0.1.0"

[header]
name = "Cosmos"
repo = "https://github.com/cosmos/gaia"
type = "golang"
binary_name = "gaiad"

[flavours.available]
db_backend = ["goleveldb", "pebbledb"]
binary_type = ["dynamic", "static"]
running_env = ["alpine3.23", "bookworm", "distroless"]

[flavours.default]
db_backend = "goleveldb"
binary_type = "static"
running_env = "alpine3.23"

[scrapper]
image = "golang:1.23-alpine3.21"
method = "try-authenticated-clone"
directory = "{{repository_path}}"

[build.linker.flags]
static = "-linkmode=external -w -s -extldflags '-Wl,-z,muldefs -static'"

[build.linker.variables]
"github.com/cosmos/cosmos-sdk/version.Version" = "{{repo_version}}"

[build.path]
path = "{{repository_path}}/cmd/gaiad"

[copy]
"/go/bin/gaiad" = { dest = "/usr/bin/gaiad", type = "entrypoint" }

[expose]
ports = [{ port = 26657, description = "RPC" }]

[image]
tag = "cosmos-gaiad-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"
```

**Variable conventions:**

| Pattern           | Resolved by    | Example                    |
| :---------------- | :------------- | :------------------------- |
| `{{UPPERCASE}}`   | Dockermint host | `{{HOST_ARCH}}`, `{{SEMVER_TAG}}` |
| `{{lowercase}}`   | Shell command at build time | `{{repo_version}}`, `{{denom}}` |

**Profiles** allow network-specific variable injection:

```toml
[profiles.network.mainnet]
denom = "ukyve"

[profiles.network.kaon]
denom = "tkyve"
```

Currently supported recipes:

| Recipe file           | Chain        | Binary  |
| :-------------------- | :----------- | :------ |
| `cosmos-gaiad.toml`   | Cosmos Hub   | `gaiad` |
| `kyve-kyved.toml`     | Kyve         | `kyved` |

Full format reference: [docs/markdown/recipe-format.md](docs/markdown/recipe-format.md)

---

## Roadmap

### What's Done

The following modules have real, tested implementations:

**Core infrastructure**

- [x] `error.rs` -- full error hierarchy for all modules using `thiserror`
- [x] `commands/` -- async shell execution with env injection and structured
      error capture (`execute`, `execute_with_env`, `execute_unchecked`)
- [x] `logger/` -- `tracing-subscriber` init with four output modes (stdout/file
      x plain/JSON), env-filter support, daily rotation
- [x] `checker/` -- async system requirements check (docker, buildx, git);
      file-based singleton lock with stale-PID detection and RAII `LockGuard`
- [x] `main.rs` -- full startup sequence: CLI parse, config load, log init,
      daemon override merging, mode dispatch

**Configuration**

- [x] `config/types.rs` -- typed `Config` struct with all sub-configs
      (`LogConfig`, `DaemonConfig`, `RpcConfig`, `DatabaseConfig`, etc.)
- [x] `config/mod.rs` -- `load()`, `load_default()`, `validate()`,
      `apply_daemon_overrides()`, `load_secrets()` (all with tests)

**CLI**

- [x] `cli/mod.rs` -- `Cli` struct with `--config` and `--log-level` global flags
- [x] `cli/commands/build.rs` -- `BuildArgs` with `--recipe`, `--tag`,
      `--platform`, `--flavor` (multi-value parsing), `--push`
- [x] `cli/commands/daemon.rs` -- `DaemonArgs` with `--poll-interval`,
      `--max-builds`, `--recipes`, `--rpc`, `--rpc-bind`

**Recipe engine**

- [x] `recipe/types.rs` -- full type tree: `Recipe`, `RecipeFlavours`,
      `FlavorValue` (single/multi), `SelectedFlavours`, `ResolvedRecipe`,
      `RecipeCopySection` with `always_entries()` and `conditional_entries()`
- [x] `recipe/validation.rs` -- flavor compatibility validation with
      host-variable template bypass
- [x] `recipe/host_vars.rs` -- `collect()` and `extend_from_env()`; ISO-8601
      UTC timestamp via Hinnant civil-from-days algorithm (no external date crate)
- [x] `recipe/mod.rs` -- `load()`, `load_all()`, `resolve_flavours()`,
      `resolve()` with full priority chain and profile injection

**Builder**

- [x] `builder/template.rs` -- `TemplateEngine::render()` and
      `TemplateEngine::unresolved_vars()`; byte-scanning O(n) implementation
- [x] `builder/dockerfile.rs` -- complete multi-stage Dockerfile generator:
      builder stage (clone, install, env, pre-build, build), runner stage
      (user creation, conditional copy, ports, labels, entrypoint); distro
      auto-detection for install commands; `running_env` to image mapping
- [x] `builder/go/mod.rs` -- `generate_build_script()`, `build_ldflags()`,
      `build_tags()`, `build_args()`, `template_to_shell()`
- [x] `builder/mod.rs` -- `ImageBuilder` trait, `BuildContext`, `BuildOutput`
- [x] `builder/buildkit.rs` -- `BuildKitBuilder`: `setup_builders()` creates
      per-platform `docker-container` buildx builders with bootstrap;
      `build()` generates Dockerfile, runs `docker buildx build` with
      `GIT_TAG`, `GH_USER`, `GH_PAT` build args; `cleanup()` removes builders
      and temp dir when `persist=false` (CLI mode), keeps them alive when
      `persist=true` (daemon mode)

**Registry**

- [x] `push/oci.rs` -- `OciRegistry`: `authenticate()` pipes password via stdin
      to `docker login`; `push_image()` runs `docker push <image>:<tag>`;
      `tag_exists()` uses `docker manifest inspect` (returns bool, no pull)

**Core pipeline**

- [x] `lib.rs::run_build()` -- full CLI pipeline: system check, recipe load,
      flavor resolve, host_vars collect, buildx setup, Dockerfile generation,
      `docker buildx build`, builder cleanup (even on failure), optional push

**Recipes**

- [x] `recipes/cosmos-gaiad.toml` -- Cosmos Hub (`gaiad`): flavors,
      scrapper, variables, builder, pre-build (wasmvm static lib), linker
      `-X` variables, copy, expose, OCI labels, image tag template
- [x] `recipes/kyve-kyved.toml` -- Kyve (`kyved`): same as above plus
      `network` flavor dimension and `[profiles.network.*]` for mainnet/kaon
      variable injection

**Daemon mode**

- [x] `scrapper/github.rs` -- `GithubClient`: GitHub REST API with pagination
      (100/page), optional Basic Auth via `GH_USER`/`GH_PAT`, globset include/
      exclude tag filtering, rate-limit detection with `retry-after` hint
- [x] `saver/redb.rs` -- `RedbDatabase`: embedded redb store keyed by
      `"recipe:tag"`, JSON-serialized `BuildRecord`, CRUD (`save_build`,
      `get_build`, `list_builds`, `is_built`), `Arc`-wrapped for thread safety,
      auto-creates parent dirs on open
- [x] `notifier/telegram.rs` -- `TelegramNotifier`: Telegram Bot API via
      reqwest, Markdown formatting, 4096-char truncation, three message types
      (start, success with duration, failure with error)
- [x] `lib.rs::run_daemon()` -- full poll loop: system check, DB open,
      persistent buildx builders, optional notifier, `GithubClient` init,
      per-recipe release fetch, skip-built filtering, `build_tag` with
      `InProgress`/`Success`/`Failed` DB lifecycle, best-effort notifications,
      graceful SIGINT/SIGTERM shutdown

**Documentation**

- [x] `docs/markdown/` -- architecture, configuration, features, modules,
      recipe-format, build-with-pebbledb guide
- [x] `docs/docusaurus/` -- MDX counterparts for all markdown docs
- [x] `docs/guides/build_with_pebbledb.md` -- PebbleDB build guide

### What's Left

**Phase 3 -- RPC server**

The HTTP endpoint is referenced in `DaemonArgs` and `run_daemon()` but has no
implementation yet. The plan is to use `axum` with `tower` middleware.

---

## Feature Flags

All default features are enabled unless explicitly disabled. Disable a default
feature to swap it for a custom implementation of the corresponding trait.

```toml
# Cargo.toml -- to use rustls instead of native-tls:
dockermint = { default-features = false, features = ["rustls-tls", "redb", "telegram", "github", "oci", "buildkit", "prometheus"] }
```

| Feature       | Enabled by default | What it activates                              |
| :------------ | :----------------- | :--------------------------------------------- |
| `redb`        | yes                | `saver::redb::RedbDatabase` (Database backend) |
| `telegram`    | yes                | `notifier::telegram::TelegramNotifier`         |
| `github`      | yes                | `scrapper::github::GithubClient`               |
| `native-tls`  | yes                | reqwest with native TLS (OpenSSL / Secure Transport) |
| `rustls-tls`  | no                 | reqwest with rustls (alternative to native-tls) |
| `oci`         | yes                | `push::oci::OciRegistry`                       |
| `buildkit`    | yes                | `builder::buildkit::BuildKitBuilder`           |
| `prometheus`  | yes                | `metrics::prometheus::PrometheusCollector` + axum |

At least one backend per module must be enabled; missing backends produce a
`compile_error!` at build time.

---

## Supported Toolchains

| Target                         | Status    |
| :----------------------------- | :-------- |
| `x86_64-unknown-linux-gnu`     | Supported |
| `x86_64-unknown-linux-musl`    | Supported |
| `aarch64-unknown-linux-gnu`    | Supported |
| `aarch64-unknown-linux-musl`   | Supported |
| `aarch64-apple-darwin`         | Supported |

Minimum Rust version: **1.94.1**

---

## Contributing

Code quality standards are defined in [CLAUDE.md](CLAUDE.md). Before opening a
pull request, verify:

```bash
cargo test
cargo build                          # zero warnings
cargo clippy -- -D warnings
cargo fmt --check
cargo deny check all
cargo audit
```

All public items must have doc comments. No `unwrap()` in library code. No
hardcoded credentials. See [.github/CONTRIBUTING.md](.github/CONTRIBUTING.md)
for the full contribution guide.

Branch from `develop`, follow Conventional Commits, sign your commits with GPG.

---

## License

Apache-2.0. See [LICENSE](LICENSE).
