# Dockermint

Automated, recipe-driven Docker image pipeline for Cosmos SDK blockchains
and their sidecars. Define a chain once in TOML — Dockermint handles the
Dockerfile, cross-compilation, and registry push without touching Rust code.

---

## Overview

Dockermint is an open-source CI/CD pipeline written in Rust. It standardizes
multi-architecture Docker image creation for Cosmos SDK nodes across three
operating modes:

- **CLI** — one-shot build, locally or via a remote BuildKit endpoint. Errors
  cause an immediate dump, log, and exit.
- **Daemon** — continuous polling for new GitHub releases. On error: log,
  notify, persist the failure, and continue polling.
- **RPC** — daemon with an optional gRPC server, accepting remote build
  requests from a CLI client. On error: log and return idle.

The central abstraction is the **Recipe**: a TOML file that fully describes
how to build a chain's Docker image — what flavors are available, which are
default, how to clone the source, how to generate the Dockerfile, and what
OCI labels to attach. New chains are onboarded by adding a recipe file; no
Rust code changes are required.

> **Status:** Phase 0 (all architecture specs confirmed). No production code
> has been written yet. All features below are planned for the phases indicated.

---

## Planned Features

### Recipe-driven extensibility

Each chain is described by a single TOML file in `recipes/`. Adding a chain
requires no Rust code changes — only a new recipe file.

### Flavor system

Recipes declare available and default flavors per dimension:

```toml
[flavours.available]
db_backend    = ["goleveldb", "pebbledb"]
binary_type   = ["dynamic", "static"]
running_env   = ["alpine3.23", "bookworm", "distroless"]
running_user  = ["root", "custom", "dockermint"]
build_tags    = ["netgo", "ledger", "muslc"]

[flavours.default]
db_backend    = "goleveldb"
binary_type   = "static"
running_env   = "alpine3.23"
running_user  = "root"
build_tags    = ["netgo", "muslc"]
```

The active flavor for any dimension is resolved in priority order:
`CLI args > config.toml per-chain override > config.toml global override > recipe defaults`

Incompatible flavor combinations produce an error before the build starts.

### Multi-architecture builds (planned: Phase 1)

BuildKit cross-compilation targeting `linux/amd64` and `linux/arm64`.
Per-platform builder instances (`dockermint-amd64`, `dockermint-arm64`) are
managed by Dockermint — created on first use and optionally destroyed after
each build. Both local and remote BuildKit endpoints are supported.

### Template engine (planned: Phase 1)

Dockerfile content is produced by a template engine that resolves two classes
of variables from the recipe:

- `{{UPPERCASE}}` — host variables injected by Dockermint (e.g. `{{HOST_ARCH}}`,
  `{{SEMVER_TAG}}`, `{{CREATION_TIMESTAMP}}`)
- `{{lowercase}}` — build variables resolved at build time, including
  shell-captured values (e.g. `{{golang_version}}`, `{{wasmvm_version}}`)

### OCI registry push (planned: Phase 1)

Registry authentication and image pushing via the OCI-compatible registry
module. Secrets (credentials) are stored exclusively in `.env`, never in
`config.toml`.

### Daemon mode with persistence and metrics (planned: Phase 2)

- Continuous GitHub release polling (configurable interval, per-chain override)
- Build state persistence (RedB, default)
- Prometheus metrics exporter
- Telegram notifications on build success or failure

### gRPC remote control (planned: Phase 3)

A gRPC server in the daemon accepts remote build requests from the CLI client.
Authentication supports both mTLS and token-based auth. Configured via the
`[grpc]` section in `config.toml`.

---

## Supported Chains

### Phase 1 targets

| Chain       | Binary      | Sidecars         |
| :---------- | :---------- | :--------------- |
| Cosmos Hub  | `gaiad`     | —                |
| Axelar      | `axelard`   | Tofnd, Vald      |
| Fetch       | `fetchd`    | —                |
| Injective   | `injectived`| Peggo            |
| Osmosis     | `osmosisd`  | —                |

