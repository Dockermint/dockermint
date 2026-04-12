# Module Reference

This document covers each `src/` module: its purpose, public trait or API,
configuration keys, and available implementations.

---

## `checker`

**Purpose:** Verify system requirements and enforce single-instance execution.

**Source:** `src/checker/mod.rs`

### Public API

```rust
pub struct SystemRequirements { pub docker: bool, pub buildx: bool, pub git: bool }
pub struct LockGuard { /* private */ }

pub async fn verify_requirements(docker_socket_uri: &str) -> Result<SystemRequirements, CheckerError>
pub fn ensure_singleton() -> Result<LockGuard, CheckerError>
pub fn ensure_singleton_at(path: &Path) -> Result<LockGuard, CheckerError>
```

`verify_requirements` accepts the Docker daemon socket URI (from
`config.docker.socket_uri`) and probes three tools:

- `docker version` — routed through `DOCKER_HOST=<docker_socket_uri>`
- `docker buildx version` — routed through `DOCKER_HOST=<docker_socket_uri>`
- `git --version` — run without any Docker environment variable

Routing Docker probes through the configured socket URI ensures that remote or
non-standard daemons are checked correctly, not just the local default socket.
Git is checked without the Docker environment because it does not communicate
with the Docker daemon.

When a Docker tool is unreachable, the error message includes the socket URI to
aid debugging:

```
docker (via tcp://192.168.1.10:2376)
docker buildx (via tcp://192.168.1.10:2376)
```

`verify_requirements` returns `CheckerError::MissingTool` for the first absent
or unreachable tool.

`ensure_singleton` writes a PID file to `/tmp/dockermint.lock`. If the file
already exists and the stored PID maps to a live process, it returns
`CheckerError::AlreadyRunning`. Stale locks (dead PID) are silently removed.
The returned `LockGuard` removes the lock file on drop.

### Errors

| Error                          | When                                                              |
| :----------------------------- | :---------------------------------------------------------------- |
| `CheckerError::MissingTool`    | A required tool (`docker`, `docker buildx`, `git`) is absent or unreachable |
| `CheckerError::AlreadyRunning` | Another live Dockermint process holds the lock                    |
| `CheckerError::CheckFailed`    | Lock file cannot be written                                       |
| `CheckerError::Command`        | Tool probe command could not be spawned                           |

---

## `commands`

**Purpose:** Async shell command execution with structured output capture.

**Source:** `src/commands/mod.rs`

### Public API

```rust
pub struct CommandOutput { pub stdout: String, pub stderr: String, pub status: ExitStatus }

pub async fn execute(cmd: &str, args: &[&str]) -> Result<CommandOutput, CommandError>
pub async fn execute_with_env(cmd: &str, args: &[&str], env: &HashMap<String, String>) -> Result<CommandOutput, CommandError>
pub async fn execute_unchecked(cmd: &str, args: &[&str]) -> Result<CommandOutput, CommandError>
pub async fn execute_unchecked_with_env(cmd: &str, args: &[&str], env: &HashMap<String, String>) -> Result<CommandOutput, CommandError>
```

`execute` treats non-zero exit as `CommandError::NonZeroExit`.
`execute_unchecked` returns `CommandOutput` regardless of exit code.
`execute_unchecked_with_env` combines both behaviors: it injects extra
environment variables and returns `CommandOutput` regardless of exit code. Used
internally by the checker to probe Docker tool availability through a configured
`DOCKER_HOST` without treating a non-zero probe result as an error.

`execute` is implemented as `execute_with_env(cmd, args, &HashMap::new())`.
Similarly, `execute_unchecked` delegates to `execute_unchecked_with_env` with
an empty env map.

### Errors

| Error                       | When                                       |
| :-------------------------- | :----------------------------------------- |
| `CommandError::Spawn`       | Process cannot be started                  |
| `CommandError::NonZeroExit` | Process exits with status != 0             |
| `CommandError::OutputCapture` | Reading stdout/stderr fails              |

---

## `config`

**Purpose:** Load `config.toml`, validate settings, apply CLI overrides, and
read `.env` secrets.

**Source:** `src/config/mod.rs`, `src/config/types.rs`

### Public API

```rust
pub fn load(path: &Path) -> Result<Config, ConfigError>
pub fn load_default() -> Result<Config, ConfigError>
pub fn validate(config: &Config) -> Result<(), ConfigError>
pub fn apply_daemon_overrides(
    config: &mut Config,
    poll_interval: Option<u64>,
    max_builds: Option<u32>,
    rpc: bool,
    rpc_bind: std::net::SocketAddr,
)
pub fn load_secrets() -> Secrets
```

`load` reads the file, deserializes it with `toml::from_str`, then calls
`validate` automatically. `load_default` deserializes an empty string,
producing a `Config` with all built-in defaults (no file required).

`validate` enforces these rules:

