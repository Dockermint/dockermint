# Feature Flags Reference

Dockermint uses Cargo feature flags to select backend implementations at
compile time. Every replaceable module is gated behind a feature. At least one
feature per module category must be enabled — if none is compiled in, the
build fails with a descriptive `compile_error!` message.

---

## Default Feature Set

Unless you pass `--no-default-features`, the following features are compiled:

```toml
[features]
default = [
    "redb",
    "telegram",
    "github",
    "native-tls",
    "oci",
    "buildkit",
    "prometheus",
]
```

This mirrors the table in CLAUDE.md and represents the standard production
build.

---

## Feature Reference

### Database backends

| Feature | Crate        | Implements     | Description                            |
| :------ | :----------- | :------------- | :------------------------------------- |
| `redb`  | `redb = "2"` | `Database`     | Embedded key-value store via redb      |

**Type alias when enabled:** `saver::SelectedDatabase = saver::redb::RedbDatabase`

At least one database feature must be enabled.

---

### Notification backends

| Feature    | Crates        | Implements | Description                          |
| :--------- | :------------ | :--------- | :----------------------------------- |
| `telegram` | _(via reqwest)_| `Notifier` | Telegram Bot API notification        |

**Type alias when enabled:** `notifier::SelectedNotifier = notifier::telegram::TelegramNotifier`

At least one notifier feature must be enabled.

Required secrets (in `.env`):
- `TELEGRAM_TOKEN`
- `TELEGRAM_CHAT_ID`

---

### VCS backends

| Feature  | Crates          | Implements             | Description              |
| :------- | :-------------- | :--------------------- | :----------------------- |
| `github` | _(via reqwest)_ | `VersionControlSystem` | GitHub REST API client   |

**Type alias when enabled:** `scrapper::SelectedVcs = scrapper::github::GithubClient`

At least one VCS feature must be enabled.

Optional secrets for authenticated requests (higher rate limits):
- `GH_USER`
- `GH_PAT`

---

### TLS backends

Exactly one TLS feature must be selected for HTTPS support (used by `reqwest`).

| Feature       | Underlying crate        | Description                               |
| :------------ | :---------------------- | :---------------------------------------- |
| `native-tls`  | `reqwest/native-tls`    | OS-provided TLS (OpenSSL on Linux, SChannel on Windows, Secure Transport on macOS) |
| `rustls-tls`  | `reqwest/rustls-tls`    | Pure-Rust TLS; useful for musl static builds |

Use `rustls-tls` when targeting `*-musl` toolchains to avoid OpenSSL link
dependencies.

---

### Registry backends

| Feature | Crates           | Implements       | Description                         |
| :------ | :--------------- | :--------------- | :---------------------------------- |
| `oci`   | _(via docker CLI)_| `RegistryClient`| OCI registry push via `docker push` |

**Type alias when enabled:** `push::SelectedRegistry = push::oci::OciRegistry`

At least one registry feature must be enabled.

---

### Builder backends

| Feature    | Crates               | Implements    | Description                            |
| :--------- | :------------------- | :------------ | :------------------------------------- |
| `buildkit` | _(via docker CLI)_   | `ImageBuilder`| `docker buildx`-based multi-arch builds|

**Type alias when enabled:** `builder::SelectedBuilder = builder::buildkit::BuildKitBuilder`

At least one builder feature must be enabled.

BuildKit creates per-platform builder instances named `dockermint-amd64` and
`dockermint-arm64`.

---

### Metrics backends

| Feature      | Crates                          | Implements         | Description                           |
| :----------- | :------------------------------ | :----------------- | :------------------------------------ |
| `prometheus` | `axum`, `tower`, `tower-http`   | `MetricsCollector` | Prometheus-format metrics over HTTP   |

**Type alias when enabled:** `metrics::SelectedMetrics = metrics::prometheus::PrometheusCollector`

At least one metrics feature must be enabled.

The metrics server binds to `127.0.0.1:9200` by default. Configure via
`[metrics]` in `config.toml`.

When `prometheus` is disabled, the `axum`, `tower`, and `tower-http`
optional dependencies are also not compiled in, shrinking the binary.

---

## Build Examples

### Standard production build (all defaults)

```bash
cargo build --release
```

### musl static build (no OpenSSL)

```bash
cargo build --release \
  --no-default-features \
  --features redb,telegram,github,rustls-tls,oci,buildkit,prometheus \
  --target x86_64-unknown-linux-musl
```

### Minimal build (no metrics server)

```bash
cargo build --release \
  --no-default-features \
  --features redb,telegram,github,native-tls,oci,buildkit
```

Note: because `prometheus` is the only metrics feature, omitting it triggers
a `compile_error!`. A stub "no-op" metrics feature is expected in a future
release.

---

## Compile-Time Enforcement

Each module with a replaceable backend has a guard:

```rust
#[cfg(not(any(feature = "redb")))]
compile_error!("At least one database backend must be enabled (e.g. 'redb').");
```

If you disable all features in a category, the build fails with this message
rather than producing a silently broken binary.

---

## Implementing a Custom Backend

To add a new database backend as an example:

1. Add a feature flag in `Cargo.toml`:
   ```toml
   mydb = ["dep:mydb-crate"]
   mydb-crate = { version = "1", optional = true }
   ```
2. Create `src/saver/mydb.rs` implementing `saver::Database`.
3. Add the conditional alias:
   ```rust
   #[cfg(feature = "mydb")]
   pub type SelectedDatabase = mydb::MyDatabase;
   ```
4. Update the guard to include `feature = "mydb"`:
   ```rust
   #[cfg(not(any(feature = "redb", feature = "mydb")))]
   compile_error!("At least one database backend must be enabled.");
   ```

The same pattern applies to all other module categories.
