# Feature: Metrics Server (metrics)

## Context

The `metrics` module exposes operational metrics for Dockermint in daemon and
RPC modes. Metrics enable operators to monitor build throughput, failure rates,
build durations, and system health via external monitoring systems
(Prometheus + Grafana or similar).

In CLI mode, metrics are not used -- the user observes progress directly via
terminal output.

The metrics server runs alongside the daemon's polling loop within the same
tokio runtime, exposing an HTTP endpoint that Prometheus scrapes on a
configurable interval.

Roadmap entry: Phase 0 -- Foundation, spec "Metrics server"
Architecture reference: `docs/specs/architecture-overview.md`, sections 2.2
(MetricsExporter trait), 3.1 (metrics-prometheus feature), 1.3 (mode-specific
behavior)

---

## Requirements

1. [confirmed] The metrics module exposes a `MetricsExporter` trait that is
   swappable via feature gate. Default implementation: `PrometheusExporter`.
2. [confirmed] The trait records build lifecycle events and exposes them for
   scraping.
3. [confirmed] Metrics to expose: builds total (counter), builds by status
   (counter), build duration (histogram), active builds (gauge), last poll
   time (gauge), errors by category (counter).
4. [confirmed] Labels: chain, tags, status. No flavor dimensions as labels
   (avoids cardinality explosion). No version as label (unbounded). Flavor
   and version analysis available via saver query interface.
5. [confirmed] The Prometheus HTTP scrape endpoint is served by an axum server.
6. [confirmed] The metrics server shares the daemon's tokio runtime -- it does
   not spawn a separate runtime.
7. [confirmed] Port and bind address are configurable in config.toml under
   `[metrics]`.
8. [confirmed] Each module owns its error type via `thiserror`.

---

## Architecture

### Module placement

```
src/metrics/
    mod.rs              -- MetricsExporter trait + conditional re-export
    error.rs            -- MetricsError enum (thiserror)
    types.rs            -- MetricsConfig, metric name constants
    prometheus/
        mod.rs          -- PrometheusExporter implementation (behind metrics-prometheus)
        server.rs       -- axum HTTP server for /metrics endpoint
```

### Trait design

The `MetricsExporter` trait defines the contract for recording build pipeline
events. It is deliberately synchronous -- metric recording is an in-memory
operation (incrementing counters, observing histograms) that must not block.

The trait also provides a method to start the scrape server, which is async
because it binds to a network socket.

The trait requires `Send + Sync` because it is shared across async tasks.

### Type design

#### MetricsConfig

Configuration struct deserialized from `[metrics]` in config.toml.

```
MetricsConfig
    enabled: bool                   -- Master switch (default: true)
    listen_address: SocketAddr      -- Bind address for scrape endpoint (default: 0.0.0.0:9100)
    path: String                    -- HTTP path for scrape endpoint (default: "/metrics")
```

Derives: `Debug`, `Clone`, `serde::Deserialize`. Uses
`#[serde(deny_unknown_fields)]`.

#### Metric definitions

All metrics use a `dockermint_` prefix to avoid collision with other
exporters on the same host.

| Metric name | Type | Labels | Description |
| :---------- | :--- | :----- | :---------- |
| `dockermint_builds_total` | Counter | chain, tags, status | Total builds started, partitioned by chain, tags, and final status |
| `dockermint_builds_success_total` | Counter | chain, tags | Total successful builds |
| `dockermint_builds_failure_total` | Counter | chain, tags, stage | Total failed builds, partitioned by failure stage |
| `dockermint_build_duration_seconds` | Histogram | chain, tags, status | Build duration distribution |
| `dockermint_builds_active` | Gauge | chain, tags | Currently in-progress builds |
| `dockermint_poll_last_timestamp_seconds` | Gauge | -- | Unix timestamp of last completed poll cycle |
| `dockermint_poll_duration_seconds` | Histogram | -- | Duration of each poll cycle |
| `dockermint_errors_total` | Counter | category | Errors by category (config, vcs, build, push, store, notify) |
| `dockermint_recipes_configured` | Gauge | -- | Number of configured recipes |
| `dockermint_scrape_requests_total` | Counter | -- | Number of times /metrics was scraped |

#### Label strategy

Labels must have bounded cardinality to avoid metric explosion:

- `chain`: recipe name (e.g., "cosmos-gaiad") -- bounded by number of recipes.
- `tags`: recipe tags from recipe metadata (e.g., "cosmos", "l1") -- bounded
  by tag vocabulary.
