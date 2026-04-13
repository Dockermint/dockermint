# Feature: Notification System (notifier)

## Context

The `notifier` module sends build status notifications to external channels.
In daemon and RPC modes, operators need real-time alerts when builds succeed,
fail, or encounter errors -- especially since the daemon runs unattended.

Notifications are part of the Unrecoverable Error Strategy for daemon mode:
dump, log, **notify**, persist, continue. The notifier fires before the
build result is persisted, ensuring operators learn about failures even if
the database write subsequently fails.

In CLI mode, notifications are not used -- the user is watching the terminal.

Roadmap entry: Phase 0 -- Foundation, spec "Notification system"
Architecture reference: `docs/specs/architecture-overview.md`, sections 2.2
(Notifier trait), 3.1 (notifier-telegram feature), 4.5 (secrets), 6.3
(daemon error strategy)

---

## Requirements

1. [confirmed] The notifier module exposes a `Notifier` trait that is
   swappable via feature gate. Default implementation: `TelegramNotifier`.
2. [confirmed] The trait is async (`Send + Sync` for shared state).
3. [confirmed] Three notification events: build start, build success, build
   failure.
4. [confirmed] Secrets (Telegram bot token, chat ID) are stored exclusively
   in `.env` and loaded via `dotenvy`. Wrapped with `secrecy` crate.
5. [confirmed] Non-secret configuration (enable/disable, notification level)
   lives in `config.toml` under `[notifier]`.
6. [confirmed] Rate limiting / batching to prevent notification spam when the
   daemon processes multiple builds in a single poll cycle.
7. [confirmed] Notification send failures must not crash the daemon. They are
   logged but do not interrupt the pipeline.
8. [confirmed] Each module owns its error type via `thiserror`.

---

## Architecture

### Module placement

```
src/notifier/
    mod.rs              -- Notifier trait + conditional re-export
    error.rs            -- NotifierError enum (thiserror)
    types.rs            -- BuildEvent, NotificationLevel, NotifierConfig
    telegram/
        mod.rs          -- TelegramNotifier implementation (behind notifier-telegram)
        message.rs      -- Message formatting / templates
```

### Trait design

The `Notifier` trait defines the contract for sending build status
notifications to an external channel. It is async because notification
delivery involves network I/O.

The trait is kept minimal: three methods, one per event type. Each method
receives a `BuildEvent` containing all context needed to format the message.
The trait does not prescribe message format -- implementations decide how to
render the event for their channel.

The trait requires `Send + Sync` for daemon shared state.

### Type design

#### BuildEvent

The data payload passed to every notification method. Contains all context
an implementation needs to format a human-readable message.

```
BuildEvent
    recipe_name: String             -- e.g., "cosmos-gaiad"
    chain_name: String              -- e.g., "Cosmos"
    version: String                 -- e.g., "v19.0.0"
    platforms: Vec<String>          -- e.g., ["linux/amd64", "linux/arm64"]
    flavors: BTreeMap<String, String>  -- Resolved flavor dimensions
    image_ref: Option<String>       -- Full image reference (on success/push)
    duration_secs: Option<u64>      -- Build duration (on completion)
    error_context: Option<String>   -- Error chain (on failure)
    pipeline_stage: Option<String>  -- Which stage failed (on failure)
    timestamp: DateTime<Utc>        -- Event timestamp
```

Derives: `Debug`, `Clone`.

#### NotificationLevel

Controls which events trigger notifications. Allows operators to suppress
noisy start notifications while keeping failure alerts.

```
enum NotificationLevel {
    /// Notify on all events: start, success, failure
    All,
    /// Notify on success and failure only
    Results,
    /// Notify on failure only
    FailuresOnly,
    /// Notifications disabled (module loaded but silent)
    None,
}
```

Derives: `Debug`, `Clone`, `PartialEq`, `Eq`, `serde::Serialize`,
`serde::Deserialize`. Default: `Results`.

#### NotifierConfig

Configuration struct deserialized from `[notifier]` in config.toml.

```
NotifierConfig
    enabled: bool                   -- Master switch (default: true)
    level: NotificationLevel        -- Which events to notify (default: Results)
    rate_limit_secs: u64            -- Minimum seconds between notifications (default: 10)
    batch_window_secs: u64          -- Window for batching multiple results (default: 30)
```

Derives: `Debug`, `Clone`, `serde::Deserialize`. Uses
`#[serde(deny_unknown_fields)]`.

### Feature gate

| Feature flag         | Module   | Default | What it activates |
| :------------------- | :------- | :------ | :---------------- |
| `notifier-telegram`  | notifier | Yes     | TelegramNotifier implementation of Notifier |

Compile-time check in `mod.rs`:

```
#[cfg(not(any(feature = "notifier-telegram")))]
compile_error!("At least one notifier backend must be enabled");
```

### Configuration

#### config.toml

The `[notifier]` section in config.toml is the **sole control** for
enabling/disabling notifications. There is no CLI flag (`--no-notify` or
similar). To disable notifications for a one-off daemon run, set
`enabled = false` in config.toml before starting.