| Rule                                         | Error                                              |
| :------------------------------------------- | :------------------------------------------------- |
| `version` != 1                               | `ConfigError::Invalid` — unsupported version       |
| `recipes_dir` is absolute and does not exist | `ConfigError::Invalid` — directory not found       |
| `daemon.poll_interval_secs` == 0             | `ConfigError::Invalid` — must be > 0               |
| `daemon.max_builds_per_cycle` == 0           | `ConfigError::Invalid` — must be > 0               |

`apply_daemon_overrides` is called after `load` in daemon mode. It creates the
`[daemon]` section with defaults (`poll_interval_secs = 60`,
`max_builds_per_cycle = 1`) if none was present, then applies any non-`None`
CLI arguments on top. When `rpc = true`, it creates or updates the `[rpc]`
section with the given bind address.

`load_secrets` calls `dotenvy::dotenv()` (ignores missing `.env`) then reads
known environment variables into a `Secrets` struct using `SecretString` for
all sensitive values.

### Key Types

- `Config` — top-level struct; see [Configuration Reference](./configuration.md).
- `DockerConfig` — `socket_uri` (`String`, default `unix:///var/run/docker.sock`)
  and `builder_prefix` (`String`, default `dockermint`). Implements `Default`.
- `Secrets` — holds `GH_USER`, `GH_PAT`, `TELEGRAM_TOKEN`, `TELEGRAM_CHAT_ID`,
  `REGISTRY_USER`, `REGISTRY_PASSWORD` as `Option<SecretString>`.
- `Mode` — enum: `Cli`, `Daemon`.

### Errors

| Error                       | When                                       |
| :-------------------------- | :----------------------------------------- |
| `ConfigError::ReadFile`     | `config.toml` cannot be read from disk     |
| `ConfigError::Parse`        | TOML deserialization fails                 |
| `ConfigError::Invalid`      | Semantic validation rule violated          |
| `ConfigError::MissingField` | Required field absent                      |
| `ConfigError::MissingEnvVar`| Required environment variable not set      |

---

## `logger`

**Purpose:** Initialize the global `tracing` subscriber.

**Source:** `src/logger/mod.rs`

### Public API

```rust
pub fn init(config: &LogConfig) -> Result<(), Error>
```

Must be called exactly once at startup. Supports four output modes:

| `directory` | `json` | Output                          |
| :---------- | :----- | :------------------------------ |
| absent      | false  | Plain text to stdout            |
| absent      | true   | JSON to stdout                  |
| set         | false  | Plain text, daily rotating file |
| set         | true   | JSON, daily rotating file       |

When file rotation is enabled, log files are named
`<file_prefix>.YYYY-MM-DD` inside `directory`.

---

## `recipe`

**Purpose:** Parse recipe TOML files, resolve flavors, validate selections.

**Source:** `src/recipe/mod.rs`, `src/recipe/types.rs`, `src/recipe/validation.rs`

### Public API

```rust
pub fn load(path: &Path) -> Result<Recipe, RecipeError>
pub fn load_all(dir: &Path) -> Result<HashMap<String, Recipe>, RecipeError>
pub fn resolve_flavours(recipe: &Recipe, config_overrides: Option<&RecipeFlavourOverride>, cli_overrides: Option<&HashMap<String, FlavorValue>>) -> Result<SelectedFlavours, RecipeError>
pub fn resolve(recipe: Recipe, config_overrides: Option<&RecipeFlavourOverride>, cli_overrides: Option<&HashMap<String, FlavorValue>>, host_variables: &HashMap<String, String>) -> Result<ResolvedRecipe, RecipeError>
```

`resolve` combines flavor resolution with:
- Injection of all selected flavor values as template variables, with any
  `{{UPPERCASE}}` host variable references inside flavor defaults expanded
  inline (e.g. `architecture = "{{HOST_ARCH}}"` becomes `"x86_64"`).
- Injection of `binary_name` from `header`.
- Injection of profile variables for any `FlavorValue::Single` dimension that
  has a matching `[profiles]` entry.

### Key Types

- `Recipe` — the deserialized TOML file.
- `SelectedFlavours` — resolved dimension → value map with `get_single`,
  `get_multiple`, and `has_value` accessors.
- `ResolvedRecipe` — `Recipe` + `SelectedFlavours` + `resolved_variables`.
- `FlavorValue` — `Single(String)` | `Multiple(Vec<String>)`.

`SelectedFlavours::has_value(value)` returns `true` if any active flavor
selection (single or multi-value) equals `value`. This is how `[[pre_build]]`
conditions are evaluated — a step fires when `has_value(step.condition)` is
`true`.

### `recipe::host_vars`

**Source:** `src/recipe/host_vars.rs`

Collects host-level variables resolved at Dockermint startup. All variables
produced here follow the `{{UPPERCASE}}` convention (except `repository_path`,
which is Dockermint-provided).

```rust
pub fn collect(tag: &str, selected_flavours: &SelectedFlavours) -> HashMap<String, String>
pub fn extend_from_env(vars: &mut HashMap<String, String>, keys: &[&str])
```

`collect` always produces:

