# Feature: Builder (Template Engine, BuildKit Manager, Go Builder, Rust Builder)

## Context

The `builder` module is the core of the Dockermint pipeline. It encompasses
three distinct subsystems that work together to produce Docker images from
recipe specifications:

1. **Template Engine** -- variable interpolation engine that resolves recipe
   templates into concrete Dockerfile content.
2. **BuildKit Manager** -- manages Docker buildx builder instances and executes
   cross-platform builds.
3. **Go Recipe Builder** (`builder/go`) -- recipe-type-specific logic for
   generating multi-stage Dockerfiles for Go-based Cosmos SDK chains.
4. **Rust Recipe Builder** (`builder/rust`) -- recipe-type-specific logic for
   generating multi-stage Dockerfiles for Rust-based blockchain projects.

These subsystems correspond to pipeline steps 6, 7, and 8 in the core build
pipeline (architecture-overview.md Section 1.2).

Roadmap entry: Phase 0 -- Foundation, specs "Dockerfile generation and template
engine", "BuildKit cross-compilation manager", and "Go recipe builder"
(docs/ROADMAP.md)

---

## Requirements

1. [confirmed] Provide a `TemplateEngine` that resolves `{{UPPERCASE}}` host
   variables and `{{lowercase}}` build variables in recipe templates.
2. [confirmed] Support shell-type variables (`{ shell = "..." }`) that execute
   commands inside the builder container to capture dynamic values.
3. [confirmed] Provide an `ImageBuilder` trait for build execution, with a
   default BuildKit implementation behind the `builder-buildkit` feature gate.
4. [confirmed] BuildKit endpoint is configurable in `config.toml`
   (`[builder]` section). On each launch, CLI and Daemon verify the Docker
   context and create/destroy builders on the specified Docker socket URI.
5. [confirmed] Per-platform builder instances: `dockermint-amd64` and
   `dockermint-arm64`, managed by Dockermint (created if absent, optionally
   destroyed after builds).
6. [confirmed] Support both local and remote BuildKit endpoints.
7. [confirmed] Cross-compilation targets: `linux/amd64` and `linux/arm64`.
8. [confirmed] The Go recipe builder (`builder/go`) generates multi-stage
   Dockerfiles for Go-based chains (Cosmos SDK pattern).
9. [confirmed] The Rust recipe builder (`builder/rust`) generates multi-stage
   Dockerfiles for Rust-based chains using `cargo build`.
10. [confirmed] Dockerfile generation flow: recipe + resolved flavors ->
    template engine -> concrete Dockerfile content.
11. [confirmed] Adding a new chain does NOT require Rust code changes. Adding a
    new build system type requires a new submodule under `builder/`.
12. [confirmed] Flavor compatibility rules are expressed in recipe TOML
    (per CEO decision).
13. [confirmed] Runner base image mapping (`running_env` -> Docker image) is in
    the recipe TOML, NOT hardcoded in the Go or Rust builder (CEO decision B1).
14. [confirmed] Shell-type variables (not host ENV) are emitted as Dockerfile
    `RUN` commands inside the builder stage. Their output is forwarded to
    subsequent stages by file-copy (CEO decision B2).
15. [confirmed] `ImageBuilder.build()` ALWAYS uses `--load` locally. Push is a
    separate step handled by the `push` module. Never `--push` directly
    (CEO decision B3).
16. [confirmed] QEMU binfmt_misc setup for cross-platform builds is handled by
    the checker module. The builder module assumes QEMU is already set up
    (CEO decision B4).
17. [confirmed] The recipe `[scrapper]` section is consumed by the scrapper
    module. The builder receives already-cloned source (via `CloneInstructions`)
    from the scrapper (CEO decision B5).

---

## Architecture

### Module placement

```
src/builder/
    mod.rs              -- ImageBuilder trait + TemplateEngine + feature-gated re-exports
    error.rs            -- BuilderError, TemplateError enums (thiserror)
    types.rs            -- BuildContext, BuildOutput, Platform, TemplateVar structs
    template/
        mod.rs          -- TemplateEngine implementation
        variables.rs    -- Variable resolution (host, build, shell)
    buildkit/
        mod.rs          -- BuildKitBuilder (default ImageBuilder impl)
        manager.rs      -- Builder instance lifecycle (create, inspect, destroy)
        executor.rs     -- Build execution via docker buildx
    go/
        mod.rs          -- GoDockerfileGenerator: Dockerfile generation for Go recipes
        stages.rs       -- Multi-stage Dockerfile structure (builder stage, runner stage)
    rust/
        mod.rs          -- RustDockerfileGenerator: Dockerfile generation for Rust recipes
        stages.rs       -- Multi-stage Dockerfile structure (builder stage, runner stage)
```

### Trait design

#### ImageBuilder trait

The primary abstraction for build execution. Swappable via feature gate.

Design constraints:
- Not async at the trait level. Build execution is CPU/IO-bound and delegates
  to shell commands. The caller runs it on a blocking task
  (`tokio::task::spawn_blocking` or `rayon`) to avoid blocking the async
  runtime.
- `Send + Sync` for daemon mode.
- Manages its own builder lifecycle (create on init, destroy on drop or
  explicit call).