```toml
[notifier]
enabled = true                  # Master switch (sole control for enable/disable)
level = "results"               # "all" | "results" | "failures_only" | "none"
rate_limit_secs = 10            # Min seconds between individual notifications
batch_window_secs = 30          # Batch results within this window into one message
```

#### .env variables

| Variable | Used by | Description |
| :------- | :------ | :---------- |
| `TELEGRAM_BOT_TOKEN` | TelegramNotifier | Telegram Bot API token |
| `TELEGRAM_CHAT_ID` | TelegramNotifier | Target chat/channel ID |

Both are required when `notifier-telegram` feature is active and
`[notifier] enabled = true`. Wrapped with `secrecy::SecretString` to prevent
accidental logging.

### Telegram bot integration

The `TelegramNotifier` sends messages via the Telegram Bot API
(`https://api.telegram.org/bot<token>/sendMessage`).

#### HTTP client

Uses `reqwest` (already a dependency for VCS and registry modules, per
architecture-overview.md Appendix C). A single `reqwest::Client` instance is
shared (connection pooling).

#### Message format

Messages use Telegram's Markdown formatting (parse_mode: "MarkdownV2").
Emoji is explicitly authorized by the CEO for notification messages (this is
an exception to the general no-emoji rule in CLAUDE.md, which permits emoji
in documentation contexts).

Message templates are hardcoded in `telegram/message.rs`. They are NOT
configurable (no template files, no config.toml templates). Each event type
has a distinct format designed to be accurate, well-sized, and readable in
Telegram:

**Build Start:**
```
&#128268; *Building* `cosmos-gaiad` v19.0.0
Platforms: linux/amd64, linux/arm64
Flavors: db=goleveldb, env=alpine3.23
```

**Build Success:**
```
&#9989; *Success* `cosmos-gaiad` v19.0.0
Duration: 4m 32s
Image: `ghcr.io/dockermint/cosmos-gaiad-goleveldb:v19.0.0-alpine3.23`
Platforms: linux/amd64, linux/arm64
```

**Build Failure:**
```
&#10060; *Failure* `cosmos-gaiad` v19.0.0
Stage: build
Error: BuildKit returned non-zero exit code (137)
Duration: 2m 15s
```

**Batched Results (when multiple builds complete within batch window):**
```
&#128230; *Batch* \- 5 builds completed
  &#9989; `cosmos-gaiad` v19.0.0 (4m 32s)
  &#9989; `kyve-kyved` v1.5.0 (3m 10s)
  &#10060; `osmosis-osmosisd` v25.0.0 \- Stage: push
  &#9989; `fetch-fetchd` v0.11.0 (5m 01s)
  &#9989; `axelar-axelard` v0.35.0 (6m 22s)
```

Key formatting decisions:
- Recipe names and image refs in backtick code spans for readability.
- Emoji prefixes for quick visual scan (authorized by CEO).
- Compact format -- no unnecessary verbosity.
- MarkdownV2 special characters properly escaped.

### Rate limiting / batching

The daemon may discover and build multiple new versions in a single poll
cycle. Sending one notification per build would spam the channel. The
notifier implements two mechanisms:

1. **Rate limiting**: enforces a minimum interval (`rate_limit_secs`) between
   individual notification sends. If a send is attempted before the interval
   has elapsed, the event is queued.

2. **Batching**: when multiple build results (success or failure) arrive
   within `batch_window_secs`, they are combined into a single batched
   message. The batch window starts when the first result arrives after the
   previous send.

Implementation approach:

- The notifier maintains an internal `tokio::sync::mpsc` channel.
- `notify_*` methods send events into the channel (non-blocking).
- A background task (`tokio::spawn`) drains the channel and applies rate
  limiting / batching logic before calling the Telegram API.
- The background task is spawned when the notifier is initialized and
  shut down gracefully with the daemon.

This ensures that `notify_*` calls from the pipeline never block on HTTP I/O.

### Event accumulation on API outage

When the Telegram API is unreachable for an extended period, the notifier
follows a bounded buffer strategy to prevent unbounded memory growth:

- The internal mpsc channel has a bounded capacity (recommended: 1000 events).
- When the buffer is full and a new event arrives, the oldest event is dropped
  (FIFO eviction).