| Variable              | Source                                                   |
| :-------------------- | :------------------------------------------------------- |
| `HOST_ARCH`           | `std::env::consts::ARCH` mapped to `x86_64` / `aarch64` |
| `CREATION_TIMESTAMP`  | UTC timestamp in ISO 8601 format (`YYYY-MM-DDTHH:MM:SSZ`), computed without external crates using the Howard Hinnant *civil_from_days* algorithm |
| `SEMVER_TAG`          | The Git tag passed as `tag` argument                     |
| `BUILD_TAGS_COMMA_SEP`| Comma-joined `build_tags` flavor values (omitted if the `build_tags` dimension has no multi-value selection) |
| `repository_path`     | Fixed as `/workspace` — the default clone destination    |

`extend_from_env` forwards additional process environment variables (e.g.
`GH_USER`, `GH_PAT`) into the variable map. Missing variables are silently
skipped.

### Errors

| Error                             | When                                            |
| :-------------------------------- | :---------------------------------------------- |
| `RecipeError::ReadFile`           | TOML file cannot be read                        |
| `RecipeError::Parse`              | TOML deserialization fails                      |
| `RecipeError::IncompatibleFlavour`| Selected value not in `flavours.available`      |
| `RecipeError::UnknownFlavour`     | Override references a dimension not in recipe   |
| `RecipeError::UnsupportedSchema`  | Schema version exceeds current maximum (1)      |
| `RecipeError::VersionMismatch`    | Recipe requires a newer Dockermint release      |

---

## `scrapper`

**Purpose:** Fetch releases/tags from a VCS provider with glob filtering.

**Source:** `src/scrapper/mod.rs`, `src/scrapper/github.rs`

### Trait

```rust
pub trait VersionControlSystem: Send + Sync {
    async fn fetch_releases(&self, repo_url: &str, filter: &TagFilter) -> Result<Vec<Release>, VcsError>;
}
```

### Key Types

```rust
pub struct Release { pub tag: String, pub prerelease: bool, pub published_at: Option<String> }
pub struct TagFilter { pub include_patterns: String, pub exclude_patterns: String }
```

`TagFilter` uses comma-separated glob patterns (via `globset`). If
`include_patterns` is empty, all tags pass. `exclude_patterns` always wins
over `include_patterns`.

### Default Implementation: `GithubClient` (feature: `github`)

Source: `src/scrapper/github.rs`.

```rust
pub struct GithubClient { /* client: reqwest::Client, auth: Option<(String, SecretString)> */ }

impl GithubClient {
    pub fn new(user: Option<&str>, pat: Option<&str>) -> Result<Self, VcsError>
}
```

**Authentication**

`GithubClient::new` accepts optional `GH_USER` and `GH_PAT` credentials.
Both must be set together or both absent — providing only one returns
`VcsError::Auth`. When credentials are present they are stored as
`(String, SecretString)` and sent via HTTP Basic Auth on every request.
Unauthenticated requests are subject to GitHub's lower public rate limit.

**Pagination**

`fetch_releases` pages through the GitHub Releases API at 100 releases per
page, accumulating results until the API returns a page with fewer than 100
entries (indicating the last page). All pages are fetched before glob
filtering is applied.

**URL parsing**

`parse_owner_repo` extracts `owner/repo` from a full GitHub URL. Trailing
slashes and `.git` suffixes are stripped before parsing.

```
https://github.com/cosmos/gaia        -> "cosmos/gaia"
https://github.com/cosmos/gaia.git    -> "cosmos/gaia"
https://github.com/KYVENetwork/chain/ -> "KYVENetwork/chain"
```

**Glob filtering**

Tag strings are matched with `globset`. Patterns in `TagFilter` are
comma-separated. If `include_patterns` is empty, all tags pass the include
step. `exclude_patterns` is evaluated after include: a tag matching any
exclude pattern is dropped even if it matched an include pattern.

```toml
# recipe header — tags processed by GithubClient
include_patterns = "v*"
exclude_patterns = "*-rc*, *-alpha*"
```

**Rate limiting**

HTTP `403` and `429` responses are detected as rate-limit errors. The
`retry-after` response header is read; when absent, 60 seconds is used as
the default back-off hint. The error is returned as `VcsError::RateLimit {
retry_after_secs }` so the caller can decide when to retry.

### Errors

| Error                  | When                                    |
| :--------------------- | :-------------------------------------- |
| `VcsError::Request`    | HTTP request fails or non-2xx status    |
| `VcsError::Parse`      | Response body cannot be deserialized    |
| `VcsError::Auth`       | Credentials rejected or inconsistent    |
| `VcsError::RateLimit`  | GitHub rate limit exceeded (`retry_after_secs` carries hint) |

---

## `builder`

**Purpose:** Generate Dockerfiles, expand template variables, and run
multi-arch builds via `docker buildx`.

**Source:** `src/builder/mod.rs`, `src/builder/template.rs`,
`src/builder/buildkit.rs`, `src/builder/dockerfile.rs`,
`src/builder/go/mod.rs`

### Trait

```rust
pub trait ImageBuilder: Send + Sync {
    async fn setup_builders(&self) -> Result<(), BuilderError>;
    async fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError>;
    async fn cleanup(&self) -> Result<(), BuilderError>;
}
```