```rust
/// Executes container image builds from generated Dockerfiles.
///
/// Implementations manage the underlying build system (BuildKit, Podman,
/// etc.) and are selected at compile time via feature gates.
/// The default implementation is `BuildKitBuilder` (feature
/// `builder-buildkit`).
///
/// # Lifecycle
///
/// 1. `init_builders` -- create/verify per-platform builder instances.
/// 2. `build` -- execute one or more platform builds.
/// 3. `create_manifest` -- combine platform images into a multi-arch
///    manifest (for multi-platform builds).
/// 4. `destroy_builders` -- tear down builder instances (optional,
///    called on shutdown or error cleanup).
pub trait ImageBuilder: Send + Sync {
    /// Initialize per-platform builder instances.
    ///
    /// Creates builder instances if they do not exist, or verifies
    /// existing ones are healthy. Must be called before `build`.
    ///
    /// # Arguments
    ///
    /// * `config` - Builder configuration (platforms, endpoint, etc.)
    ///
    /// # Errors
    ///
    /// Returns `BuilderError::BuilderInit` if creation or verification fails.
    fn init_builders(&self, config: &BuilderConfig) -> Result<(), BuilderError>;

    /// Execute a build for a single platform.
    ///
    /// # Arguments
    ///
    /// * `context` - All information needed for the build: Dockerfile
    ///   content, build args, platform, image tag, labels.
    ///
    /// # Returns
    ///
    /// `BuildOutput` containing the built image reference and metadata.
    ///
    /// # Errors
    ///
    /// Returns `BuilderError` for Docker/BuildKit failures, missing
    /// prerequisites, or command execution errors.
    fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError>;

    /// Create a multi-arch manifest list referencing platform-specific images.
    ///
    /// # Arguments
    ///
    /// * `tag` - The manifest list tag (e.g., "cosmos-gaiad-goleveldb:v19.0.0-alpine3.23")
    /// * `platform_images` - References to per-platform images to include.
    ///
    /// # Errors
    ///
    /// Returns `BuilderError::Manifest` if manifest creation fails.
    fn create_manifest(
        &self,
        tag: &str,
        platform_images: &[BuildOutput],
    ) -> Result<(), BuilderError>;

    /// Destroy per-platform builder instances.
    ///
    /// Called during shutdown or error cleanup. Idempotent: safe to call
    /// even if builders do not exist.
    fn destroy_builders(&self) -> Result<(), BuilderError>;
}
```

#### DockerfileGenerator (internal, not a swappable trait)

Each recipe type (Go, Rust, etc.) implements Dockerfile generation. This is
selected at runtime based on the recipe `[header] type` field, not via a
feature gate. It is an internal dispatch mechanism within the builder module.

```rust
/// Generates a Dockerfile from a parsed recipe and resolved variables.
///
/// This is NOT a feature-gated trait. It is dispatched at runtime based
/// on the recipe type field. Each recipe type has its own implementation
/// under `builder/<type>/`.
pub(crate) trait DockerfileGenerator {
    /// Generate complete Dockerfile content for the given recipe.
    ///
    /// # Arguments
    ///
    /// * `recipe` - The parsed recipe structure.
    /// * `resolved_vars` - All variables (host + build) after template
    ///   engine resolution.
    /// * `platform` - Target platform for this Dockerfile.
    ///
    /// # Returns
    ///
    /// The Dockerfile content as a String.
    fn generate(
        &self,
        recipe: &ParsedRecipe,
        resolved_vars: &ResolvedVariables,
        platform: &Platform,
    ) -> Result<String, BuilderError>;
}
```

### Type design

#### Platform

```rust
/// A target platform for container image builds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Platform {
    LinuxAmd64,
    LinuxArm64,
}

impl Platform {
    /// Docker platform string (e.g., "linux/amd64").
    pub fn as_docker_platform(&self) -> &'static str;

    /// Builder instance name (e.g., "dockermint-amd64").
    pub fn builder_name(&self) -> &'static str;

    /// Architecture string matching recipe conventions (e.g., "x86_64").
    pub fn arch_str(&self) -> &'static str;
}
```

#### BuilderConfig

```rust
/// Configuration for the ImageBuilder, deserialized from config.toml
/// `[builder]` section.
///
/// More than 3 configuration values, so grouped into a dedicated struct
/// per CLAUDE.md config struct pattern.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderConfig {
    /// Target platforms for builds.
    /// Default: ["linux/amd64", "linux/arm64"]
    pub platforms: Vec<String>,

    /// Docker socket URI for BuildKit.
    /// Examples: "unix:///var/run/docker.sock", "tcp://remote:2376"
    /// Default: "unix:///var/run/docker.sock"
    pub docker_host: Option<String>,

    /// Whether to destroy builder instances after builds complete.
    /// Default: false (keep builders for reuse).
    pub cleanup_builders: Option<bool>,
}
```

#### BuildContext

```rust
/// All information needed to execute a single-platform build.
#[derive(Debug, Clone)]
pub struct BuildContext {
    /// Recipe name (e.g., "cosmos-gaiad").
    pub recipe_name: String,

    /// Version being built (e.g., "v19.0.0").
    pub version: String,

    /// Generated Dockerfile content.
    pub dockerfile: String,

    /// Target platform for this build.
    pub platform: Platform,

    /// Image tag for this platform-specific image.
    pub image_tag: String,

    /// OCI labels from recipe `[labels]` section (already resolved).
    pub labels: Vec<(String, String)>,

    /// Build arguments to pass via --build-arg.
    pub build_args: Vec<(String, String)>,
}
```

#### BuildOutput

```rust
/// Result of a successful single-platform build.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOutput {
    /// Full image reference (registry/name:tag or local name:tag).
    pub image_ref: String,

    /// Platform this image was built for.
    pub platform: Platform,

    /// Image digest (sha256:...).
    pub digest: String,

    /// Build duration.
    pub duration: std::time::Duration,
}
```

#### Template variable types

```rust
/// A template variable definition from a recipe.
#[derive(Debug, Clone)]
pub enum TemplateVar {
    /// A host variable resolved before Dockerfile generation.
    /// Syntax: {{UPPERCASE}}
    /// Source: Dockermint internals or host environment.
    Host {
        name: String,
        value: String,
    },

    /// A build variable resolved during Dockerfile generation.
    /// Syntax: {{lowercase}}
    /// Source: recipe [variables] section, resolved flavors, or profiles.
    Build {
        name: String,
        value: TemplateVarValue,
    },
}

/// The value of a build variable.
#[derive(Debug, Clone)]
pub enum TemplateVarValue {
    /// A literal string value.
    Literal(String),

    /// A shell command whose stdout output becomes the value.
    /// Executed inside the builder container.
    Shell {
        command: String,
    },
}
```

#### ResolvedVariables

```rust
/// All variables after resolution, ready for template interpolation.
///
/// Maps variable names to their resolved string values.
/// Host variables are keyed as UPPERCASE, build variables as lowercase.
#[derive(Debug, Clone)]
pub struct ResolvedVariables {
    vars: std::collections::HashMap<String, String>,
}

impl ResolvedVariables {
    pub fn get(&self, name: &str) -> Option<&str>;
    pub fn insert(&mut self, name: String, value: String);
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)>;
}
```

