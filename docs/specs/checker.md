# Feature: System Requirements Checker

## Context

The `checker` module verifies that all system prerequisites are met before
the build pipeline runs. It checks for Docker, BuildKit, network connectivity,
QEMU availability (for cross-platform builds), and manages the BuildKit builder
lifecycle. It also enforces singleton instance semantics to prevent concurrent
Dockermint runs from conflicting.

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)
Architecture reference: docs/specs/architecture-overview.md, Sections 1.2, 2.3, 8.3

---

## Requirements

1. [confirmed] Verify Docker is installed and the Docker daemon is running
2. [confirmed] Verify BuildKit is available (via `docker buildx` subcommand)
3. [confirmed] Verify network connectivity (at minimum: can reach GitHub API)
4. [confirmed] Verify QEMU is available for cross-platform builds; set up if missing
5. [confirmed] Enforce singleton instance (only one Dockermint process at a time)
6. [confirmed] BuildKit builder lifecycle: on each launch, verify Docker context on the configured Docker socket URI, create builders if missing, destroy stale builders
7. [confirmed] Per-platform builders: `dockermint-amd64` and `dockermint-arm64`
8. [confirmed] Docker socket URI from `[builder] docker_host` in config.toml (Q3)
9. [confirmed] Report failures per mode: CLI exits with code 4, Daemon logs + notifies
10. [confirmed] All checks run at startup before the pipeline begins
11. [confirmed] Builder lifecycle on shutdown: CLI mode destroys builders (default), Daemon mode keeps builders running (default). Both modes override via `--keep-builders` / `--destroy-builders` CLI flags.
12. [confirmed] Singleton lock file path follows platform best practices (XDG_RUNTIME_DIR on Linux, platform-conventional on macOS)

---

## Architecture

### Module placement

```
src/checker/
    mod.rs              -- Public API: run_checks(), CheckResult, re-exports
    error.rs            -- CheckerError enum (thiserror)
    docker.rs           -- Docker and BuildKit verification
    network.rs          -- Network connectivity checks
    qemu.rs             -- QEMU verification and setup for cross-platform builds
    singleton.rs        -- Process singleton enforcement
    builder_lifecycle.rs -- BuildKit builder create/destroy lifecycle
```

This module is NOT behind a feature gate. System prerequisite checks are
deterministic and have a single implementation. See architecture-overview.md
Section 2.3.

### Type design

#### Check result

```
CheckResult
  +-- docker: CheckStatus
  +-- buildkit: CheckStatus
  +-- network: CheckStatus
  +-- qemu: CheckStatus
  +-- singleton: CheckStatus
  +-- builders: BuilderLifecycleResult

CheckStatus
  -- Passed
  -- Failed { reason: String }

BuilderLifecycleResult
  +-- amd64: BuilderStatus
  +-- arm64: BuilderStatus

BuilderStatus
  -- Created           -- Builder was freshly created
  -- Verified          -- Existing builder verified healthy
  -- Recreated         -- Stale builder destroyed and recreated
  -- Failed { reason: String }
```

`CheckResult` aggregates the outcome of all checks. The caller inspects
it to determine whether to proceed or abort. In CLI mode, any `Failed`
status triggers exit code 4. In daemon mode, startup failures halt the
daemon; per-cycle failures log + notify + skip.

#### Checker configuration (derived from AppConfig)

```
CheckerConfig
  +-- docker_host: Option<String>      -- Docker socket URI from BuilderConfig
  +-- platforms: Vec<String>           -- Target platforms from BuilderConfig
  +-- network_check_url: String        -- URL to probe for connectivity (default: "https://api.github.com")
```

`CheckerConfig` is constructed from `AppConfig` at startup. It is not a
separate config.toml section; values are derived from `[builder]` and
hardcoded defaults.

Note: Disk space checking has been explicitly excluded from scope (CEO
decision). Disk capacity management is left to the operator.

### Singleton instance enforcement

Dockermint must not allow two instances to run concurrently, as they would
conflict on Docker builder resources and potentially corrupt the database.

Strategy: **PID file with advisory file lock**.