### Key Types

```rust
pub struct BuildContext {
    pub recipe: ResolvedRecipe,
    pub tag: String,
    pub variables: HashMap<String, String>,
    pub platforms: Vec<String>,
}

pub struct BuildOutput {
    pub image_id: String,
    pub image_tag: String,
    pub duration: Duration,
    pub platforms: Vec<String>,
}
```

`BuildContext::resolve_image_tag` expands the recipe's `image.tag` template
using the context's resolved variables.

### TemplateEngine

`builder::template::TemplateEngine` is a stateless struct. All methods are
associated functions:

```rust
pub fn render(template: &str, vars: &HashMap<String, String>) -> String
pub fn unresolved_vars(s: &str) -> Vec<String>
```

Unknown `{{placeholders}}` are preserved in the output.

### Dockerfile Generator (`builder::dockerfile`)

**Source:** `src/builder/dockerfile.rs`

```rust
pub fn generate(recipe: &ResolvedRecipe) -> Result<String, BuilderError>
```

Produces a complete multi-stage Dockerfile from a `ResolvedRecipe`. The
generator is fully data-driven — every instruction originates from recipe
fields. No chain-specific Rust code is required.

The generated Dockerfile has two stages:

**Stage 1 — `builder`**

1. `FROM <scrapper.image> AS builder`
2. `RUN` to install scrapper deps and builder deps (merged with `&&`). The
   correct builder install command is selected via `detect_install_command`,
   which matches the longest `[builder.install]` key that appears as a
   substring of the scrapper image name.
3. `ARG` declarations for environment variables from `scrapper.env`, plus
   `GIT_TAG`.
4. `RUN git clone ...` using the method from `scrapper.method`. The
   `try-authenticated-clone` method attempts an authenticated clone first,
   falling back to a public clone.
5. `WORKDIR` and `RUN git checkout ${GIT_TAG}`.
6. `ENV` for each entry in `[build.env]`.
7. Conditional `[[pre_build]]` steps — each fires when
   `selected_flavours.has_value(step.condition)` is `true`.
8. `RUN <build_script>` — dispatched by `header.type` to the appropriate
   sub-module (currently `"golang"`).

**Stage 2 — `runner`**

The `running_env` flavor value is converted to a Docker image reference by
`running_env_to_image`: values containing a digit boundary are split at that
boundary (`alpine3.23` → `alpine:3.23`, `ubuntu24.04` → `ubuntu:24.04`);
`bookworm` maps to `debian:bookworm-slim`; `distroless` maps to
`gcr.io/distroless/static-debian12`; full references (containing `/` or `:`)
pass through unchanged.

1. `FROM <runner_image> AS runner`
2. Optional `RUN` to create a non-root user (Alpine uses `adduser/addgroup`;
   other distros use `useradd/groupadd`; distroless skips user creation
   entirely).
3. `COPY --from=builder` for always-copied entries, then for conditional
   entries matching the active `binary_type` flavor.
4. `EXPOSE` for all ports in `[expose]`.
5. `LABEL` for all entries in `[labels]`.
6. `USER` if non-root.
7. `ENTRYPOINT` for the entry marked `type = "entrypoint"` in `[copy]`.

Adding support for a new build system (not a new chain) requires one match arm
in `generate_build_command` and one new submodule.

### Go Build Helpers (`builder::go`)

**Source:** `src/builder/go/mod.rs`

```rust
pub fn generate_build_script(recipe: &ResolvedRecipe) -> String
pub fn build_ldflags(recipe: &ResolvedRecipe, variables: &HashMap<String, String>) -> String
pub fn build_tags(recipe: &ResolvedRecipe) -> String
pub fn build_args(recipe: &ResolvedRecipe, variables: &HashMap<String, String>) -> Vec<String>
pub fn template_to_shell(s: &str) -> String
```

`generate_build_script` produces a shell script body for the Dockerfile `RUN`
instruction. It implements a two-phase variable strategy:

- **Host-time variables** (already present in `resolved_variables`) are
  expanded inline by `TemplateEngine::render` before the script is written.
- **Build-time variables** (`{{lowercase}}` from `[variables]`) remain as
  `{{name}}` after the first pass. `template_to_shell` then converts each
  remaining `{{name}}` to `$name` for shell interpolation.

The resulting script has this structure:

```bash
set -e; \
    repo_commit=$(git log -1 --format='%H'); \
    repo_version=$(git describe ...); \
    go build -mod=readonly \
      -tags=netgo,muslc \
      -ldflags="-linkmode=external ... -X 'path.Version=$repo_version'" \
      -o /go/bin/gaiad \
      /workspace/cmd/gaiad
```

`build_tags` collects the `build_tags` multi-value flavor and automatically
appends a non-`goleveldb` `db_backend` value as an additional tag (e.g.
`pebbledb`).