#### Derive strategy

- `Platform`: `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash` (used as map key).
- `BuilderConfig`: `Debug`, `Clone`, `Deserialize`.
- `BuildContext`: `Debug`, `Clone`.
- `BuildOutput`: `Debug`, `Clone`, `PartialEq`.
- `TemplateVar`, `TemplateVarValue`: `Debug`, `Clone`.
- `ResolvedVariables`: `Debug`, `Clone`.

### Feature gate

| Feature flag | Module | Default |
| :----------- | :----- | :------ |
| `builder-buildkit` | builder | Yes |

From architecture-overview.md Section 3.1. Compile-time enforcement:

```
#[cfg(not(feature = "builder-buildkit"))]
compile_error!("At least one builder backend must be enabled (e.g., builder-buildkit)");
```

The template engine, Go builder submodule, and Rust builder submodule are NOT
feature-gated -- they are core components always compiled in.

### Configuration

New config.toml keys in `[builder]` section (updating architecture-overview.md
Section 4.2):

```toml
[builder]
platforms = ["linux/amd64", "linux/arm64"]
docker_host = "unix:///var/run/docker.sock"  # Docker socket URI for BuildKit
cleanup_builders = false                      # Destroy builders after build
```

| Key | Type | Default | Notes |
| :-- | :--- | :------ | :---- |
| `platforms` | Array of strings | `["linux/amd64", "linux/arm64"]` | Target platforms |
| `docker_host` | String (optional) | System default Docker socket | URI for Docker/BuildKit endpoint. Per CEO decision: configurable in config.toml |
| `cleanup_builders` | Boolean (optional) | `false` | Whether to destroy builder instances after builds complete |

CLI flag overrides:

| Flag | Overrides | Notes |
| :--- | :-------- | :---- |
| `--platform` | `builder.platforms` | Default: `all` (linux/amd64 + linux/arm64). Narrow to single: `--platform linux/amd64` |
| `--docker-host` | `builder.docker_host` | Docker socket URI |

### Error types

```rust
/// Errors originating from the builder module.
///
/// Owned by the `builder` module. Defined in `src/builder/error.rs`.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// Builder instance creation or verification failed.
    #[error("failed to initialize builder '{builder_name}': {message}")]
    BuilderInit {
        builder_name: String,
        message: String,
    },

    /// Builder instance destruction failed.
    #[error("failed to destroy builder '{builder_name}': {message}")]
    BuilderDestroy {
        builder_name: String,
        message: String,
    },

    /// Docker/BuildKit build command failed.
    #[error("build failed for {recipe_name} {version} on {platform}: {message}")]
    BuildFailed {
        recipe_name: String,
        version: String,
        platform: String,
        message: String,
        exit_code: Option<i32>,
    },

    /// Multi-arch manifest creation failed.
    #[error("manifest creation failed for {tag}: {message}")]
    Manifest {
        tag: String,
        message: String,
    },

    /// Docker daemon is not reachable or Docker socket is invalid.
    #[error("Docker not reachable at '{endpoint}': {message}")]
    DockerNotReachable {
        endpoint: String,
        message: String,
    },

    /// Recipe type is not supported (no DockerfileGenerator for it).
    #[error("unsupported recipe type: {recipe_type}")]
    UnsupportedRecipeType {
        recipe_type: String,
    },

    /// Shell command execution failed (for commands module delegation).
    #[error("command execution failed: {message}")]
    CommandFailed {
        message: String,
        #[source]
        source: Option<std::io::Error>,
    },
}

/// Errors originating from the template engine.
///
/// Separate from BuilderError because the template engine is a distinct
/// subsystem within the builder module.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// A referenced variable is not defined in host vars, build vars,
    /// or recipe variables.
    #[error("undefined template variable: {name}")]
    UndefinedVariable {
        name: String,
    },

    /// A shell-type variable command failed to execute.
    #[error("shell variable '{name}' failed: {message}")]
    ShellExecution {
        name: String,
        message: String,
        exit_code: Option<i32>,
    },

    /// Template syntax error (malformed {{ }} delimiters).
    #[error("template syntax error at position {position}: {message}")]
    Syntax {
        position: usize,
        message: String,
    },

    /// Flavor compatibility violation detected.
    #[error("incompatible flavors: {message}")]
    IncompatibleFlavors {
        message: String,
    },
}
```

---

## Subsystem 1: Template Engine

### Purpose

The template engine resolves variables in recipe TOML values before Dockerfile
generation. It is the bridge between the declarative recipe format and
concrete build instructions.

### Variable resolution

#### Variable syntax

| Pattern | Type | Resolution source | Resolution time |
| :------ | :--- | :---------------- | :-------------- |
| `{{UPPERCASE}}` | Host variable | Dockermint internals, host env | Before Dockerfile generation |
| `{{lowercase}}` | Build variable | Recipe `[variables]`, resolved flavors, profiles | During Dockerfile generation |

#### Host variables (UPPERCASE)

These are provided by Dockermint itself. They are known before any recipe
processing begins.

| Variable | Source | Example value |
| :------- | :----- | :------------ |
| `{{HOST_ARCH}}` | Host system architecture detection | `x86_64`, `aarch64` |
| `{{GH_PAT}}` | `.env` file | (secret, never logged) |
| `{{GH_USER}}` | `.env` file | (secret, never logged) |
| `{{CREATION_TIMESTAMP}}` | Current UTC time (RFC 3339) | `2026-04-13T10:30:00Z` |
| `{{SEMVER_TAG}}` | Version being built (from VCS fetch) | `v19.0.0` |
| `{{BUILD_TAGS_COMMA_SEP}}` | Resolved `build_tags` flavor, comma-joined | `netgo,muslc` |

Host variables containing secrets (`GH_PAT`, `GH_USER`) are wrapped in
`secrecy::SecretString` within the template engine and are only unwrapped at
the point of interpolation into Dockerfile content. They are never logged.

#### Build variables (lowercase)

These come from:
1. Recipe `[variables]` section (literal or shell commands).
2. Resolved flavor values (e.g., `{{db_backend}}` = "goleveldb").
3. Profile variables from `[profiles.<dimension>.<value>]` sections
   (e.g., `{{denom}}` from `[profiles.network.mainnet]`).

