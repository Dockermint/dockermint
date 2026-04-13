# Feature: Build State Persistence (saver)

## Context

The `saver` module provides build state persistence for Dockermint. In daemon
and RPC modes, every build result (success or failure) must be recorded so that
operators can audit build history, diagnose failures, and avoid rebuilding
versions that have already been processed.

In CLI mode, persistence is optional -- the CLI can operate without a database
for one-shot local builds.

Roadmap entry: Phase 0 -- Foundation, spec "Build state persistence"
Architecture reference: `docs/specs/architecture-overview.md`, sections 2.1,
2.2 (BuildStore trait), 3.1 (db-redb feature), 6.3 (daemon error strategy)

---

## Requirements

1. [confirmed] The saver module exposes a `BuildStore` trait that is swappable
   via feature gate. Default implementation: `RedbStore` (redb).
2. [confirmed] The trait must be `Send + Sync` for shared state in daemon/RPC.
3. [confirmed] Build results include: recipe name, chain name, version (tag),
   resolved flavors, build status, timestamps (start, end), error context,
   target platforms, image reference.
4. [confirmed] The module supports queries: list all builds, filter by
   chain/status/date range.
5. [confirmed] Daemon mode registers failures via `save_failure` -- this is
   part of the Unrecoverable Error Strategy (dump, log, notify, persist,
   continue).
6. [confirmed] CLI mode: persistence is optional. If no database feature is
   active, the pipeline skips the save step.
7. [confirmed] Database, notifier, and metrics features must be active for
   daemon/RPC mode (architecture-overview.md section 3.3).
8. [confirmed] Each module owns its error type via `thiserror`.

---

## Architecture

### Module placement

```
src/saver/
    mod.rs              -- BuildStore trait + conditional re-export
    error.rs            -- StoreError enum (thiserror)
    types.rs            -- BuildRecord, BuildStatus, BuildQuery, FailureRecord
    redb/
        mod.rs          -- RedbStore implementation (behind db-redb feature)
```

### Trait design

The `BuildStore` trait is the public contract for all persistence backends.
It follows the pattern established in architecture-overview.md section 2.1.

The trait must support:

- Saving a completed build result (success or failure).
- Saving an explicit failure record (for daemon error strategy).
- Querying build history with filters.
- Checking whether a specific version has already been built (to avoid
  re-processing in daemon mode).
- Retention cleanup (deleting records older than a threshold).

The trait is generic over its error type via an associated type, but all
implementations map to `StoreError` at the module boundary. The trait
requires `Send + Sync` to be shareable across async tasks in the daemon.

### Type design

#### BuildRecord

The primary data structure persisted by the saver. Captures the full context
of a single build execution.

```
BuildRecord
    id: BuildId                     -- Newtype over u64 (auto-incremented)
    recipe_name: String             -- e.g., "cosmos-gaiad"
    chain_name: String              -- e.g., "Cosmos"
    version: String                 -- e.g., "v19.0.0"
    flavors: BTreeMap<String, String>  -- Resolved flavor dimensions
    platforms: Vec<String>          -- e.g., ["linux/amd64", "linux/arm64"]
    status: BuildStatus             -- Enum: Success, Failure, InProgress
    image_ref: Option<String>       -- Full image reference if pushed
    started_at: DateTime<Utc>       -- Build start timestamp
    finished_at: Option<DateTime<Utc>>  -- Build end timestamp
    duration_secs: Option<u64>      -- Computed duration
    error_context: Option<String>   -- Error chain as string (on failure)
    pipeline_stage: Option<String>  -- Which stage failed (on failure)
```

#### BuildStatus

```
enum BuildStatus {
    InProgress,
    Success,
    Failure,
}
```

Derives: `Debug`, `Clone`, `PartialEq`, `Eq`, `serde::Serialize`,
`serde::Deserialize`.

#### BuildQuery

Filter struct for querying build history.

```
BuildQuery
    recipe_name: Option<String>
    chain_name: Option<String>
    status: Option<BuildStatus>
    since: Option<DateTime<Utc>>
    until: Option<DateTime<Utc>>
    limit: Option<usize>
    offset: Option<usize>
```

All fields optional. An empty `BuildQuery` returns all records subject to a
default limit of 100. The `limit` field accepts values up to a hard maximum
of 10000 to prevent unbounded memory allocation.

#### FailureRecord

A subset of `BuildRecord` focused on the failure context. Used by the daemon
error strategy to quickly persist failure details before continuing.

```
FailureRecord
    recipe_name: String
    chain_name: String
    version: String
    pipeline_stage: String          -- e.g., "build", "push", "template"
    error_context: String           -- Full error chain
    timestamp: DateTime<Utc>
```

#### BuildId

Newtype wrapper for type safety.

```
struct BuildId(u64);
```

Derives: `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`,
`serde::Serialize`, `serde::Deserialize`.

### Feature gate

| Feature flag | Module | Default | What it activates |
| :----------- | :----- | :------ | :---------------- |
| `db-redb`    | saver  | Yes     | RedbStore implementation of BuildStore |

As per architecture-overview.md section 3.4, the module's `mod.rs` includes a
compile-time check:

```
#[cfg(not(any(feature = "db-redb")))]
compile_error!("At least one database backend must be enabled");
```

Future alternative backends (e.g., SQLite, PostgreSQL) would add new feature
flags following the same pattern.

### Configuration

#### config.toml

No new config.toml section is needed for the saver. The database is an
internal implementation detail -- redb is embedded and requires no connection
string or credentials.

The database file path is derived from the general config:

```
Data directory: {log_dir}/../data/ or a dedicated [saver] section
```

The saver data directory follows platform-appropriate best practices (XDG
Base Directory Specification on Linux, platform conventions elsewhere):

- Linux: `$XDG_DATA_HOME/dockermint/` (defaults to `~/.local/share/dockermint/`)
- macOS: `~/Library/Application Support/dockermint/`

The data directory is configurable via `[saver]` in config.toml to override
the XDG/platform default:

```toml
[saver]
data_dir = "/var/lib/dockermint"       # Override platform default (optional)
retention_days = 90                     # Delete records older than N days (0 = keep forever)
```

If `data_dir` is not set in config.toml, the platform-appropriate default is
used. The `data_dir` key is `Option<PathBuf>` in the config struct.

#### .env variables

None. The database is embedded and has no credentials.

### Database schema design (redb key-value structure)

redb is a pure-Rust embedded key-value store. It uses typed tables where keys
and values are serialized via redb's `Key` and `Value` traits.

#### Tables

```
Table: "builds"
    Key:   BuildId (u64)
    Value: BuildRecord (serialized via bincode)

Table: "builds_by_recipe"
    Key:   (recipe_name: String, BuildId: u64)   -- compound key
    Value: ()                                     -- index only

Table: "builds_by_status"
    Key:   (status: u8, BuildId: u64)            -- compound key
    Value: ()                                     -- index only

Table: "builds_by_time"
    Key:   (timestamp_secs: u64, BuildId: u64)   -- compound key
    Value: ()                                     -- index only

Table: "version_check"
    Key:   (recipe_name: String, version: String) -- compound key
    Value: BuildStatus (u8)                        -- latest status for dedup

Table: "failures"
    Key:   u64                                    -- auto-incremented
    Value: FailureRecord (serialized)

Table: "metadata"
    Key:   String                                 -- e.g., "schema_version", "next_build_id"
    Value: Vec<u8>                                -- flexible value
```

The `version_check` table enables O(1) lookups to determine whether a version
has already been built, which is the hot path in daemon polling mode.

Secondary index tables (`builds_by_recipe`, `builds_by_status`,
`builds_by_time`) are maintained transactionally alongside the primary
`builds` table to support filtered queries without full table scans.

#### Schema versioning

The `metadata` table stores a `schema_version` key. On startup, the
`RedbStore` checks this version:

- If missing (fresh database): initialize all tables, write current schema version.
- If matching: proceed normally.
- If older: run migration functions in sequence.
- If newer: refuse to open (binary is too old for this database).

### Retention / cleanup strategy

- Configurable via `retention_days` in `[saver]` config section.
- Default: 90 days. 0 means keep forever.
- Cleanup runs at daemon startup and once per day during daemon operation.
- Cleanup is a single write transaction that:
  1. Scans `builds_by_time` for entries older than the threshold.
  2. Deletes matching entries from all tables (primary + indices).
  3. Deletes matching entries from `failures` table.
- Cleanup is non-blocking: runs in a `tokio::task::spawn_blocking` to avoid
  blocking the async runtime.

### Error types

```
enum StoreError {
    /// Database could not be opened or created
    Open { path: PathBuf, source: ... },

    /// Write transaction failed
    Write { operation: String, source: ... },

    /// Read transaction failed
    Read { operation: String, source: ... },

    /// Record not found
    NotFound { id: BuildId },

    /// Schema version mismatch -- database is newer than binary
    SchemaMismatch { expected: u32, found: u32 },

    /// Serialization or deserialization of a record failed
    Serialization { context: String, source: ... },

    /// Data directory does not exist or is not writable
    DataDir { path: PathBuf, source: std::io::Error },
}
```

Each variant wraps its source error for chaining. Application-level code wraps
`StoreError` with `anyhow::Context` to add recipe/version context.

---

## Interface contract