`build_ldflags` combines the `binary_type`-keyed flag string from
`[build.linker.flags]` with `-X` flags from `[build.linker.variables]`, with
template variables fully expanded (no shell interpolation — used outside of
Dockerfile generation).

`build_args` assembles the full `go build` argument vector:
`-mod=readonly`, `-tags=...`, `-ldflags=...`, `-o /go/bin/<binary>`, build
path.

### Default Implementation: `BuildKitBuilder`

Feature flag: `buildkit`. Source: `src/builder/buildkit.rs`.

```rust
pub struct BuildKitBuilder { /* docker_host, prefix, persist */ }

impl BuildKitBuilder {
    pub fn new(docker_host: String, prefix: String, persist: bool) -> Self
}
```

**Builder lifecycle**

`setup_builders()` iterates over `[("linux/amd64", "amd64"), ("linux/arm64",
"arm64")]` and calls `ensure_builder` for each. `ensure_builder` first runs
`docker buildx inspect <name>` to detect an existing builder; if absent it
runs:

```bash
docker buildx create --name <name> --platform <platform> --driver docker-container
docker buildx inspect <name> --bootstrap
```

Bootstrap runs before the first build so the builder is ready immediately.

**`build()`**

1. Calls `dockerfile::generate(&context.recipe)` to produce the Dockerfile
   text.
2. Writes the Dockerfile to `$TMPDIR/dockermint-build/Dockerfile`.
3. Selects the builder instance — for multi-platform builds uses the `amd64`
   builder (QEMU handles cross-compilation inside the container driver); for
   single-platform builds uses the matching suffix.
4. Assembles `docker buildx build` arguments:
   - `--builder <name>`
   - `--platform <comma-separated>`
   - `-f <dockerfile path>`
   - `--build-arg GIT_TAG=<tag>`
   - `--build-arg GH_USER=<value>` and `--build-arg GH_PAT=<value>` when
     those environment variables are present
   - `--load` (loads the image into the local Docker daemon)
   - Build context directory (same directory as the Dockerfile)
5. Runs the command via `commands::execute_with_env` with `DOCKER_HOST` set.
6. Returns `BuildOutput` with `image_id`, `image_tag`, `duration`, and
   `platforms`.

**`cleanup()`**

When `persist = false` (CLI mode): runs `docker buildx stop <name>` then
`docker buildx rm <name>` for each platform builder, and removes the temp
build directory. When `persist = true` (daemon mode): returns immediately,
leaving builders alive for the next polling cycle.

**`persist` flag**

| `persist` | Mode   | Behavior                                                  |
| :-------- | :----- | :-------------------------------------------------------- |
| `false`   | CLI    | Builders created before build, removed after             |
| `true`    | Daemon | Builders created once, survive across polling cycles     |

### Errors

| Error                          | When                                       |
| :----------------------------- | :----------------------------------------- |
| `BuilderError::DockerfileGeneration` | Dockerfile template expansion fails  |
| `BuilderError::BuildFailed`    | `docker buildx build` exits non-zero       |
| `BuilderError::BuildxSetup`    | Builder instance creation fails            |
| `BuilderError::UnresolvedVariable` | Template variable left unresolved      |
| `BuilderError::Command`        | Subprocess cannot be spawned               |

---

## `push`

**Purpose:** Authenticate with a container registry and push images.

**Source:** `src/push/mod.rs`, `src/push/oci.rs`

### Trait

```rust
pub trait RegistryClient: Send + Sync {
    async fn authenticate(&self) -> Result<(), RegistryError>;
    async fn push_image(&self, image: &str, tag: &str) -> Result<(), RegistryError>;
    async fn tag_exists(&self, image: &str, tag: &str) -> Result<bool, RegistryError>;
}
```

### Default Implementation: `OciRegistry`

Feature flag: `oci`. Source: `src/push/oci.rs`.

```rust
pub struct OciRegistry { /* docker_host, registry_url */ }

impl OciRegistry {
    pub fn new(docker_host: String, registry_url: Option<String>) -> Self
}
```

All operations pass `DOCKER_HOST` to Docker CLI commands.

**`authenticate()`**

Reads `REGISTRY_USER` and `REGISTRY_PASSWORD` from the process environment.
If either is absent, authentication is skipped (useful for public registries or
pre-authenticated daemons). When both are present:

```bash
# password is piped via stdin — never passed on the command line
echo "$REGISTRY_PASSWORD" | docker login --username <user> --password-stdin [<registry>]
```

The registry argument is omitted when `registry_url` is `None` (Docker Hub).

**`push_image(image, tag)`**

Constructs `<image>:<tag>` and runs:

```bash
docker push <image>:<tag>
```

**`tag_exists(image, tag)`**

Runs `docker manifest inspect <image>:<tag>` without pulling the image.
Returns `true` on exit code 0 (tag exists), `false` on non-zero (not found).
Network errors (spawn failure) are returned as `RegistryError::Query`.

### Configuration

```toml
[registry]
url = "ghcr.io"   # optional; absent = Docker Hub
```

### Errors

