# Dockermint Roadmap

Last updated: 2026-04-13 (Rust recipe builder added to Phase 0 + Phase 1)

---

## Phase 0 -- Foundation [IN PROGRESS]

> Define architecture specs and design each feature module before any implementation begins.

- **Target**: N/A
- **Dependencies**: none

### Specs

- [x] Architecture overview (foundational reference) -- `docs/specs/architecture-overview.md`
- [x] CLI architecture and subcommand design -- `docs/specs/cli.md`
- [x] Configuration loading and validation -- `docs/specs/config.md`
- [x] Structured logging with rotation -- `docs/specs/logger.md`
- [x] System requirements checker -- `docs/specs/checker.md`
- [x] Recipe parsing and flavor resolution -- `docs/specs/recipe.md`
- [x] VCS client (GitHub) and tag/release scraping -- `docs/specs/scraper.md`
- [x] Dockerfile generation and template engine -- `docs/specs/builder.md`
- [x] BuildKit cross-compilation manager -- `docs/specs/builder.md`
- [x] Go recipe builder -- `docs/specs/builder.md`
- [x] Rust recipe builder -- `docs/specs/builder.md`
- [x] Registry auth and image pushing (OCI) -- `docs/specs/push.md`
- [x] Build state persistence -- `docs/specs/saver.md`
- [x] Notification system -- `docs/specs/notifier.md`
- [x] Metrics server -- `docs/specs/metrics.md`
- [x] PebbleDB flavor support (Go builder) -- `docs/specs/pebbledb.md`

---

## Phase 1 -- CLI Mode [PLANNED]

> First runnable mode -- one-shot CLI builds for supported chains with local and remote Docker/BuildKit support.

- **Target**: v0.1.0
- **Priority**: P0
- **Dependencies**: Phase 0 (all specs confirmed)

### Core

- [ ] `dockermint-cli build ...` binary with Clap subcommands
- [ ] GitHub PAT support for private/rate-limited clones
- [ ] `config.toml` loading with CLI argument overrides
- [ ] Recipe parsing and flavor resolution (CLI args > config.toml > recipe defaults)
- [ ] Dockerfile generation via `TemplateEngine` (Go and Rust recipes)
- [ ] Go recipe builder (`builder/go`)
- [ ] Rust recipe builder (`builder/rust`)
- [ ] BuildKit cross-compilation: `linux/amd64` and `linux/arm64` (local or remote)
- [ ] OCI registry push
- [ ] Local-only build support (no push)
- [ ] System requirements checker
- [ ] Structured logging with rotation

### Chains (Go)

- [ ] Cosmos Hub (`gaiad`)
- [ ] Axelar (`axelard`) + sidecars: Tofnd, Vald
- [ ] Fetch (`fetchd`)
- [ ] Injective (`injectived`) + sidecar: Peggo
- [ ] Osmosis (`osmosisd`)

### Chains (Rust)

- [ ] TBD -- specific Rust-based chains to be determined

---

## Phase 2 -- MVP Daemon [PLANNED]

> Daemon mode with continuous polling, build persistence, metrics, and notifications.

- **Target**: v0.2.0
- **Priority**: P1
- **Dependencies**: Phase 1

### Features

- [ ] `dockermint-daemon` binary with continuous GitHub release polling
- [ ] Build state persistence via redb
- [ ] Prometheus metrics exporter
- [ ] Telegram notifier
- [ ] Daemon error strategy (log, notify, persist failure, continue)

---

## Phase 3 -- gRPC [PLANNED]

> gRPC server mode for the daemon and authenticated CLI client connectivity.

- **Target**: v0.3.0
- **Priority**: P1
- **Dependencies**: Phase 2

### Features

- [ ] gRPC server in `dockermint-daemon` (`grpc = true` in config)
- [ ] `dockermint-cli` gRPC client for remote daemon connection
- [ ] Authenticated gRPC connections (mTLS + token-based auth, both supported)
- [ ] RPC error strategy (log, return idle)

---

## Phase 4 -- Expansion [PLANNED]

> Broaden chain support, expose a C-FFI library for external consumers, and harden the system with an external audit.

- **Target**: v1.0.0
- **Priority**: P2
- **Dependencies**: Phase 3

### Features

- [ ] Additional chains (TBD)
- [ ] C-FFI library (cdylib/staticlib) for external consumers
- [ ] External security audit