Shell-type variables (`{ shell = "..." }`) are NOT executed by the template
engine on the host. They are emitted as Dockerfile `RUN` commands inside the
builder stage (CEO decision B2). Their output is captured to files in the
builder stage and forwarded to the runner stage via `COPY --from=builder`
(file-copy approach). This avoids multi-stage ENV propagation issues.

Example flow for a shell variable `repo_version = { shell = "git describe ..." }`:

```dockerfile
# Builder stage: execute shell command and write output to file
RUN git describe --exact-match 2>/dev/null || echo "unknown" > /tmp/vars/repo_version

# Runner stage: read value from copied file
COPY --from=builder /tmp/vars/ /tmp/vars/
```

The template engine only needs to know the command string to emit the correct
`RUN` instruction and the corresponding `COPY` in the runner stage.

#### Resolution algorithm

```
1. Collect host variables from Dockermint internals + .env
2. Collect flavor values from resolution chain
   (CLI args > config.toml > recipe defaults)
3. Collect profile variables from matching profile sections
4. Collect recipe [variables] (literal values resolved immediately,
   shell commands deferred to Dockerfile RUN)
5. For each template string in the recipe:
   a. Scan for {{PATTERN}} tokens
   b. If UPPERCASE: look up in host variables
   c. If lowercase: look up in build variables (flavors, profiles, [variables])
   d. If not found: return TemplateError::UndefinedVariable
6. Return fully resolved string (or Dockerfile RUN for shell vars)
```

### Template engine interface

```rust
/// Resolves template variables in recipe strings.
///
/// Not a trait (not swappable). Single implementation that is always
/// compiled in.
pub struct TemplateEngine {
    host_vars: HashMap<String, String>,
    secret_vars: HashMap<String, secrecy::SecretString>,
}

impl TemplateEngine {
    /// Create a new TemplateEngine with host variables pre-loaded.
    pub fn new(
        host_vars: HashMap<String, String>,
        secret_vars: HashMap<String, secrecy::SecretString>,
    ) -> Self;

    /// Resolve all {{VARIABLE}} patterns in the input string.
    ///
    /// # Arguments
    ///
    /// * `template` - String containing {{VARIABLE}} patterns.
    /// * `build_vars` - Build-time variables (lowercase) from recipe.
    ///
    /// # Returns
    ///
    /// The resolved string with all variables substituted.
    ///
    /// # Errors
    ///
    /// Returns `TemplateError::UndefinedVariable` if a referenced
    /// variable is not found in either host or build variables.
    /// Returns `TemplateError::Syntax` for malformed delimiters.
    pub fn resolve(
        &self,
        template: &str,
        build_vars: &ResolvedVariables,
    ) -> Result<String, TemplateError>;
}
```

---

## Subsystem 2: BuildKit Manager

### Purpose

Manages Docker buildx builder instances and executes builds via the Docker CLI.
This is the default `ImageBuilder` implementation behind the `builder-buildkit`
feature gate.

### Builder lifecycle

On each Dockermint launch (CLI or daemon):

```
[1. Verify Docker]
 Check that Docker daemon is reachable at the configured docker_host.
 If not: return BuilderError::DockerNotReachable
         |
         v
[2. Inspect Builders]
 For each target platform, check if a builder instance exists:
   docker buildx inspect dockermint-amd64
   docker buildx inspect dockermint-arm64
         |
         v
[3. Create Missing Builders]
 If a builder does not exist, create it:
   docker buildx create \
     --name dockermint-amd64 \
     --platform linux/amd64 \
     --driver docker-container \
     [--driver-opt endpoint=<docker_host>]
         |
         v
[4. Bootstrap Builders]
 Ensure builders are ready:
   docker buildx inspect --bootstrap dockermint-amd64
         |
         v
[5. Execute Builds]
 Run builds using the appropriate per-platform builder:
   docker buildx build \
     --builder dockermint-amd64 \
     --platform linux/amd64 \
     --file <Dockerfile> \
     --tag <tag> \
     --label <key>=<value> \
     --build-arg <key>=<value> \
     --load \
     <context>

   NOTE: The builder ALWAYS uses --load. Images are loaded into the
   local Docker daemon. Push to a registry is a separate step handled
   by the push module. The builder never uses --push directly
   (CEO decision B3).
         |
         v
[6. Create Manifest (if multi-platform)]
   docker buildx imagetools create \
     --tag <manifest-tag> \
     <amd64-image> <arm64-image>
         |
         v
[7. Cleanup (optional)]
 If cleanup_builders is true:
   docker buildx rm dockermint-amd64
   docker buildx rm dockermint-arm64
```

### Local vs remote BuildKit

| Mode | `docker_host` value | Behavior |
| :--- | :------------------ | :------- |
| Local | `unix:///var/run/docker.sock` (default) | Builders use local Docker daemon |
| Remote | `tcp://remote-host:2376` | Builders connect to remote Docker/BuildKit endpoint |

The `docker_host` is passed via the `DOCKER_HOST` environment variable when
invoking Docker CLI commands. The builder module sets this env var for each
command execution, rather than modifying the system-wide Docker context.

### Per-platform builders

| Builder name | Platform | Purpose |
| :----------- | :------- | :------ |
| `dockermint-amd64` | `linux/amd64` | Builds x86_64 images |
| `dockermint-arm64` | `linux/arm64` | Builds ARM64 images |

Each builder is a Docker buildx builder instance using the
`docker-container` driver. This driver runs BuildKit in a container,
providing isolation and cross-platform build capabilities.

### Cross-compilation strategy

For each version to build:

```
1. For each target platform in config.platforms:
   a. Select the corresponding builder (dockermint-amd64 or dockermint-arm64)
   b. Generate a platform-specific Dockerfile (if needed -- typically the
      same Dockerfile works for both via --platform flag)
   c. Execute: docker buildx build --builder <name> --platform <platform>
   d. Collect BuildOutput (image ref, digest, duration)

2. If multiple platforms were built:
   a. Create a multi-arch manifest list referencing all platform images
   b. The manifest tag is the recipe's [image] tag after variable resolution
```

BuildKit handles the actual cross-compilation via QEMU emulation (for
architectures different from the host) or native building (when the host
matches the target).