| Error                  | When                                     |
| :--------------------- | :--------------------------------------- |
| `RegistryError::Auth`  | Credentials rejected by registry         |
| `RegistryError::Push`  | `docker push` fails                      |
| `RegistryError::Query` | Tag existence check fails                |
| `RegistryError::Command` | Subprocess cannot be spawned           |

---

## `saver`

**Purpose:** Persist and query build records.

**Source:** `src/saver/mod.rs`, `src/saver/redb.rs`

### Trait

```rust
pub trait Database: Send + Sync {
    async fn save_build(&self, record: &BuildRecord) -> Result<(), DatabaseError>;
    async fn get_build(&self, recipe: &str, tag: &str) -> Result<Option<BuildRecord>, DatabaseError>;
    async fn list_builds(&self, recipe: &str) -> Result<Vec<BuildRecord>, DatabaseError>;
    async fn is_built(&self, recipe: &str, tag: &str) -> Result<bool, DatabaseError>;
}
```

### Key Types

```rust
pub struct BuildRecord {
    pub recipe_name: String,
    pub tag: String,
    pub status: BuildStatus,
    pub image_tag: Option<String>,
    pub started_at: String,       // ISO-8601
    pub completed_at: Option<String>,
    pub duration_secs: Option<u64>,
    pub error: Option<String>,
    pub flavours: HashMap<String, String>,
}

pub enum BuildStatus { InProgress, Success, Failed }
```

### Default Implementation: `RedbDatabase` (feature: `redb`)

Source: `src/saver/redb.rs`.

```rust
pub struct RedbDatabase { /* Arc<redb::Database> */ }

impl RedbDatabase {
    pub fn open(path: &Path) -> Result<Self, DatabaseError>
}
```

**Storage layout**

Records are stored in a single redb table named `"builds"`. The key is the
string `"{recipe_name}:{tag}"` (e.g. `"cosmos-gaiad:v21.0.1"`). The value is
JSON-serialized bytes of a `StoredRecord` (an internal flat struct).

**Open / create**

`open` creates parent directories automatically before opening the database
file. If the file does not exist, redb creates it. The `"builds"` table is
initialized in a write transaction on first open so subsequent reads never
encounter a missing table.

**Thread safety**

`RedbDatabase` wraps the inner `redb::Database` in an `Arc`, making it
`Clone + Send + Sync`. Multiple tasks may hold a clone and call methods
concurrently; redb serializes transactions internally.

**CRUD operations**

| Method | Key lookup | Transaction |
| :----- | :--------- | :---------- |
| `save_build` | `recipe_name:tag` | write — inserts or overwrites |
| `get_build` | `recipe_name:tag` | read — returns `None` when absent |
| `list_builds` | `recipe_name:` prefix scan | read — iterates full table |
| `is_built` | `recipe_name:tag` | read — existence check only |

`is_built` checks for a key regardless of `BuildStatus`. A tag with
`BuildStatus::Failed` is considered built and will be skipped in the daemon
polling loop.

**JSON serialization**

`BuildRecord` is not directly serialized. An internal `StoredRecord` struct
is used instead, converting `BuildStatus` to/from string literals
(`"in_progress"`, `"success"`, `"failed"`). Unknown status strings deserialize
as `InProgress`.

### Configuration

```toml
[database]
path = "data/dockermint.redb"
```

### Errors

| Error                        | When                                     |
| :--------------------------- | :--------------------------------------- |
| `DatabaseError::Open`        | Database file cannot be created/opened, or parent dir creation fails |
| `DatabaseError::Read`        | Read transaction fails                   |
| `DatabaseError::Write`       | Write transaction fails                  |
| `DatabaseError::Serialization` | Record serialization/deserialization fails |

---

## `notifier`

**Purpose:** Send build lifecycle notifications to an external channel.

**Source:** `src/notifier/mod.rs`, `src/notifier/telegram.rs`

### Trait

```rust
pub trait Notifier: Send + Sync {
    async fn notify_build_start(&self, recipe: &str, tag: &str) -> Result<(), NotifierError>;
    async fn notify_build_success(&self, recipe: &str, tag: &str, duration: Duration) -> Result<(), NotifierError>;
    async fn notify_build_failure(&self, recipe: &str, tag: &str, error: &str) -> Result<(), NotifierError>;
}
```

Delivery is best-effort: a `NotifierError` should be logged but must not abort
a build.

### Default Implementation: `TelegramNotifier` (feature: `telegram`)

Source: `src/notifier/telegram.rs`.

```rust
pub struct TelegramNotifier { /* client: reqwest::Client, api_url: String, chat_id: String */ }

impl TelegramNotifier {
    pub fn new(token: &str, chat_id: &str) -> Result<Self, NotifierError>
}
```

`new` validates that neither `token` nor `chat_id` is empty. The Bot API URL
is constructed as `https://api.telegram.org/bot{token}/sendMessage` and stored
at construction time. Required secrets:

| Variable           | Description                        |
| :----------------- | :--------------------------------- |
| `TELEGRAM_TOKEN`   | Bot API token from BotFather       |
| `TELEGRAM_CHAT_ID` | Chat ID where messages are sent    |