- Dropped events are counted and logged via `tracing::warn!`.
- When connectivity is restored, the first message sent includes a note about
  the number of dropped events (e.g., "N notifications were dropped during
  outage").
- The batch window mechanism naturally compresses events, reducing buffer
  pressure during outages.

This bounded-buffer-with-drop approach follows standard best practices for
notification systems: it prevents OOM while preserving the most recent events
(which are typically most actionable).

### Error types

```
enum NotifierError {
    /// HTTP request to notification service failed
    Send { service: String, source: reqwest::Error },

    /// Notification service returned an error response
    Response { service: String, status: u16, body: String },

    /// Authentication failed (invalid token, expired, revoked)
    Auth { service: String, detail: String },

    /// Rate limited by the notification service (429)
    RateLimited { service: String, retry_after_secs: Option<u64> },

    /// Required secret not found in .env
    MissingSecret { variable: String },

    /// Message formatting failed
    Format { context: String, source: std::fmt::Error },

    /// Internal channel send failure (background task died)
    Channel { source: ... },
}
```

Notification errors are **never fatal** to the pipeline. The daemon logs
them via `tracing::error!` and continues. They are not persisted to the
database (the saver handles build failures, not notification failures).

---

## Interface contract

```rust
/// Trait for sending build status notifications.
///
/// Implementations must be `Send + Sync` for use in async daemon/RPC contexts.
/// The default implementation is `TelegramNotifier` behind the
/// `notifier-telegram` feature gate.
///
/// Notification delivery is best-effort. Implementations must not panic or
/// propagate errors that would halt the build pipeline.
pub trait Notifier: Send + Sync {
    /// Notify that a build has started.
    ///
    /// Respects `NotificationLevel` -- only fires if level is `All`.
    async fn notify_start(&self, event: &BuildEvent) -> Result<(), NotifierError>;

    /// Notify that a build completed successfully.
    ///
    /// Respects `NotificationLevel` -- fires for `All` and `Results`.
    async fn notify_success(&self, event: &BuildEvent) -> Result<(), NotifierError>;

    /// Notify that a build failed.
    ///
    /// Fires for all levels except `None`.
    async fn notify_failure(&self, event: &BuildEvent) -> Result<(), NotifierError>;
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
       [build start]      [build success]    [build failure]
              |                  |                  |
              v                  v                  v
    +-------------------+-------------------+-------------------+
    |                  Notifier trait                            |
    |  .notify_start()  .notify_success()  .notify_failure()    |
    +----------------------------+------------------------------+
                                 |
                                 v
                    +------------------------+
                    | internal mpsc channel  |
                    +------------------------+
                                 |
                                 v
                    +------------------------+
                    | background send task   |
                    | (rate limit + batch)   |
                    +------------------------+
                                 |
                                 v
                    +------------------------+
                    | Telegram Bot API       |
                    | (reqwest HTTP POST)    |
                    +------------------------+

    Error flow:
    Telegram API failure --> tracing::error! --> continue
    (notification errors never halt the pipeline)
```

---

## Testing strategy

### Unit tests

- BuildEvent construction and field access.
- NotificationLevel filtering logic (which events pass each level).
- NotifierConfig deserialization from TOML (valid, invalid, missing fields).
- Message formatting for each event type (start, success, failure, batch).
- MarkdownV2 escaping of special characters in chain names / versions.
- Rate limiting logic: events within window are held, events after window
  are sent.
- Batching logic: multiple events within batch window produce a single
  batched message.

### Integration tests

- TelegramNotifier initialization with valid secrets.
- TelegramNotifier initialization fails gracefully with missing secrets.
- End-to-end send with a mock HTTP server (using `wiremock` or similar).
- Verify correct Telegram API endpoint, headers, and payload format.
- Verify rate limiting actually delays sends (time-based test).

### Mocking

- Mock `Notifier` trait for testing pipeline orchestration without real
  Telegram API calls.
- Mock `reqwest` HTTP layer (via `wiremock`) for TelegramNotifier integration
  tests.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| N1 | Should message templates be configurable (config.toml or template file) or are hardcoded templates sufficient? | RESOLVED | NO configurable templates. Messages are hardcoded in `telegram/message.rs` using Markdown format with emoji (CEO explicitly authorized emoji in notifications). Messages must be accurate and well-sized. |
| N2 | Should the notifier support disabling via CLI flag (`--no-notify`) for one-off daemon runs, or is `config.toml` the only control? | RESOLVED | NO dedicated CLI flag. Notification is controlled entirely by `[notifier] enabled = true/false` in config.toml. |
| N3 | If the Telegram API is unreachable for an extended period, should the notifier accumulate events in memory (risk OOM) or drop old events after a threshold? | RESOLVED | Follow best practices: bounded buffer (recommended 1000 events) with FIFO eviction when full. Dropped events are counted and reported when connectivity is restored. |

---

## Dependencies

| Crate | Use case | Status |
| :---- | :------- | :----- |
| `reqwest` | HTTP client for Telegram Bot API | Already listed in architecture-overview.md conventions (shared dependency) |
| `secrecy` | Wrap bot token and chat ID | Already listed in architecture-overview.md section 4.5 |
| `dotenvy` | Load secrets from .env | Already listed in architecture-overview.md section 4.5 |
| `tokio` | mpsc channel, spawn background task | Already listed in architecture-overview.md conventions |
| `chrono` | Timestamps in BuildEvent | Delegate to @lead-dev if not already evaluated for saver module |
| `thiserror` | NotifierError definition | Already listed in architecture-overview.md conventions |
| `wiremock` | HTTP mock server for integration tests | Delegate to @lead-dev: evaluate wiremock for HTTP integration testing, dev-dependency only |