QEMU binfmt_misc registration is verified and set up by the **checker module**
(the same module that creates BuildKit containers). The builder module assumes
QEMU is already configured and does not perform any QEMU setup itself
(CEO decision B4).

---

## Subsystem 3: Go Recipe Builder

### Purpose

The `builder/go` submodule generates multi-stage Dockerfiles for Go-based
Cosmos SDK chains. It is selected when the recipe `[header] type` is
`"golang"`.

### Dockerfile generation flow

```
Recipe TOML (parsed)
  + Resolved flavors
  + Resolved variables (from template engine)
  + CloneInstructions (from scrapper module)
  |
  v
GoDockerfileGenerator.generate()
  |
  +-- Stage 1: Builder Stage
  |     Base image: recipe [scrapper] image (e.g., "golang:1.23-alpine3.21")
  |     Install OS deps: recipe [builder.install] (alpine or ubuntu)
  |     Clone repo: embed CloneInstructions from scrapper module
  |       (the scrapper owns [scrapper] section and produces Dockerfile
  |        lines for authenticated/unauthenticated git clone)
  |     Shell variables: emitted as Dockerfile RUN commands (CEO decision B2)
  |       Output values are written to files and forwarded to the runner
  |       stage via COPY --from=builder (file-copy approach)
  |     Pre-build steps: recipe [[pre_build]] (conditional)
  |     Set build env: recipe [build.env]
  |     Execute Go build: go build with linker flags, build tags, build path
  |
  +-- Stage 2: Runner Stage
  |     Base image: from recipe [flavours.images] mapping (CEO decision B1)
  |       e.g., running_env "alpine3.23" -> image "alpine:3.23" (in recipe)
  |     Copy binaries: recipe [copy] section
  |     Copy dynamic libs: recipe [copy.<flavor>] conditional sections
  |     Copy shell variable outputs: COPY --from=builder (file-copy)
  |     Create user: if running_user != "root", use recipe [user.*] section
  |     Expose ports: recipe [expose] section
  |     Set labels: recipe [labels] section
  |     Set entrypoint: the binary marked type = "entrypoint" in [copy]
  |
  v
Complete Dockerfile (String)
```

### Multi-stage Dockerfile structure (Go)

```dockerfile
# ============================================================================
# Stage 1: Builder
# ============================================================================
FROM golang:1.23-alpine3.21 AS builder

# Install OS build dependencies
RUN apk add --no-cache alpine-sdk linux-headers

# --- Source acquisition (from scrapper CloneInstructions) ---
# These lines are produced by the scrapper module and embedded verbatim.
# The builder module does NOT generate clone logic itself (CEO decision B5).
ARG GH_USER
ARG GH_PAT
RUN if [ -n "$GH_PAT" ]; then \
      git config --global url."https://${GH_USER}:${GH_PAT}@github.com/".insteadOf "https://github.com/"; \
    fi
RUN git clone --depth 1 --branch {{SEMVER_TAG}} https://github.com/cosmos/gaia /workspace
WORKDIR /workspace
# --- End scrapper CloneInstructions ---

# Conditional pre-build steps (e.g., download wasmvm for static builds)
# [[ Only emitted if condition matches resolved flavor ]]
ADD https://github.com/.../libwasmvm_muslc.x86_64.a /lib/libwasmvm_muslc.x86_64.a

# Shell variables: emitted as RUN commands, output written to files
# (CEO decision B2: shell vars -> Dockerfile RUN -> file-copy to runner)
RUN mkdir -p /tmp/vars && \
    git log -1 --format='%H' > /tmp/vars/repo_commit && \
    (git describe --exact-match 2>/dev/null || echo "unknown") > /tmp/vars/repo_version

# Set build environment
ENV CGO_ENABLED=1

# Execute Go build (reads shell var files for linker flags)
RUN REPO_VERSION=$(cat /tmp/vars/repo_version) && \
    REPO_COMMIT=$(cat /tmp/vars/repo_commit) && \
    go build \
    -tags "netgo,muslc,goleveldb" \
    -ldflags "-linkmode=external -w -s -X 'pkg/version.Version=${REPO_VERSION}' -X 'pkg/version.Commit=${REPO_COMMIT}'" \
    -o /go/bin/gaiad \
    ./cmd/gaiad

# ============================================================================
# Stage 2: Runner
# Base image from recipe [flavours.images] mapping (CEO decision B1)
# ============================================================================
FROM alpine:3.23

# Copy built binary
COPY --from=builder /go/bin/gaiad /usr/bin/gaiad

# Copy shell variable output files (for downstream use if needed)
COPY --from=builder /tmp/vars/ /tmp/vars/

# Copy dynamic libraries (conditional on binary_type=dynamic)
# COPY --from=builder /go/pkg/mod/.../libwasmvm.x86_64.so /lib/libwasmvm.x86_64.so

# Create non-root user (if running_user != "root")
# RUN addgroup -g 10000 dockermint && adduser -u 10000 -G dockermint ...
# USER dockermint

# Expose ports
EXPOSE 26656 26657 26660 1317 9090 9091

# Labels
LABEL org.opencontainers.image.created="2026-04-13T10:30:00Z"
LABEL org.opencontainers.image.version="v19.0.0"
# ...

ENTRYPOINT ["gaiad"]
```

NOTE: The builder always uses `--load` to load the built image into the local
Docker daemon (CEO decision B3). Push to a registry is handled separately by
the push module.

### Go build command construction

The Go build command is assembled from recipe fields:

```
go build \
  -tags "{{BUILD_TAGS_COMMA_SEP}}" \
  -ldflags "{{resolved_linker_flags}} {{resolved_linker_variables}}" \
  -o /go/bin/{{binary_name}} \
  {{build_path}}
```

Where:
- `{{BUILD_TAGS_COMMA_SEP}}` = comma-joined `build_tags` flavor values
- Linker flags = `[build.linker.flags]` value keyed by `binary_type` flavor
  (e.g., `static` or `dynamic`)
- Linker variables = `[build.linker.variables]` entries formatted as
  `-X 'pkg.Var={{value}}'` with variables resolved by the template engine
- `{{binary_name}}` = recipe `[header] binary_name`
- `{{build_path}}` = recipe `[build.path] path`

