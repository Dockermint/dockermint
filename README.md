<p align="center">
  <img src="./assets/Logo.svg" alt="Dockermint" width="240" />
</p>

# Dockermint

The first CI/CD Pipeline for Cosmos SDK.

---

## What is Dockermint?

Building and maintaining Docker images for blockchain nodes is repetitive
work: tracking upstream releases, writing multi-stage Dockerfiles, managing
cross-compilation for `amd64` and `arm64`, pushing to a registry, and
keeping everything consistent across a fleet of chains. Teams usually end up
with a collection of bespoke shell scripts that diverge over time.

Dockermint replaces that with a single, uniform pipeline. Define a chain once
in a TOML **recipe** file and Dockermint handles the rest — Dockerfile
generation, cross-compilation via BuildKit, release polling, registry push,
persistence, and notifications. Adding a new chain requires no code changes:
only a new recipe file.

**Who is Dockermint for?** Infrastructure engineers and DevOps teams running
Cosmos SDK validator nodes or RPC infrastructure who need reproducible,
multi-architecture Docker images without maintaining a separate build system
per chain.

> **Status:** Phase 0 (all architecture specs confirmed). No production code
> has been written yet. All features below are planned for the phases indicated.

---

## Resources

| Resource         | URL                                          |
| :--------------- | :------------------------------------------- |
| Main site        | https://dockermint.io                        |
| Documentation    | https://docs.dockermint.io/                  |
| GitHub           | https://github.com/Dockermint/dockermint     |
| Related project   | [Pebblify](https://github.com/Dockermint/pebblify) |

---

## Operating Modes

Dockermint runs in three modes depending on your use case:

- **CLI** — one-shot build, locally or via a remote BuildKit endpoint. Errors
  cause an immediate dump, log, and exit.
- **Daemon** — continuous polling for new GitHub releases. On error: log,
  notify, persist the failure, and continue polling.
- **RPC** — daemon with an optional gRPC server, accepting remote build
  requests from a CLI client. On error: log and return idle.

---

## Planned Features

### Recipe-driven extensibility

Each chain is described by a single TOML file in `recipes/`. Adding a chain
requires no code changes — only a new recipe file.

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
Per-platform builder instances are managed by Dockermint — created on first
use and optionally destroyed after each build. Both local and remote BuildKit
endpoints are supported.

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

| Chain       | Binary       | Sidecars         |
| :---------- | :----------- | :--------------- |
| Cosmos Hub  | `gaiad`      | —                |
| Axelar      | `axelard`    | Tofnd, Vald      |
| Fetch       | `fetchd`     | —                |
| Injective   | `injectived` | Peggo            |
| Osmosis     | `osmosisd`   | —                |

### Current recipes (available now)

| Chain      | Binary  | Recipe file                  |
| :--------- | :------ | :--------------------------- |
| Cosmos Hub | `gaiad` | `recipes/cosmos-gaiad.toml`  |
| Kyve       | `kyved` | `recipes/kyve-kyved.toml`    |

---

## Configuration

Dockermint is configured via `config.toml`. Secrets (GitHub PAT, registry
credentials, notifier tokens) are stored exclusively in `.env` and are never
written to `config.toml`.

All configuration files carry a `config_version` field. Unknown fields are
rejected at startup to prevent silent misconfiguration.

Full schema reference: [docs.dockermint.io/configuration](https://docs.dockermint.io/configuration)
or `docs/specs/config.md` in this repository.

---

## Project Status

| Phase | Target  | Scope                                          | Status   |
| :---- | :------ | :--------------------------------------------- | :------- |
| 0     | N/A  | Architecture specs (all modules)               | Complete |
| 1     | v0.1.0  | CLI mode, 5 chains, BuildKit, OCI push         | Planned  |
| 2     | v0.2.0  | Daemon mode, persistence, metrics, notifier    | Planned  |
| 3     | v0.3.0  | gRPC server and authenticated CLI client       | Planned  |
| 4     | v1.0.0  | Chain expansion, C-FFI library, security audit | Planned  |

See `docs/ROADMAP.md` for the full phase breakdown.

---

## Technical Overview

This section covers internals relevant to contributors and operators.

### Architecture

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

All module specs are in `docs/specs/`. The full reference is
[docs.dockermint.io/architecture](https://docs.dockermint.io/architecture)
or `docs/specs/architecture-overview.md` in this repository.

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

### Template engine variables

Dockerfile content is produced by a template engine that resolves two classes
of variables from the recipe:

- `{{UPPERCASE}}` — host variables injected by Dockermint (e.g. `{{HOST_ARCH}}`,
  `{{SEMVER_TAG}}`, `{{CREATION_TIMESTAMP}}`)
- `{{lowercase}}` — build variables resolved at build time, including
  shell-captured values (e.g. `{{golang_version}}`, `{{wasmvm_version}}`)

### Compile-time feature modules

Key modules are selectable at compile time. Defaults:

| Concern  | Default implementation |
| :------- | :--------------------- |
| Database | RedB                   |
| Notifier | Telegram               |
| VCS      | GitHub                 |
| SSL      | OpenSSL (vendored)     |
| Registry | OCI                    |
| Builder  | BuildKit               |
| Metrics  | Prometheus             |

### Compilation targets

Dockermint compiles and runs on all five toolchains:

| Target                       |
| :--------------------------- |
| `x86_64-unknown-linux-gnu`   |
| `x86_64-unknown-linux-musl`  |
| `aarch64-unknown-linux-gnu`  |
| `aarch64-unknown-linux-musl` |
| `aarch64-apple-darwin`       |

---

## Contributing

Dockermint follows a design-first engineering workflow. Every feature begins
with an architecture spec that is reviewed and confirmed before any code is
written. A GitHub issue is opened to track the work, code is implemented
against the spec, and the change must pass the full test suite — including
mutation testing — before a pull request is opened. Code review is required
before merge. No step may be skipped.

See [docs.dockermint.io/contributing](https://docs.dockermint.io/contributing)
or `docs/` in this repository for the full contribution guide.

---

## License

Apache License, Version 2.0. See [LICENSE](./LICENSE).