```rust
/// Trait for persisting and querying build state.
///
/// Implementations must be `Send + Sync` for use in async daemon/RPC contexts.
/// The default implementation is `RedbStore` behind the `db-redb` feature gate.
pub trait BuildStore: Send + Sync {
    /// Persist a completed build record.
    ///
    /// Returns the assigned BuildId.
    fn save_build(&self, record: &BuildRecord) -> Result<BuildId, StoreError>;

    /// Update an existing build record (e.g., mark InProgress -> Success).
    fn update_build(&self, id: BuildId, record: &BuildRecord) -> Result<(), StoreError>;

    /// Retrieve a build record by ID.
    fn get_build(&self, id: BuildId) -> Result<BuildRecord, StoreError>;

    /// Query build records with optional filters.
    ///
    /// Returns records ordered by start time (most recent first).
    fn list_builds(&self, query: &BuildQuery) -> Result<Vec<BuildRecord>, StoreError>;

    /// Check if a version has already been built for a given recipe.
    ///
    /// Returns the status of the most recent build, or None if never built.
    ///
    /// The daemon uses this to decide whether to build a version:
    /// - `None` -> build it (never attempted)
    /// - `Some(Success)` -> skip it (already built)
    /// - `Some(Failure)` -> skip by default; build if `--retry-failed` flag is active
    /// - `Some(InProgress)` -> skip it (build in progress)
    fn check_version(
        &self,
        recipe_name: &str,
        version: &str,
    ) -> Result<Option<BuildStatus>, StoreError>;

    /// Persist a failure record (daemon error strategy).
    fn save_failure(&self, failure: &FailureRecord) -> Result<(), StoreError>;

    /// List failure records, optionally filtered by recipe name.
    fn list_failures(
        &self,
        recipe_name: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<FailureRecord>, StoreError>;

    /// Delete records older than the given threshold.
    ///
    /// Returns the number of records deleted.
    fn cleanup(&self, older_than: DateTime<Utc>) -> Result<usize, StoreError>;
}
```

---

## Module interaction diagram

```
                        +-------------------+
                        |   daemon loop     |
                        |  (pipeline orch.) |
                        +--------+----------+
                                 |
              +------------------+------------------+
              |                  |                  |
              v                  v                  v
    [build success]      [build failure]     [poll cycle]
              |                  |                  |
              v                  v                  v
    +-------------------+  +-------------------+  +-------------------+
    | BuildStore        |  | BuildStore        |  | BuildStore        |
    | .save_build()     |  | .save_build()     |  | .check_version() |
    |   status=Success  |  |   status=Failure  |  |   (dedup check)  |
    +-------------------+  | .save_failure()   |  +-------------------+
                           +-------------------+
                                 |
                                 v
                           +-------------------+
                           | Notifier          |
                           | .notify_failure() |
                           +-------------------+

    Daily cleanup (daemon only):
    +-------------------+
    | BuildStore        |
    | .cleanup()        |
    +-------------------+
```

---

## Testing strategy

### Unit tests

- Serialization / deserialization round-trip for all data types (BuildRecord,
  FailureRecord, BuildStatus, BuildQuery).
- BuildQuery filtering logic (each filter field independently and combined).
- BuildId newtype operations.
- Schema version comparison logic (match, older, newer).

### Integration tests

- RedbStore: open database, save record, retrieve by ID, verify fields.
- RedbStore: save multiple records, query with filters, verify ordering.
- RedbStore: check_version returns correct status for built/unbuilt versions.
- RedbStore: save_failure and list_failures round-trip.
- RedbStore: cleanup deletes old records but preserves recent ones.
- RedbStore: schema migration from version N to N+1 (when migrations exist).
- RedbStore: concurrent read/write from multiple tasks (Send + Sync
  verification).

### Mocking

- Mock `BuildStore` trait for testing pipeline orchestration without a real
  database.
- Use `tempdir` for integration tests to avoid test pollution.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| S1 | Should the saver data directory be a dedicated `[saver]` config section or derived from a convention relative to `log_dir`? | RESOLVED | Follow best practices: use XDG Base Directory / platform-appropriate defaults. `[saver] data_dir` is an optional override in config.toml. |
| S2 | Should the daemon retry a failed build on the next poll cycle, or skip versions that have a `Failure` status? If retry, how many attempts before permanent skip? | RESOLVED | NO automatic retry. Default behavior: skip failed builds. If daemon is started with `--retry-failed` flag, retry previously failed versions. If notifier is enabled, notify on failure regardless. No max attempt limit -- `--retry-failed` retries all failed versions each cycle. |
| S3 | What serialization format for redb values: bincode (fast, compact, no schema evolution) or rmp-serde (MessagePack, supports schema evolution)? | RESOLVED | Use bincode (fastest option). CEO directive: use the fastest serialization. Bincode is typically faster and more compact than rmp-serde. Delegate to @lead-dev for final crate evaluation. |
| S4 | Should `list_builds` have a hard maximum limit to prevent unbounded memory allocation on large databases? If so, what value? | RESOLVED | Follow best practices: `list_builds` applies a default limit when `BuildQuery.limit` is `None`. Recommended default: 100. Callers can set explicit limits up to a hard maximum (recommended: 10000) to prevent unbounded allocation. |

---

## Dependencies

| Crate | Use case | Status |
| :---- | :------- | :----- |
| `redb` | Embedded key-value database (default BuildStore impl) | Delegate to @lead-dev: evaluate redb for embedded persistence, check latest version, API surface, musl/aarch64 compatibility |
| `bincode` | Record serialization within redb values (fastest option per CEO decision) | Delegate to @lead-dev: evaluate bincode for redb value serialization, check latest version, API surface, musl/aarch64 compatibility |
| `chrono` | DateTime<Utc> for timestamps | Delegate to @lead-dev: evaluate chrono for timestamp handling, musl compatibility |
| `thiserror` | StoreError definition | Already listed in architecture-overview.md conventions |