### Runner base image mapping (recipe-defined)

The runner stage base image is determined by the `running_env` flavor, with
the mapping defined in the recipe TOML (CEO decision B1). This mapping is
NOT hardcoded in the Go builder. Each recipe defines its own
`[flavours.images]` section mapping flavor values to Docker base images:

```toml
# In recipe TOML:
[flavours.images]
alpine3.23 = "alpine:3.23"
bookworm = "debian:bookworm-slim"
distroless = "gcr.io/distroless/static-debian12"
```

The Go builder reads the resolved `running_env` flavor value, looks up the
corresponding base image in `[flavours.images]`, and uses it as the `FROM`
instruction for the runner stage. This approach allows new runner environments
to be added per-recipe without modifying Rust code.

| running_env value | Recipe `[flavours.images]` key | Docker base image |
| :---------------- | :---------------------------- | :---------------- |
| `alpine3.23` | `alpine3.23` | `alpine:3.23` |
| `bookworm` | `bookworm` | `debian:bookworm-slim` |
| `distroless` | `distroless` | `gcr.io/distroless/static-debian12` |

### Conditional sections

Several recipe sections are conditional on flavor values:

| Section | Condition | Example |
| :------ | :-------- | :------ |
| `[[pre_build]]` | `condition` field matches a flavor value | `condition = "static"` -> only emitted when `binary_type = "static"` |
| `[copy.<flavor_value>]` | Key matches a resolved flavor value | `[copy.dynamic]` -> only copied when `binary_type = "dynamic"` |

The Dockerfile generator checks each conditional section against the resolved
flavors and only emits the corresponding Dockerfile instructions when the
condition matches.

### Profile variable injection

When a recipe has `[profiles.<dimension>.<value>]` sections (e.g., Kyve's
network profiles), the Go builder:

1. Looks up the resolved value for the dimension (e.g., `network = "mainnet"`).
2. Loads all key-value pairs from the matching profile section.
3. Injects them as build variables available to the template engine and
   linker variable resolution.

```
[profiles.network.mainnet]
denom = "ukyve"          ->  {{denom}} = "ukyve" in templates
team_tge = "..."         ->  {{team_tge}} = "..." in templates
```

---

## Subsystem 4: Rust Recipe Builder

### Purpose

The `builder/rust` submodule generates multi-stage Dockerfiles for Rust-based
blockchain projects. It is selected when the recipe `[header] type` is
`"rust"`.

This follows the same architecture as the Go recipe builder: recipe-driven,
template-based Dockerfile generation dispatched at runtime via the
`DockerfileGenerator` trait. No feature gate -- always compiled in.

### Dockerfile generation flow

```
Recipe TOML (parsed, type = "rust")
  + Resolved flavors
  + Resolved variables (from template engine)
  + CloneInstructions (from scrapper module)
  |
  v
RustDockerfileGenerator.generate()
  |
  +-- Stage 1: Builder Stage
  |     Base image: recipe [scrapper] image (e.g., "rust:1.82-alpine3.21")
  |     Install OS deps: recipe [builder.install] (alpine or ubuntu)
  |     Clone repo: embed CloneInstructions from scrapper module
  |       (the scrapper owns [scrapper] section and produces Dockerfile
  |        lines for authenticated/unauthenticated git clone)
  |     Shell variables: emitted as Dockerfile RUN commands (CEO decision B2)
  |       Output values are written to files and forwarded to the runner
  |       stage via COPY --from=builder (file-copy approach)
  |     Pre-build steps: recipe [[pre_build]] (conditional)
  |     Set build env: recipe [build.env]
  |     Execute Rust build: cargo build with target, features, profile
  |
  +-- Stage 2: Runner Stage
  |     Base image: from recipe [flavours.images] mapping (CEO decision B1)
  |       e.g., running_env "alpine3.23" -> image "alpine:3.23" (in recipe)
  |     Copy binaries: recipe [copy] section
  |     Copy dynamic libs: recipe [copy.<flavor>] conditional sections
  |     Copy shell variable outputs: COPY --from=builder (file-copy)
  |     Create user: if running_user != "root", use recipe [user.*] section
  |     Expose ports: recipe [expose] section
  |     Set labels: recipe [labels] section
  |     Set entrypoint: the binary marked type = "entrypoint" in [copy]
  |
  v
Complete Dockerfile (String)
```

### Multi-stage Dockerfile structure (Rust)

```dockerfile
# ============================================================================
# Stage 1: Builder
# ============================================================================
FROM rust:1.82-alpine3.21 AS builder

# Install OS build dependencies
RUN apk add --no-cache alpine-sdk linux-headers protobuf-dev

# --- Source acquisition (from scrapper CloneInstructions) ---
# These lines are produced by the scrapper module and embedded verbatim.
# The builder module does NOT generate clone logic itself (CEO decision B5).
ARG GH_USER
ARG GH_PAT
RUN if [ -n "$GH_PAT" ]; then \
      git config --global url."https://${GH_USER}:${GH_PAT}@github.com/".insteadOf "https://github.com/"; \
    fi
RUN git clone --depth 1 --branch {{SEMVER_TAG}} https://github.com/example/chain /workspace
WORKDIR /workspace
# --- End scrapper CloneInstructions ---

# Shell variables: emitted as RUN commands, output written to files
# (CEO decision B2: shell vars -> Dockerfile RUN -> file-copy to runner)
RUN mkdir -p /tmp/vars && \
    git log -1 --format='%H' > /tmp/vars/repo_commit && \
    (git describe --exact-match 2>/dev/null || echo "unknown") > /tmp/vars/repo_version

# Conditional pre-build steps (e.g., protobuf generation)
# [[ Only emitted if condition matches resolved flavor ]]

# Set build environment
ENV CARGO_TARGET_DIR=/workspace/target

# Execute Rust build
RUN REPO_VERSION=$(cat /tmp/vars/repo_version) && \
    REPO_COMMIT=$(cat /tmp/vars/repo_commit) && \
    cargo build \
    --release \
    --features "{{cargo_features}}" \
    --target {{cargo_target}} \
    --bin {{binary_name}}

# ============================================================================
# Stage 2: Runner
# Base image from recipe [flavours.images] mapping (CEO decision B1)
# ============================================================================
FROM alpine:3.23

# Copy built binary
COPY --from=builder /workspace/target/{{cargo_target}}/release/{{binary_name}} /usr/bin/{{binary_name}}

# Copy shell variable output files (for downstream use if needed)
COPY --from=builder /tmp/vars/ /tmp/vars/

# Create non-root user (if running_user != "root")
# RUN addgroup -g 10000 dockermint && adduser -u 10000 -G dockermint ...
# USER dockermint

# Expose ports
EXPOSE 26656 26657 26660 1317 9090 9091

# Labels
LABEL org.opencontainers.image.created="2026-04-13T10:30:00Z"
LABEL org.opencontainers.image.version="v1.0.0"
# ...

ENTRYPOINT ["{{binary_name}}"]
```