```
1. On startup, determine the lock file path using platform best practices:
   - Linux: $XDG_RUNTIME_DIR/dockermint.lock (falls back to /tmp/dockermint.lock)
   - macOS: ~/Library/Caches/dockermint/dockermint.lock
2. Attempt to create/open the lock file
3. Attempt an exclusive advisory lock (flock / fcntl)
4. If lock acquired: write PID to file, proceed
5. If lock fails: another instance is running, report error
6. On clean shutdown: release lock (automatic on process exit)
7. On crash: OS releases advisory lock, next startup succeeds
```

The lock file path follows platform-conventional paths and is NOT
configurable in config.toml (CEO decision). Advisory locks (not mandatory
locks) are used because they are portable across Linux and macOS and
automatically release on process termination.

### BuildKit builder lifecycle

On each Dockermint launch (CLI or Daemon), the checker module manages
BuildKit builder instances. This implements CEO decision Q3:

```
1. Resolve Docker host:
   - If config.toml [builder] docker_host is non-empty, use that URI
   - Otherwise, use the system default Docker context

2. Verify Docker context:
   - Run `docker context inspect` or `docker info` against the resolved host
   - If unreachable, report CheckStatus::Failed

3. For each target platform in config.toml [builder] platforms:
   - Map platform to builder name:
     "linux/amd64" -> "dockermint-amd64"
     "linux/arm64" -> "dockermint-arm64"
   - Check if builder exists: `docker buildx inspect <builder-name>`
   - If exists and healthy: BuilderStatus::Verified
   - If exists but unhealthy (wrong driver, wrong endpoint):
     Destroy: `docker buildx rm <builder-name>`
     Recreate: `docker buildx create --name <builder-name> --driver docker-container --platform <platform>`
     Result: BuilderStatus::Recreated
   - If does not exist:
     Create: `docker buildx create --name <builder-name> --driver docker-container --platform <platform>`
     Result: BuilderStatus::Created
   - If creation fails: BuilderStatus::Failed

4. If docker_host is specified, pass --buildkitd-flags or endpoint config
   to point builders at the correct Docker socket URI.
```

#### QEMU setup for cross-platform builds

Before builder lifecycle runs, the checker verifies that QEMU user-static
binaries are registered for cross-platform emulation. This is required for
BuildKit to produce images for architectures other than the host.

```
1. Check if QEMU binfmt_misc entries exist for target platforms
   (e.g., /proc/sys/fs/binfmt_misc/qemu-aarch64 on x86_64 host)
2. If missing or stale:
   Run: docker run --rm --privileged multiarch/qemu-user-static --reset -p yes
3. Verify registration succeeded
4. If setup fails: CheckStatus::Failed (cross-platform builds will not work)
```

QEMU setup is idempotent and safe to re-run. On macOS (aarch64-apple-darwin),
QEMU is not needed for arm64 builds (native) and x86_64 emulation is handled
by Docker Desktop's built-in Rosetta integration.

#### Builder lifecycle on shutdown

Builder destruction on shutdown is handled by the binary entrypoint (CLI or
daemon), not by the checker module. The checker only creates and verifies.

The shutdown behavior depends on the operating mode, with CLI flag overrides:

| Mode | Default behavior | Override flag |
| :--- | :--------------- | :------------ |
| CLI | Destroy builders at end of run | `--keep-builders` to preserve |
| Daemon | Keep builders running across cycles | `--destroy-builders` to clean up on shutdown |

The `destroy_builders()` function (defined in the checker module interface)
is called by the entrypoint when destruction is appropriate. The entrypoint
determines whether to call it based on mode defaults and CLI flag overrides.

### Per-requirement check functions

Each check is an independent function that returns `CheckStatus`:

| Check | Function | What it does |
| :---- | :------- | :----------- |
| Docker | `check_docker()` | Runs `docker info`, verifies daemon is responsive |
| BuildKit | `check_buildkit()` | Runs `docker buildx version`, verifies buildx plugin |
| Network | `check_network()` | HTTP HEAD request to `network_check_url` |
| QEMU | `check_and_setup_qemu()` | Verifies QEMU binfmt_misc registration, sets up if missing |
| Singleton | `acquire_lock()` | Attempts advisory file lock |