- `status`: "success", "failure", "in_progress" -- 3 values.
- `stage`: pipeline stage that failed (e.g., "build", "push") -- bounded by
  number of stages (~6).
- `category`: error category -- bounded by number of module error types (~8).

The `version` and `flavor` labels are intentionally excluded from Prometheus
metrics to avoid cardinality explosion. Version-level and flavor-level detail
is available via the `saver` module's query interface.

Per CEO decision, flavor dimensions are NOT included as Prometheus labels.
Instead, the `tags` label (from recipe metadata) and `chain` label provide
sufficient grouping. All flavor-specific analysis goes through the saver
module's query interface, not Prometheus.

### Feature gate

| Feature flag           | Module  | Default | What it activates |
| :--------------------- | :------ | :------ | :---------------- |
| `metrics-prometheus`   | metrics | Yes     | PrometheusExporter implementation of MetricsExporter |

Compile-time check in `mod.rs`:

```
#[cfg(not(any(feature = "metrics-prometheus")))]
compile_error!("At least one metrics backend must be enabled");
```

### Configuration

#### config.toml

```toml
[metrics]
enabled = true                          # Master switch
listen_address = "0.0.0.0:9100"        # Bind address for scrape endpoint
path = "/metrics"                       # HTTP path
```

#### .env variables

None. Metrics exposure has no secrets.

### Prometheus HTTP server

The `PrometheusExporter` starts an axum HTTP server that serves the
Prometheus exposition format at the configured path.

#### Server lifecycle

1. **Startup**: called from the daemon binary's `main()` after module
   initialization. Spawned as a `tokio::spawn` task on the shared runtime.
2. **Running**: serves GET requests at `{listen_address}{path}`. Each request
   gathers all registered metrics from the Prometheus registry and renders
   them in text exposition format.
3. **Shutdown**: the server task is aborted during graceful daemon shutdown
   via a `tokio::sync::watch` or `CancellationToken` signal.

#### axum routes

```
GET {path}  -->  render Prometheus text exposition format
GET /health -->  return 200 OK (liveness probe)
```

The `/health` endpoint is part of the metrics module's HTTP server (best
practice: co-locate health checks with the metrics scrape endpoint since
they share the same axum server and network exposure). The health endpoint
returns 200 OK when the daemon is running and responsive. No authentication
is required on either endpoint -- access control is handled at the network
level (firewall, network policy) per standard Prometheus deployment practice.

The server follows CLAUDE.md's axum guidelines: async handlers returning
`Result<Response, AppError>`, tower middleware for timeouts, no global
mutable state.

### How metrics server runs alongside daemon

```
daemon main()
    |
    +-- load config
    +-- initialize modules (saver, notifier, metrics)
    +-- spawn metrics server task    <-- tokio::spawn
    +-- enter polling loop
    |       |
    |       +-- [poll] metrics.record_poll_start()
    |       +-- [build] metrics.record_build_start()
    |       +-- [done]  metrics.record_build_success() or record_build_failure()
    |       +-- [poll]  metrics.record_poll_complete()
    |
    +-- on shutdown signal: cancel metrics server task
```

The metrics exporter instance is shared between the polling loop (which calls
`record_*` methods) and the axum server (which reads the registry). The
`prometheus` crate's `Registry` is internally `Arc`-wrapped and thread-safe.

### Error types

```
enum MetricsError {
    /// Failed to bind the HTTP server to the configured address
    ServerBind { address: SocketAddr, source: std::io::Error },

    /// Failed to register a metric with the Prometheus registry
    Registration { metric: String, source: prometheus::Error },

    /// Failed to render metrics in exposition format
    Render { source: std::fmt::Error },

    /// Configuration error (invalid address format, etc.)
    Config { detail: String },
}
```

`MetricsError::ServerBind` is a startup-time error that prevents the daemon
from starting (per daemon error strategy: startup failures cause exit).

`MetricsError::Registration` is also a startup-time error.

`MetricsError::Render` occurs during scrape requests and is returned as an
HTTP 500 to the scraper. It is logged but does not affect the build pipeline.

---

## Interface contract