NOTE: The builder always uses `--load` to load the built image into the local
Docker daemon (CEO decision B3). Push to a registry is handled separately by
the push module.

### Rust build command construction

The Rust build command is assembled from recipe fields:

```
cargo build \
  --release \
  --features "{{cargo_features}}" \
  --target {{cargo_target}} \
  --bin {{binary_name}}
```

Where:
- `{{cargo_features}}` = comma-joined Cargo feature flags from the recipe
  (analogous to Go build tags)
- `{{cargo_target}}` = Rust target triple (e.g., `x86_64-unknown-linux-musl`)
  derived from the target platform
- `{{binary_name}}` = recipe `[header] binary_name`

The build profile is always `--release`. Linker flags and environment variables
(e.g., `RUSTFLAGS`, `CC`, `AR` for cross-compilation) are read from the
recipe `[build.env]` section.

### Runner base image mapping (recipe-defined)

Identical to the Go builder approach. The runner stage base image is determined
by the `running_env` flavor, with the mapping defined in the recipe TOML
(CEO decision B1). The Rust builder reads the resolved `running_env` flavor
value, looks up the corresponding base image in `[flavours.images]`, and uses
it as the `FROM` instruction for the runner stage.

### Conditional sections

Same mechanism as the Go builder:

| Section | Condition | Example |
| :------ | :-------- | :------ |
| `[[pre_build]]` | `condition` field matches a flavor value | `condition = "static"` -> only emitted when `binary_type = "static"` |
| `[copy.<flavor_value>]` | Key matches a resolved flavor value | `[copy.dynamic]` -> only copied when `binary_type = "dynamic"` |

### Supported Rust chains

TBD -- specific Rust-based chains to be supported will be determined by the
CEO. The architecture and builder pattern are ready for any Rust-based project
that follows a standard `cargo build` workflow.

---

## Interface contract

```rust
// -- src/builder/mod.rs --

pub trait ImageBuilder: Send + Sync {
    fn init_builders(&self, config: &BuilderConfig) -> Result<(), BuilderError>;
    fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError>;
    fn create_manifest(
        &self,
        tag: &str,
        platform_images: &[BuildOutput],
    ) -> Result<(), BuilderError>;
    fn destroy_builders(&self) -> Result<(), BuilderError>;
}

// -- src/builder/template/mod.rs --

pub struct TemplateEngine { /* ... */ }

impl TemplateEngine {
    pub fn new(
        host_vars: HashMap<String, String>,
        secret_vars: HashMap<String, secrecy::SecretString>,
    ) -> Self;

    pub fn resolve(
        &self,
        template: &str,
        build_vars: &ResolvedVariables,
    ) -> Result<String, TemplateError>;
}

// -- src/builder/go/mod.rs --

pub(crate) struct GoDockerfileGenerator;

impl DockerfileGenerator for GoDockerfileGenerator {
    fn generate(
        &self,
        recipe: &ParsedRecipe,
        resolved_vars: &ResolvedVariables,
        platform: &Platform,
    ) -> Result<String, BuilderError>;
}

// -- src/builder/rust/mod.rs --

pub(crate) struct RustDockerfileGenerator;

impl DockerfileGenerator for RustDockerfileGenerator {
    fn generate(
        &self,
        recipe: &ParsedRecipe,
        resolved_vars: &ResolvedVariables,
        platform: &Platform,
    ) -> Result<String, BuilderError>;
}

// -- src/builder/buildkit/mod.rs (behind feature "builder-buildkit") --

pub struct BuildKitBuilder { /* ... */ }

impl ImageBuilder for BuildKitBuilder {
    fn init_builders(&self, config: &BuilderConfig) -> Result<(), BuilderError>;
    fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError>;
    fn create_manifest(
        &self,
        tag: &str,
        platform_images: &[BuildOutput],
    ) -> Result<(), BuilderError>;
    fn destroy_builders(&self) -> Result<(), BuilderError>;
}
```

---

## Module interaction diagram

```
+----------+    +----------+    +----------+
|  recipe  |--->| template |--->| go/      |  or  | rust/    |
| (parsed) |    | engine   |    | Dockerfile|      | Dockerfile|
+----------+    +----+-----+    | generator |      | generator |
                     |          +-----+-----+      +-----+-----+
              resolved vars           |                  |
                     |          Dockerfile (String)      |
                     v                |                  |
              +------+------+         |                  |
              | ResolvedVars|<--------+------------------+
              +------+------+
                     |
                     v
+----------+   +----+------+    +----------+
| config   |-->| BuildKit  |--->|  push    |
| (builder |   | Builder   |    | (registry|
|  section)|   | (exec)    |    |  push)   |
+----------+   +-----+-----+    +----------+
                     |
              docker buildx CLI
                     |
                     v
              +------+------+
              | Docker      |
              | daemon      |
              | (local or   |
              |  remote)    |
              +-------------+

Builder instance lifecycle:

  init_builders()         build() x N           create_manifest()
       |                      |                       |
       v                      v                       v
  +----------+         +-------------+         +-------------+
  | create   |         | buildx      |         | imagetools  |
  | buildx   |-------->| build       |-------->| create      |
  | instances|         | --builder   |         | manifest    |
  +----------+         | --platform  |         +------+------+
                       +-------------+                |
                                               destroy_builders()
                                                (optional cleanup)
```

---

## Dockerfile generation end-to-end flow