### Current recipes (available now)

| Chain      | Binary  | Recipe file              |
| :--------- | :------ | :----------------------- |
| Cosmos Hub | `gaiad` | `recipes/cosmos-gaiad.toml` |
| Kyve       | `kyved` | `recipes/kyve-kyved.toml`   |

---

## Architecture

```
dockermint-cli          dockermint-daemon
(one-shot build)        (polling + optional gRPC server)
       |                       |
       +----------+------------+
                  |
         SHARED CORE PIPELINE
                  |
  config -> checker -> recipe -> scrapper
                                    |
  push <- builder <- builder <- builder
               (buildx)  (Dockerfile)  (template engine)
       |
  saver / notifier / metrics

  CROSS-CUTTING: logger, commands
```

All modules are described in `docs/specs/`. The full architecture reference is
`docs/specs/architecture-overview.md`.

### Module summary

| Module     | Responsibility                                              |
| :--------- | :---------------------------------------------------------- |
| `config`   | Load and merge `config.toml`, `.env`, and CLI args          |
| `checker`  | Verify Docker, BuildKit, disk, and network prerequisites    |
| `recipe`   | Parse TOML recipes, resolve flavors, validate compatibility |
| `scrapper` | GitHub API client: fetch tags and releases                  |
| `builder`  | Template engine, BuildKit manager, Go recipe builder        |
| `push`     | OCI registry authentication and image push                  |
| `saver`    | Build state persistence                                     |
| `notifier` | Build status notifications (default: Telegram)              |
| `metrics`  | Prometheus metrics server                                   |
| `cli`      | Clap-based CLI with subcommands and exit code mapping       |
| `logger`   | Structured logging with log rotation                        |
| `commands` | Shell command execution shared by all modules               |

### Compile-time feature modules

Dockermint modules are selectable at compile time. Default implementations:

| Concern    | Default implementation      |
| :--------- | :-------------------------- |
| Database   | RedB                        |
| Notifier   | Telegram                    |
| VCS        | GitHub                      |
| SSL        | OpenSSL (vendored)          |
| Registry   | OCI                         |
| Builder    | BuildKit                    |
| Metrics    | Prometheus                  |

---

## Configuration

Dockermint is configured via `config.toml`. Secrets (GitHub PAT, registry
credentials, notifier tokens) are stored exclusively in `.env` and are never
written to `config.toml`.

All configuration files carry a `config_version` field. Unknown fields are
rejected at startup to prevent silent misconfiguration.

See `docs/specs/config.md` for the full schema.

---

## Compilation Targets

Dockermint must compile and run on all five toolchains:

| Target                         |
| :----------------------------- |
| `x86_64-unknown-linux-gnu`     |
| `x86_64-unknown-linux-musl`    |
| `aarch64-unknown-linux-gnu`    |
| `aarch64-unknown-linux-musl`   |
| `aarch64-apple-darwin`         |

---

## Project Status

| Phase | Target  | Scope                                          | Status    |
| :---- | :------ | :--------------------------------------------- | :-------- |
| 0     | v0.1.0  | Architecture specs (all modules)               | Complete  |
| 1     | v0.2.0  | CLI mode, 5 chains, BuildKit, OCI push         | Planned   |
| 2     | v0.3.0  | Daemon mode, persistence, metrics, notifier    | Planned   |
| 3     | v0.4.0  | gRPC server and authenticated CLI client       | Planned   |
| 4     | v1.0.0  | Chain expansion, C-FFI library, security audit | Planned   |

See `docs/ROADMAP.md` for the full phase breakdown.

---

## Contributing

Development follows a structured workflow defined in `CLAUDE.md`:
architecture spec confirmed by the team, GitHub issue created, code
implemented against the spec, test suite and mutation testing passed, code
review approved, then committed and opened as a pull request. No step may be
skipped.

See [CLAUDE.md](./CLAUDE.md) for the complete development workflow, agent
responsibilities, and contribution guidelines.

---

## License

Apache License, Version 2.0. See [LICENSE](./LICENSE).