```rust
/// Trait for recording and exposing build pipeline metrics.
///
/// Implementations must be `Send + Sync` for use across async tasks.
/// The default implementation is `PrometheusExporter` behind the
/// `metrics-prometheus` feature gate.
///
/// Recording methods are synchronous and non-blocking (in-memory operations).
/// The server method is async because it binds to a network socket.
pub trait MetricsExporter: Send + Sync {
    /// Start the metrics scrape server.
    ///
    /// This method spawns an HTTP server task and returns a handle
    /// that can be used to shut it down.
    ///
    /// Called once during daemon startup.
    async fn start_server(
        &self,
        config: &MetricsConfig,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<tokio::task::JoinHandle<()>, MetricsError>;

    /// Record that a build has started.
    fn record_build_start(&self, chain: &str);

    /// Record that a build completed successfully.
    fn record_build_success(&self, chain: &str, duration_secs: f64);

    /// Record that a build failed.
    fn record_build_failure(&self, chain: &str, stage: &str, duration_secs: f64);

    /// Record that a poll cycle completed.
    fn record_poll_complete(&self, duration_secs: f64);

    /// Record an error by category.
    fn record_error(&self, category: &str);

    /// Set the number of configured recipes (called at startup and on reload).
    fn set_recipes_configured(&self, count: usize);
}
```

---

## Module interaction diagram

```
    +-------------------+          +-------------------+
    | daemon polling    |          | Prometheus        |
    | loop              |          | (external scraper)|
    +--------+----------+          +--------+----------+
             |                              |
             |  record_build_start()        |  GET /metrics
             |  record_build_success()      |
             |  record_build_failure()      |
             |  record_poll_complete()      |
             |  record_error()              |
             v                              v
    +---------------------------------------------------+
    |            MetricsExporter                         |
    |                                                    |
    |   +------------------+    +--------------------+   |
    |   | Prometheus       |    | axum HTTP server   |   |
    |   | Registry         |<---| (serves /metrics)  |   |
    |   | (counters,       |    +--------------------+   |
    |   |  histograms,     |                             |
    |   |  gauges)         |                             |
    |   +------------------+                             |
    +---------------------------------------------------+
             |
             | (both run on same tokio runtime)
             v
    +---------------------------------------------------+
    |              tokio runtime                         |
    |  [polling loop task]  [metrics server task]       |
    +---------------------------------------------------+
```

---

## Testing strategy

### Unit tests

- MetricsConfig deserialization from TOML (valid, invalid, missing fields).
- Default values for MetricsConfig fields.
- Metric name constants are correctly prefixed with `dockermint_`.
- Label cardinality: verify label values are from the bounded set.

### Integration tests

- PrometheusExporter: register all metrics, record events, render output,
  verify Prometheus text format is valid.
- PrometheusExporter: start server, send HTTP GET to /metrics, verify
  response status 200 and content-type.
- PrometheusExporter: start server, send HTTP GET to /health, verify 200 OK.
- PrometheusExporter: record multiple builds, verify counter values in
  scraped output.
- PrometheusExporter: shutdown signal causes server task to terminate.
- PrometheusExporter: verify record_* methods do not panic or block on
  concurrent calls from multiple tasks.

### Mocking

- Mock `MetricsExporter` trait for testing pipeline orchestration without
  real Prometheus infrastructure.
- Use `reqwest` or `hyper` client in integration tests to query the server.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| M1 | Should any flavor dimensions (e.g., `db_backend`) be included as Prometheus labels, or should all flavor-level analysis go through the saver? | RESOLVED | NO flavor dimensions as Prometheus labels. Use `tags` (recipe metadata) and `chain` as labels instead. Flavor-level analysis goes through the saver query interface. Avoids cardinality explosion. |
| M2 | Should the metrics endpoint require authentication (e.g., basic auth), or is network-level access control (firewall) sufficient? | RESOLVED | No auth on the metrics endpoint. Standard Prometheus practice: secure at the network level (firewall, network policy). |
| M3 | Should the /health endpoint be part of the metrics module or a separate concern? | RESOLVED | Follow best practices: `/health` is part of the metrics module's axum HTTP server. Co-locating health checks with the scrape endpoint is standard practice since they share the same server and network exposure. |

---

## Dependencies

| Crate | Use case | Status |
| :---- | :------- | :----- |
| `prometheus` | Prometheus client library (Registry, Counter, Histogram, Gauge) | Delegate to @lead-dev: evaluate prometheus crate for metrics collection, check latest version, API surface, musl/aarch64 compatibility |
| `axum` | HTTP server for /metrics endpoint | Already listed in CLAUDE.md preferred tools |
| `tower` | Middleware (timeouts) for axum server | Already listed in CLAUDE.md preferred tools |
| `tokio` | Runtime, spawn, watch channel for shutdown | Already listed in architecture-overview.md conventions |
| `thiserror` | MetricsError definition | Already listed in architecture-overview.md conventions |