```
[1. Recipe Parsed]
     recipe.toml -> ParsedRecipe struct (by recipe module)
                |
[2. Flavor Resolution]
     CLI args > config.toml > recipe defaults
     Validate against [flavours.available]
     Check compatibility rules in recipe TOML
                |
[3. Variable Collection]
     Host vars: HOST_ARCH, GH_PAT, CREATION_TIMESTAMP, SEMVER_TAG, etc.
     Flavor vars: db_backend, binary_type, running_env, build_tags, etc.
     Profile vars: denom, team_tge, etc. (from matching [profiles] section)
     Recipe vars: [variables] section (literal or shell)
                |
[4. Template Resolution]
     TemplateEngine.resolve() for each template string in recipe
     {{UPPERCASE}} -> host var lookup
     {{lowercase}} -> build var lookup
     Missing var -> TemplateError::UndefinedVariable
                |
[5. Dockerfile Generation]
     DockerfileGenerator.generate() (dispatched by recipe type)
     For type="golang" -> GoDockerfileGenerator
     For type="rust"   -> RustDockerfileGenerator
     Produces multi-stage Dockerfile string
                |
[6. Build Execution]
     ImageBuilder.build() for each platform
     docker buildx build with generated Dockerfile
                |
[7. Manifest Creation]
     ImageBuilder.create_manifest() if multi-platform
     docker buildx imagetools create
```

---

## Testing strategy

### Unit tests

| Test | What it validates |
| :--- | :---------------- |
| Template engine: simple host var substitution | `{{HOST_ARCH}}` -> `"x86_64"` |
| Template engine: simple build var substitution | `{{db_backend}}` -> `"goleveldb"` |
| Template engine: mixed vars in one string | Multiple patterns in one template |
| Template engine: undefined variable | Returns `TemplateError::UndefinedVariable` |
| Template engine: malformed delimiters | Returns `TemplateError::Syntax` |
| Template engine: secret var does not leak in Debug | `secrecy::SecretString` redaction |
| Template engine: empty template | Returns empty string |
| Template engine: no variables in string | Returns input unchanged |
| Platform: enum to string conversions | `as_docker_platform`, `builder_name`, `arch_str` |
| Go generator: linker flags assembly | Correct `-ldflags` string for static/dynamic |
| Go generator: build tags assembly | Correct `-tags` string from flavor array |
| Go generator: conditional pre_build | Section emitted only when condition matches |
| Go generator: conditional copy | `[copy.dynamic]` emitted only when `binary_type = "dynamic"` |
| Go generator: user creation | User/group commands emitted when `running_user != "root"` |
| Go generator: runner image mapping | Correct base image for each `running_env` value |
| Go generator: profile variable injection | Kyve network profile variables injected correctly |
| Rust generator: cargo build command assembly | Correct `cargo build` with --features, --target, --bin |
| Rust generator: conditional pre_build | Section emitted only when condition matches |
| Rust generator: conditional copy | `[copy.dynamic]` emitted only when `binary_type = "dynamic"` |
| Rust generator: user creation | User/group commands emitted when `running_user != "root"` |
| Rust generator: runner image mapping | Correct base image for each `running_env` value |
| Rust generator: target triple from platform | Correct `--target` for linux/amd64 and linux/arm64 |
| BuilderConfig deserialization | Correct parsing of config.toml `[builder]` section |

### Integration tests (mocked Docker CLI)

| Test | What it validates |
| :--- | :---------------- |
| BuildKitBuilder: init creates builders | `docker buildx create` called with correct args |
| BuildKitBuilder: init skips existing builders | No creation when `docker buildx inspect` succeeds |
| BuildKitBuilder: build invokes buildx | `docker buildx build` called with correct flags |
| BuildKitBuilder: destroy removes builders | `docker buildx rm` called for each builder |
| BuildKitBuilder: Docker not reachable | Returns `BuilderError::DockerNotReachable` |
| Full Dockerfile generation (cosmos-gaiad) | End-to-end: recipe -> Dockerfile string matches expected output |
| Full Dockerfile generation (kyve-kyved) | End-to-end with profile variables |
| Full Dockerfile generation (Rust chain TBD) | End-to-end: Rust recipe -> Dockerfile string matches expected output |

Docker CLI mocking: delegate to @qa for mock strategy (likely mock the
`commands` module since all Docker CLI calls go through it).

### What NOT to test in this module

- Recipe TOML parsing (owned by recipe module).
- Flavor resolution logic (owned by recipe module).
- Registry push (owned by push module).
- Actual Docker daemon behavior (integration/E2E tests).

---

## Resolved questions

| ID | Question | Decision | Status |
| :- | :------- | :------- | :----- |
| B1 | Should the runner base image mapping (`running_env` -> Docker image) be hardcoded in the Go builder or configurable in the recipe TOML? | Configurable in the recipe TOML via a `[flavours.images]` section. NOT hardcoded. Each recipe defines its own `running_env` -> Docker image mapping, allowing new runner environments without code changes. | RESOLVED |
| B2 | Should shell-type variables be executed on the host or emitted as `RUN` instructions inside the Dockerfile builder stage? | Emitted as Dockerfile `RUN` commands (not host ENV). Their output is written to files in the builder stage and forwarded into the runner stage by `COPY --from=builder` (file-copy approach). This avoids multi-stage ENV propagation issues. | RESOLVED |
| B3 | Should `ImageBuilder.build()` support `--push` directly or always `--load` locally? | `ImageBuilder.build()` ALWAYS uses `--load` locally. Push is a separate step handled by the push module. The builder never uses `--push` directly. This decouples build from push and simplifies error handling. | RESOLVED |
| B4 | How should the builder handle QEMU setup for cross-platform builds? | QEMU binfmt_misc setup is handled by the checker module (the same module that creates BuildKit containers). The builder module assumes QEMU is already set up and does not verify or register QEMU itself. | RESOLVED |
| B5 | Does the Go builder consume the recipe `[scrapper]` section, or does the scrapper module handle it? | The recipe `[scrapper]` section is consumed by the scrapper module. The builder receives already-prepared `CloneInstructions` from the scrapper module and embeds them into the Dockerfile. The builder does NOT parse or consume `[scrapper]` directly. | RESOLVED |