Checks are run sequentially in a defined order (singleton first, then docker,
buildkit, qemu, network). If singleton fails, no other checks run. If docker
fails, buildkit, qemu, and builder lifecycle are skipped.

### Error handling per mode

The checker module itself returns `CheckResult`. The mode-specific handler
interprets it:

| Mode | On any Failed check | Exit code |
| :--- | :------------------ | :-------- |
| CLI | Log error, print summary to stderr, exit | 4 (system check failure) |
| Daemon (startup) | Log error, halt daemon | N/A (process exits) |
| Daemon (per-cycle) | Log error, notify, skip this cycle | N/A (continues) |
| RPC | Log error, return gRPC error status | N/A (returns idle) |

Note: In daemon mode, the full check suite runs at startup. Per-cycle, only
lightweight checks run (builder health, network). The full suite (including
QEMU verification) is not repeated on every poll tick.

### Error types

```
CheckerError (thiserror)
  +-- DockerNotFound
  +-- DockerDaemonUnreachable { host: String, source: std::io::Error }
  +-- BuildKitNotAvailable { reason: String }
  +-- BuilderCreationFailed { name: String, reason: String }
  +-- BuilderDestructionFailed { name: String, reason: String }
  +-- NetworkUnreachable { url: String, source: Box<dyn std::error::Error + Send + Sync> }
  +-- QemuSetupFailed { reason: String, source: std::io::Error }
  +-- QemuNotAvailable { platform: String }
  +-- SingletonLockFailed { path: PathBuf, source: std::io::Error }
  +-- AnotherInstanceRunning { pid: u32 }
  +-- CommandFailed { command: String, source: std::io::Error }
```

### Dependencies

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `thiserror` | Error type definitions | `CheckerError` |
| `reqwest` | Network connectivity check | HTTP HEAD request (already used by VCS, registry, notifier) |

System-level operations (file lock, disk space query, process execution) use
`std::fs`, `std::process::Command`, and platform APIs. No additional crates
needed for these.

Delegate to @lead-dev: verify `reqwest` musl/aarch64 compatibility (already
evaluated for other modules, confirm no additional concerns for checker usage).

**Cross-compilation note**: Advisory file locking uses `flock(2)` on Linux and
`flock(2)` on macOS. Both are available on all 5 mandatory toolchains. The
`fs2` crate or raw `libc::flock` can provide cross-platform advisory locking.
Delegate to @lead-dev: evaluate `fs2` crate for advisory file locking, check
musl/aarch64 compatibility.

---

## Interface contract

```rust
/// Run all system prerequisite checks and return aggregated results.
///
/// Checks are run in order: singleton, docker, buildkit, qemu, network.
/// If singleton fails, remaining checks are skipped.
/// If docker fails, buildkit, qemu, and builder lifecycle are skipped.
///
/// # Arguments
///
/// * `config` - Checker configuration derived from AppConfig
///
/// # Returns
///
/// CheckResult containing the status of each check
///
/// # Errors
///
/// Returns CheckerError only for unrecoverable internal failures
/// (e.g., cannot execute any shell command at all). Individual check
/// failures are reported via CheckStatus::Failed within CheckResult.
pub fn run_checks(config: &CheckerConfig) -> Result<CheckResult, CheckerError>;

/// Manage BuildKit builder lifecycle: verify, create, or recreate
/// builders for each configured platform.
///
/// # Arguments
///
/// * `config` - Checker configuration with docker_host and platforms
///
/// # Returns
///
/// BuilderLifecycleResult with per-platform status
///
/// # Errors
///
/// Returns CheckerError if builder management encounters an
/// unrecoverable failure
pub fn manage_builders(
    config: &CheckerConfig,
) -> Result<BuilderLifecycleResult, CheckerError>;

/// Acquire the singleton instance lock.
///
/// # Returns
///
/// A guard that holds the lock. Dropping the guard releases the lock.
///
/// # Errors
///
/// Returns CheckerError::AnotherInstanceRunning if another Dockermint
/// process holds the lock, or CheckerError::SingletonLockFailed if the
/// lock file cannot be created.
pub fn acquire_singleton_lock() -> Result<SingletonGuard, CheckerError>;

/// Opaque guard. Holds the file lock. Dropping releases the lock.
pub struct SingletonGuard { /* private: File handle with advisory lock */ }

/// Destroy BuildKit builders created by Dockermint.
/// Called during clean shutdown.
///
/// # Arguments
///
/// * `config` - Checker configuration with platforms
///
/// # Errors
///
/// Returns CheckerError::BuilderDestructionFailed if a builder
/// cannot be removed (non-fatal, logged as warning)
pub fn destroy_builders(config: &CheckerConfig) -> Result<(), CheckerError>;
```