Enable/disable via `config.toml`:

```toml
[notifier]
enabled = true
```

**Message types**

All messages use `parse_mode: "Markdown"`. Messages longer than 4096
characters (Telegram's limit) are truncated at the byte boundary before
sending.

| Method | Format |
| :----- | :----- |
| `notify_build_start` | `*Build started*\nRecipe: \`{recipe}\`\nTag: \`{tag}\`` |
| `notify_build_success` | `*Build succeeded*\nRecipe: \`{recipe}\`\nTag: \`{tag}\`\nDuration: {secs}s` |
| `notify_build_failure` | `*Build failed*\nRecipe: \`{recipe}\`\nTag: \`{tag}\`\nError: \`\`\`\n{error}\n\`\`\`` |

**Best-effort delivery**

The daemon calls each `notify_*` method and logs a warning on error, but
never propagates `NotifierError` to the caller. A Telegram outage does not
stop builds.

### Errors

| Error                  | When                                        |
| :--------------------- | :------------------------------------------ |
| `NotifierError::Send`  | HTTP delivery fails or Telegram API returns non-2xx |
| `NotifierError::Config`| Token or chat ID is empty                   |

---

## `metrics`

**Purpose:** Collect build metrics and expose them over HTTP.

**Source:** `src/metrics/mod.rs`, `src/metrics/prometheus.rs`

### Trait

```rust
pub trait MetricsCollector: Send + Sync {
    fn record_build_start(&self, recipe: &str, tag: &str);
    fn record_build_success(&self, recipe: &str, tag: &str, duration: Duration);
    fn record_build_failure(&self, recipe: &str, tag: &str);
    async fn serve(&self, addr: SocketAddr) -> Result<(), MetricsError>;
}
```

The three `record_*` methods are synchronous (non-blocking counter
increments). `serve` starts an async HTTP server.

### Default Implementation: `PrometheusCollector`

Feature flag: `prometheus`. Exposes metrics in Prometheus text format over
HTTP (built with `axum` + `tower`). Bind address is configured via:

```toml
[metrics]
enabled = true
bind    = "127.0.0.1:9200"
```

Scrape endpoint: `http://<bind>/metrics`

### Errors

| Error                       | When                                    |
| :-------------------------- | :-------------------------------------- |
| `MetricsError::Server`      | HTTP server fails to bind or serve      |
| `MetricsError::Registration`| Metric registration fails at startup    |

---

## `cli`

**Purpose:** Clap-based CLI definition with two subcommands.

**Source:** `src/cli/mod.rs`, `src/cli/commands/mod.rs`,
`src/cli/commands/build.rs`, `src/cli/commands/daemon.rs`

### Public API

```rust
pub struct Cli {
    pub config: Option<PathBuf>,   // --config / DOCKERMINT_CONFIG
    pub log_level: Option<String>, // --log-level
    pub command: Commands,
}

pub enum Commands {
    Build(BuildArgs),
    Daemon(DaemonArgs),
}
```

### `BuildArgs`

```rust
pub struct BuildArgs {
    pub recipe: PathBuf,                    // -r / --recipe
    pub tag: String,                        // -t / --tag
    pub platform: String,                   // -p / --platform [default: "linux/amd64"]
    pub flavors: Vec<(String, FlavorValue)>,// -f / --flavor (repeatable)
    pub push: bool,                         // --push
}
```

Helper methods:
- `flavor_overrides() -> HashMap<String, FlavorValue>` — converts the `flavors`
  vec into a map for `recipe::resolve`.
- `platforms() -> Vec<String>` — splits the `platform` string on commas.

### `DaemonArgs`

```rust
pub struct DaemonArgs {
    pub poll_interval: Option<u64>,  // -i / --poll-interval
    pub max_builds: Option<u32>,     // -m / --max-builds
    pub recipes: Vec<String>,        // -r / --recipes (file stems)
    pub rpc: bool,                   // --rpc
    pub rpc_bind: SocketAddr,        // --rpc-bind [default: 127.0.0.1:9100]
}
```

`--rpc` enables the embedded HTTP server. `--rpc-bind` sets the bind address
and requires `--rpc` to have any effect. Values in `DaemonArgs` are applied
via `config::apply_daemon_overrides` before the daemon loop starts.

---

## `lib` — Mode Entry Points

**Purpose:** Provide the top-level async functions that wire all modules
together for each operating mode.

**Source:** `src/lib.rs`

### `run_build()`

```rust
pub async fn run_build(config: Config, args: BuildArgs) -> Result<(), Error>
```

Executes a complete one-shot CLI build. Steps in order:

1. **System check** — calls `checker::verify_requirements(&config.docker.socket_uri)`.
   Exits immediately if `docker`, `docker buildx`, or `git` is missing or
   unreachable through the configured socket URI.
2. **Recipe load** — calls `recipe::load(&args.recipe)`.
3. **Flavor resolution** — pre-resolves flavors via `recipe::resolve_flavours`
   to obtain `build_tags` (needed for `BUILD_TAGS_COMMA_SEP`), then calls
   `host_vars::collect` and `host_vars::extend_from_env` to build the host
   variable map, then calls the full `recipe::resolve` with host vars.
4. **Builder setup** — constructs `BuildKitBuilder::new(socket_uri, prefix,
   persist=false)` and calls `setup_builders()` to create `{prefix}-amd64`
   and `{prefix}-arm64` buildx instances.
5. **Build** — constructs `BuildContext` and calls `builder.build(&context)`.
6. **Cleanup** — calls `builder.cleanup()` unconditionally (even when the
   build failed). A cleanup error is logged but does not mask the build error.
7. **Push** (when `--push` is set) — constructs `OciRegistry`, calls
   `authenticate()`, then `push_image(name, tag)`. The `image:tag` string from
   `BuildOutput` is split at the last `:` to obtain separate image name and
   tag arguments.

### `run_daemon()`

```rust
pub async fn run_daemon(config: Config, args: DaemonArgs) -> Result<(), Error>
```

Executes the continuous daemon polling loop. Steps in order:

1. **System check** — calls `checker::verify_requirements` with the configured
   Docker socket URI. Fatal on failure.
2. **Database open** — calls `RedbDatabase::open(&config.database.path)`. Parent
   directories are created automatically. Fatal on failure.
3. **Builder setup** — constructs `BuildKitBuilder::new(socket_uri, prefix,
   persist=true)` and calls `setup_builders()`. Builders are created once and
   survive across polling cycles.
4. **Notifier init** — calls `init_notifier(&config)`. Returns `None` when
   `config.notifier.enabled` is `false` or when `TELEGRAM_TOKEN` /
   `TELEGRAM_CHAT_ID` are absent. Notifier errors during init are logged and
   silently skipped; the daemon continues without notifications.
5. **VCS client init** — constructs `GithubClient::new` with `GH_USER` and
   `GH_PAT` read from the process environment via `config::load_secrets`. Fatal
   if only one credential is set.
6. **Registry client init** — constructs `OciRegistry` from `config.docker` and
   `config.registry`.
7. **Poll loop** — `tokio::select!` between a graceful shutdown signal and
   `daemon_cycle(...)`. After each cycle, another `select!` sleeps for
   `poll_interval_secs` (default 60) or exits on signal.
8. **Shutdown** — on SIGINT or SIGTERM the loop exits cleanly. Because
   `persist=true`, buildx builders are left alive (they were not created by the
   daemon exclusively).

**`daemon_cycle`**

Loads all recipes from `config.recipes_dir` with `recipe::load_all`. Iterates
each recipe stem; skips any not in `--recipes` filter when that filter is
non-empty. Calls `process_recipe` for each; errors from `process_recipe` are
logged and the daemon continues to the next recipe.

**`process_recipe`**

1. Fetches releases from GitHub using the recipe's `header.repo`, `include_patterns`,
   and `exclude_patterns`.
2. For each release (newest-first), calls `db.is_built(recipe_stem, tag)`. Tags
   that are already recorded in the database (any status, including
   `BuildStatus::Failed`) are skipped. Tags where the DB check itself fails are
   also skipped with a warning.
3. Collects at most `max_builds_per_cycle` (default 1) new tags to build.
4. Calls `build_tag` for each collected tag sequentially.

**`build_tag`**

Errors at any step update the `BuildRecord` and persist it as
`BuildStatus::Failed` via `finish_build_failed`. The daemon then continues
to the next tag. Steps:

| Step | Action |
| :--- | :----- |
| 1 | Notify `notify_build_start` (best-effort) |
| 2 | Save `InProgress` record to DB |
| 3 | Resolve flavors and host variables |
| 4 | Build via `BuildKitBuilder::build` (platforms: `linux/amd64`, `linux/arm64`) |
| 5 | Push via `OciRegistry::authenticate` + `push_image` |
| 6 | Save `Success` record with `image_tag`, `duration_secs`, `completed_at` |
| 7 | Notify `notify_build_success` (best-effort) |

On any failure at steps 3–5, `finish_build_failed` saves `Failed` status with
the error string and calls `notify_build_failure`.

**Unrecoverable error strategy**

Per the project error policy for Daemon mode: individual build failures are
logged, persisted to the database, sent as notifications, and do not stop the
daemon. Only startup failures (steps 1–6 of `run_daemon`) return `Err` and
exit the process.

---

## `error`

**Purpose:** Central error type hierarchy.

**Source:** `src/error.rs`

The top-level `Error` enum aggregates all module errors via `#[from]`
conversions, allowing `?` propagation across module boundaries:

```rust
pub enum Error {
    Config(ConfigError),
    Recipe(RecipeError),
    Builder(BuilderError),
    Vcs(VcsError),
    Registry(RegistryError),
    Database(DatabaseError),
    Notifier(NotifierError),
    Command(CommandError),
    Checker(CheckerError),
    Metrics(MetricsError),
    Io(std::io::Error),
}
```

All module-specific error types are derived with `thiserror` and implement
`std::error::Error`.