---

## Module interaction diagram

```
[config] ---> CheckerConfig (derived from BuilderConfig + hardcoded defaults)
                   |
                   v
            [checker/mod.rs]
                   |
     +------+------+------+------+-----------+
     |      |      |      |      |           |
     v      v      v      v      v           v
singleton docker buildkit qemu  network builder_lifecycle
     |      |      |      |      |           |
     v      v      v      v      v           v
  lock    docker  docker  binfmt HTTP     docker buildx
  file    info    buildx  misc   HEAD     create/inspect/rm
                          +QEMU
                          setup
                   |
                   v
              CheckResult
                   |
          +--------+--------+
          |                 |
          v                 v
     CLI (exit 4)      Daemon (log + notify + continue)
```

---

## Testing strategy

- **Unit tests**: Each check function returns correct `CheckStatus` for
  success and failure scenarios.
- **Unit tests**: `CheckResult` aggregation correctly identifies overall
  pass/fail.
- **Unit tests**: Builder name mapping (`linux/amd64` -> `dockermint-amd64`)
  is correct for all configured platforms.
- **Unit tests**: `CheckerError` variants format with meaningful messages.
- **Unit tests**: Singleton lock logic handles already-locked and
  unlocked scenarios.
- **Integration tests**: Full `run_checks()` against a Docker-capable
  environment (CI only, gated by environment flag).
- **Unit tests**: QEMU check correctly detects presence/absence of binfmt_misc
  entries. Setup function is invoked when entries are missing.
- **Unit tests**: Builder lifecycle shutdown behavior respects mode defaults
  (CLI=destroy, daemon=keep) and CLI flag overrides.
- **Mock**: Shell command execution for `docker info`, `docker buildx`, QEMU
  setup commands. Network requests for connectivity check. File system for
  lock file.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| C1 | What is the minimum required disk space (in GB) before builds are allowed? Default 10 GB proposed. | RESOLVED | NO disk space check. Requirement removed entirely. Disk management is the operator's responsibility. |
| C2 | Should the network connectivity check probe GitHub API specifically, or a generic endpoint? If the VCS backend changes from GitHub, the check URL should change too. | RESOLVED | YES, include network connectivity check. Default URL is `https://api.github.com`. When VCS backend is swappable, the check URL should correspond to the configured VCS endpoint. |
| C3 | Should builder destruction happen automatically on clean shutdown (CLI and daemon), or only on explicit cleanup command? | RESOLVED | CLI mode: destroy builders at end of run (default). Daemon mode: keep builders running (default). Both modes support `--keep-builders` and `--destroy-builders` CLI flags to override the default. |
| C4 | Should the singleton lock file path be configurable in config.toml, or use a platform-conventional path? | RESOLVED | Follow platform best practices. Linux: `$XDG_RUNTIME_DIR/dockermint.lock` (fallback `/tmp/dockermint.lock`). macOS: `~/Library/Caches/dockermint/dockermint.lock`. Not configurable in config.toml. |

### Additional decisions applied

| ID | Decision | Source |
| :- | :------- | :----- |
| B4 | QEMU setup for cross-platform builds is handled by the checker module. Checker verifies QEMU binfmt_misc is available and sets it up if needed before builder lifecycle runs. | CEO (from builder spec context) |
