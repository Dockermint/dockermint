# Feature: Push (Registry Client)

## Context

The `push` module is the registry integration layer of the Dockermint pipeline.
It sits after build execution (step 8) and before state persistence (step 9) in
the core build pipeline (architecture-overview.md Section 1.2, step 9 "Push /
Save").

Its job is to authenticate with a container registry, push per-platform images,
and create/push multi-arch manifest lists. It also supports a "local-only" mode
where images are built and loaded into the local Docker daemon without pushing
to any registry.

Roadmap entry: Phase 0 -- Foundation, spec "Registry auth and image pushing
(OCI) -- push" (docs/ROADMAP.md)

---

## Requirements

1. [confirmed] Provide a `RegistryClient` trait that abstracts registry
   operations, with a default OCI implementation behind the `registry-oci`
   feature gate.
2. [confirmed] Authenticate with container registries using credentials from
   `.env` (`REGISTRY_USER`, `REGISTRY_PASSWORD`). No credentials in config.toml.
3. [confirmed] Push per-platform images produced by the builder module.
4. [confirmed] Create and push multi-arch manifest lists combining
   `linux/amd64` and `linux/arm64` images.
5. [confirmed] Support a tag naming strategy: version tag, "latest" tag
   (applied only when this version is the highest semver -- CEO decision P1),
   and custom tags from the recipe `[image] tag` template.
6. [confirmed] Support local-only mode: images are loaded into the local
   Docker daemon (`--load`) and no push occurs.
7. [confirmed] The push vs local-only decision is driven by configuration
   (registry URL presence in config.toml) and/or CLI flags.
8. [confirmed] Registry URL is configured in `config.toml` `[registry]`
   section. Credentials are exclusively in `.env`.
9. [confirmed] The module produces a `PushResult` that downstream modules
   (saver, notifier) consume to record/report build outcomes.
10. [confirmed] Push via direct OCI Distribution API (HTTP), NOT Docker CLI
    (CEO decision P2).
11. [confirmed] If a tag already exists in the registry, skip the build
    entirely (do not build). Build only if `--force` is specified. This is
    a mandatory pre-build check (CEO decision P3).
12. [confirmed] Single registry support only. No multi-registry push
    (CEO decision P4).
13. [confirmed] Platform suffix format: `-linux-amd64` and `-linux-arm64`.
    Both platforms are built by default (CEO decision P5).

---

## Architecture

### Module placement

```
src/push/
    mod.rs              -- RegistryClient trait + feature-gated re-exports
    error.rs            -- RegistryError enum (thiserror)
    types.rs            -- PushResult, PushMode, ImageReference, TagStrategy structs
    oci/
        mod.rs          -- OciRegistryClient implementation of RegistryClient
        auth.rs         -- OCI token exchange (WWW-Authenticate challenge flow)
        blobs.rs        -- Blob existence check, chunked upload, finalization
        manifest.rs     -- Platform manifest and OCI Image Index creation + push
        tags.rs         -- Tag listing, semver comparison for "latest" logic
```

### Trait design

#### RegistryClient trait

The primary abstraction for registry operations. Swappable via feature gate.

Design constraints:
- Async (network I/O bound).
- `Send + Sync` for daemon mode shared state.
- Does not depend on the builder module -- receives image references and
  digests, not Dockerfiles or build contexts.

```rust
/// A client for authenticating with and pushing images to a container
/// registry.
///
/// Implementations are selected at compile time via feature gates.
/// The default implementation is `OciRegistryClient` (feature
/// `registry-oci`).
///
/// # Trait bounds
///
/// `Send + Sync` is required because the daemon shares the client
/// across async tasks.
pub trait RegistryClient: Send + Sync {
    /// Authenticate with the registry.
    ///
    /// Must be called before `push_image` or `push_manifest`. The
    /// authentication state is stored internally in the client.
    ///
    /// # Arguments
    ///
    /// * `registry_url` - Registry URL (e.g., "ghcr.io")
    /// * `credentials` - Username and password wrapped in secrecy types.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError::Auth` if credentials are rejected or
    /// the registry is unreachable.
    async fn authenticate(
        &self,
        registry_url: &str,
        credentials: &RegistryCredentials,
    ) -> Result<(), RegistryError>;

    /// Push a single-platform image to the registry.
    ///
    /// The image must already exist in the local Docker daemon (loaded
    /// via `docker buildx build --load` or equivalent).
    ///
    /// # Arguments
    ///
    /// * `image_ref` - Full image reference with tag (e.g.,
    ///   "ghcr.io/dockermint/cosmos-gaiad:v19.0.0-amd64")
    ///
    /// # Returns
    ///
    /// `PushResult` containing the pushed image digest and metadata.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError` for auth failures, network errors, or
    /// registry rejection.
    async fn push_image(
        &self,
        image_ref: &ImageReference,
    ) -> Result<PushResult, RegistryError>;

    /// Push a multi-arch manifest list to the registry.
    ///
    /// Creates a manifest list referencing multiple platform-specific
    /// images and pushes it under the given tag.
    ///
    /// # Arguments
    ///
    /// * `manifest_tag` - Tag for the manifest list (e.g.,
    ///   "ghcr.io/dockermint/cosmos-gaiad:v19.0.0-alpine3.23")
    /// * `platform_images` - Platform-specific image references to
    ///   include in the manifest.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError::Manifest` if manifest creation or push
    /// fails.
    async fn push_manifest(
        &self,
        manifest_tag: &ImageReference,
        platform_images: &[PushResult],
    ) -> Result<PushResult, RegistryError>;

    /// Check whether a tag already exists in the registry.
    ///
    /// Used to skip builds for versions that have already been pushed
    /// (especially useful in daemon mode).
    ///
    /// # Arguments
    ///
    /// * `image_ref` - Full image reference to check.
    ///
    /// # Returns
    ///
    /// `true` if the tag exists, `false` otherwise.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError` for auth or network failures.
    async fn tag_exists(
        &self,
        image_ref: &ImageReference,
    ) -> Result<bool, RegistryError>;
}
```

### Type design

#### RegistryCredentials

```rust
/// Credentials for registry authentication.
///
/// Loaded from .env: REGISTRY_USER and REGISTRY_PASSWORD.
/// Both values are wrapped in secrecy types to prevent accidental logging.
#[derive(Debug, Clone)]
pub struct RegistryCredentials {
    /// Registry username.
    pub username: secrecy::SecretString,

    /// Registry password or token.
    pub password: secrecy::SecretString,
}
```

Note: `Debug` is derived but `secrecy::SecretString` redacts its contents in
`Debug` output, so no credential leakage occurs.

#### ImageReference

```rust
/// A fully qualified container image reference.
///
/// Parsed from the combination of registry URL (config.toml) and image
/// tag (recipe [image] section after template resolution).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageReference {
    /// Registry hostname (e.g., "ghcr.io").
    pub registry: String,

    /// Repository path within the registry (e.g., "dockermint/cosmos-gaiad-goleveldb").
    pub repository: String,

    /// Image tag (e.g., "v19.0.0-alpine3.23").
    pub tag: String,
}

impl ImageReference {
    /// Full reference string: "registry/repository:tag"
    pub fn full_ref(&self) -> String;

    /// Reference without tag: "registry/repository"
    pub fn without_tag(&self) -> String;
}
```

#### PushResult

```rust
/// Result of a successful image or manifest push.
///
/// Consumed by the saver module (to persist build records) and the
/// notifier module (to report build outcomes).
#[derive(Debug, Clone, PartialEq)]
pub struct PushResult {
    /// The full image reference that was pushed.
    pub image_ref: ImageReference,

    /// The digest of the pushed image or manifest (sha256:...).
    pub digest: String,

    /// The platform this image targets (None for manifest lists).
    pub platform: Option<Platform>,

    /// Size of the pushed image in bytes (if reported by registry).
    pub size_bytes: Option<u64>,
}
```

#### PushMode

```rust
/// Determines whether images are pushed to a registry or kept local.
///
/// Resolved from config.toml [registry] url presence and CLI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushMode {
    /// Push images to the configured registry.
    Registry,

    /// Keep images in the local Docker daemon only. No registry push.
    LocalOnly,
}
```

#### TagStrategy

```rust
/// Describes which tags to apply to a built image.
///
/// The primary tag comes from the recipe [image] tag template after
/// variable resolution.
///
/// The "latest" tag is applied by semver comparison: "latest" is set
/// only if the current build version is the highest semver among all
/// tags already in the registry for this image name (CEO decision P1).
#[derive(Debug, Clone)]
pub struct TagStrategy {
    /// Primary version tag from recipe (e.g., "cosmos-gaiad-goleveldb:v19.0.0-alpine3.23").
    pub version_tag: String,

    /// Whether to also tag as "latest" for this recipe.
    /// Determined by semver comparison: true only if this version is
    /// the highest semver among existing tags in the registry.
    /// Pre-release versions (e.g., v1.0.0-rc1) never get "latest".
    pub tag_latest: bool,

    /// Additional custom tags (empty by default).
    pub additional_tags: Vec<String>,
}
```

#### RegistryConfig

```rust
/// Configuration for the push module, deserialized from config.toml
/// [registry] section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    /// Registry URL (e.g., "ghcr.io/dockermint").
    /// If empty or absent, push is skipped (local-only mode).
    pub url: String,
}
```

#### Derive strategy

- `RegistryCredentials`: `Debug`, `Clone`. Not `PartialEq` (secrets should
  not be compared).
- `ImageReference`: `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`.
- `PushResult`: `Debug`, `Clone`, `PartialEq`.
- `PushMode`: `Debug`, `Clone`, `PartialEq`, `Eq`.
- `TagStrategy`: `Debug`, `Clone`.
- `RegistryConfig`: `Debug`, `Clone`, `Deserialize`.

#### Visibility

- `RegistryClient` trait: `pub` (used by pipeline orchestrator and binaries).
- All types in `types.rs`: `pub` (shared data model consumed by saver/notifier).
- `OciRegistryClient`: `pub` (re-exported from `mod.rs` behind feature gate).
- Internal OCI auth/manifest helpers: `pub(crate)` or `pub(super)`.

### Feature gate

| Feature flag | Module | Default |
| :----------- | :----- | :------ |
| `registry-oci` | push | Yes |

From architecture-overview.md Section 3.1. Compile-time enforcement:

```
#[cfg(not(feature = "registry-oci"))]
compile_error!("At least one registry backend must be enabled (e.g., registry-oci)");
```

### Configuration

Existing config.toml keys in `[registry]` section (already defined in
architecture-overview.md Section 4.2):

```toml
[registry]
url = "ghcr.io/dockermint"     # Registry URL. Empty = local-only mode.
```

Secrets in `.env`:

| Secret | .env variable | Notes |
| :----- | :------------ | :---- |
| Registry username | `REGISTRY_USER` | Loaded via `dotenvy`, wrapped in `secrecy::SecretString` |
| Registry password | `REGISTRY_PASSWORD` | Loaded via `dotenvy`, wrapped in `secrecy::SecretString` |

CLI flag overrides:

| Flag | Overrides | Notes |
| :--- | :-------- | :---- |
| `--registry` | `registry.url` | Registry URL |
| `--local-only` | Forces `PushMode::LocalOnly` | Skip push regardless of config |
| `--force` | Overrides tag_exists skip | Build and push even if tag already exists in registry |

### Push mode resolution

```
1. If --local-only CLI flag is set:
     -> PushMode::LocalOnly

2. Else if registry.url in config.toml is empty or absent:
     -> PushMode::LocalOnly

3. Else if REGISTRY_USER or REGISTRY_PASSWORD not set in .env:
     -> Error: registry URL configured but credentials missing

4. Else:
     -> PushMode::Registry
```

### Error types

```rust
/// Errors originating from registry push operations.
///
/// Owned by the `push` module. Defined in `src/push/error.rs`.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Authentication with the registry failed.
    #[error("registry authentication failed for '{registry}': {message}")]
    Auth {
        registry: String,
        message: String,
    },

    /// Registry credentials not found in .env.
    #[error("registry credentials missing: {missing_var} not set in .env")]
    MissingCredentials {
        missing_var: String,
    },

    /// Network error during push.
    #[error("registry network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Registry rejected the push (quota, permission, invalid image).
    #[error("registry rejected push for '{image_ref}': {status} {message}")]
    Rejected {
        image_ref: String,
        status: u16,
        message: String,
    },

    /// Multi-arch manifest creation or push failed.
    #[error("manifest push failed for '{tag}': {message}")]
    Manifest {
        tag: String,
        message: String,
    },

    /// Image reference parsing or construction failed.
    #[error("invalid image reference: {reference}")]
    InvalidReference {
        reference: String,
    },

    /// Tag already exists in the registry (when checking before push).
    /// Not necessarily an error -- informational for skip logic.
    #[error("tag already exists: {image_ref}")]
    TagExists {
        image_ref: String,
    },

    /// OCI blob or manifest upload failed (chunked upload error, digest mismatch).
    #[error("OCI upload failed for '{image_ref}': {message}")]
    UploadFailed {
        image_ref: String,
        message: String,
    },
}
```

Error mapping to Unrecoverable Error Strategy
(architecture-overview.md Section 6.3):

| RegistryError variant | CLI | Daemon | RPC |
| :-------------------- | :-- | :----- | :-- |
| Auth | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| MissingCredentials | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| Network | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| Rejected | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| Manifest | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| InvalidReference | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| TagExists | Log + skip (info) | Log + skip (not a failure) | Log + return info |
| UploadFailed | Log + exit | Log + notify + persist failure + continue | Log + return idle |

### Dependencies

External crates needed:

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `reqwest` | HTTP client for OCI Distribution API | Already used by scrapper, notifier |
| `serde` + `serde_json` | Deserialize OCI API responses and manifest JSON | Already a project dependency |
| `secrecy` | Wrap registry credentials | Already in architecture-overview.md conventions |
| `dotenvy` | Load `.env` for credentials | Already in architecture-overview.md conventions |
| `tracing` | Structured logging for push operations | Already a project convention |

No new crate dependencies beyond what the project already requires.

---

## OCI Distribution Spec integration

### Push strategy

The default `OciRegistryClient` implements the OCI Distribution API directly
via HTTP (CEO decision P2). It does NOT use the Docker CLI for push operations.
This eliminates the Docker CLI dependency for push, provides full control over
upload behavior, and enables the pre-build `tag_exists` check without external
tools.

Only a single registry is supported (CEO decision P4). The registry URL is
configured in `config.toml [registry] url`.

### Pre-build tag existence check (mandatory)

Before any build is attempted, the push module performs a `tag_exists` check
against the registry (CEO decision P3). This is a mandatory pre-build check:

```
1. For the version about to be built, construct the manifest tag.
2. HEAD request to the registry to check if the tag already exists.
3. If tag exists AND --force is NOT specified:
     -> Skip the build entirely. Log at info level. Not an error.
4. If tag exists AND --force IS specified:
     -> Proceed with build and push (overwrite).
5. If tag does not exist:
     -> Proceed with build and push.
```

This check happens BEFORE build execution, saving time and compute resources
by not building images that already exist in the registry. In daemon mode, this
is especially valuable as it avoids rebuilding every version on each poll.

### Push flow (via OCI Distribution API)

```
[1. Authenticate]
 POST /v2/ or GET /v2/ to trigger WWW-Authenticate challenge.
 Exchange credentials for a Bearer token via the token endpoint.
 Credentials: REGISTRY_USER + REGISTRY_PASSWORD from .env.
         |
         v
[2. Export Image from Docker Daemon]
 docker save <local-image:tag> -> OCI image tarball
 Extract layers, config, and manifest from the tarball.
         |
         v
[3. Push Blobs (Layers + Config)]
 For each layer and the config blob:
   a. HEAD /v2/<repo>/blobs/<digest> -- check if blob exists (skip if so)
   b. POST /v2/<repo>/blobs/uploads/ -- initiate upload
   c. PATCH /v2/<repo>/blobs/uploads/<uuid> -- upload chunk(s)
   d. PUT /v2/<repo>/blobs/uploads/<uuid>?digest=<digest> -- finalize
         |
         v
[4. Push Platform Manifests]
 For each platform image (linux/amd64, linux/arm64):
   PUT /v2/<repo>/manifests/<tag-platform-suffix>
   Content-Type: application/vnd.oci.image.manifest.v1+json
   Capture digest from response.
         |
         v
[5. Push Manifest Index (Multi-arch)]
   Construct OCI Image Index referencing platform manifests.
   PUT /v2/<repo>/manifests/<tag>
   Content-Type: application/vnd.oci.image.index.v1+json
         |
         v
[6. Tag Latest (conditional on semver)]
 If this version is the highest semver (CEO decision P1):
   PUT /v2/<repo>/manifests/latest
   (Same manifest index content, different tag)
```

### Authentication flow

```
1. Load REGISTRY_USER and REGISTRY_PASSWORD from .env via dotenvy.
2. Wrap both in secrecy::SecretString.
3. GET /v2/ -- expect 401 with WWW-Authenticate header.
4. Parse the WWW-Authenticate header for realm, service, and scope.
5. POST/GET to the token realm with Basic auth (username:password).
6. Receive a Bearer token. Store it in the client for subsequent requests.
7. Include "Authorization: Bearer <token>" in all subsequent API calls.
8. On token expiry: re-authenticate transparently.
9. On auth failure: return RegistryError::Auth.
```

Credentials are never passed as CLI arguments or written to files. The Bearer
token is held in memory only.

### Multi-arch manifest handling

Multi-arch images are the standard approach for supporting both `linux/amd64`
and `linux/arm64` from a single image tag. The flow is:

```
                    +--------------------+
                    | manifest list      |
                    | cosmos-gaiad:v19   |
                    +----+----------+----+
                         |          |
                         v          v
                  +------+---+ +---+------+
                  | amd64    | | arm64    |
                  | image    | | image    |
                  | digest:  | | digest:  |
                  | sha256:a | | sha256:b |
                  +----------+ +----------+
```

When a user pulls `cosmos-gaiad:v19`, the Docker daemon automatically selects
the image matching the host architecture.

### Tag naming strategy

The primary image tag comes from the recipe `[image] tag` field after template
variable resolution:

```toml
# Recipe: cosmos-gaiad.toml
[image]
tag = "cosmos-gaiad-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"

# Resolved example:
# tag = "cosmos-gaiad-goleveldb:v19.0.0-alpine3.23"
```

The full image reference is constructed by prepending the registry URL:

```
<registry.url>/<resolved_tag>
ghcr.io/dockermint/cosmos-gaiad-goleveldb:v19.0.0-alpine3.23
```

Per-platform tags append a platform suffix during push (CEO decision P5:
format is `-linux-amd64` and `-linux-arm64`), then the manifest index uses the
unsuffixed tag. Both platforms are built by default.

| Image | Tag | Purpose |
| :---- | :-- | :------ |
| amd64 image | `cosmos-gaiad-goleveldb:v19.0.0-alpine3.23-linux-amd64` | Platform-specific |
| arm64 image | `cosmos-gaiad-goleveldb:v19.0.0-alpine3.23-linux-arm64` | Platform-specific |
| manifest | `cosmos-gaiad-goleveldb:v19.0.0-alpine3.23` | Multi-arch entry point (OCI Image Index) |
| latest | `cosmos-gaiad-goleveldb:latest` | Conditional: only if this version is the highest semver |

### "latest" tag logic (semver-based)

The "latest" tag is applied only when the current build version is the highest
semver among all existing tags in the registry for this image name
(CEO decision P1):

```
1. Fetch existing tags for the image from the registry:
   GET /v2/<repo>/tags/list
2. Parse all tags as semver (skip non-semver tags).
3. Find the highest semver among existing tags.
4. If the current version > highest existing version:
     -> tag_latest = true
     -> Push the manifest index again under the "latest" tag
5. If the current version <= highest existing version:
     -> tag_latest = false
     -> Do not update "latest"
6. Pre-release versions (e.g., v1.0.0-rc1) are never tagged as "latest".
```

### Local-only mode

When `PushMode::LocalOnly`:

1. The builder always uses `--load` (images are in the local Docker daemon).
2. No OCI API calls occur (no authentication, no blob upload, no manifest push).
3. The pipeline proceeds directly to saver/notifier.
4. `PushResult` is still produced with local image references (no digest from
   registry, only local image ID).

This mode is useful for:
- Local development and testing.
- CI environments where images are consumed locally.
- Users who do not have a registry.

---

## Interface contract

```rust
// -- src/push/mod.rs --

pub trait RegistryClient: Send + Sync {
    async fn authenticate(
        &self,
        registry_url: &str,
        credentials: &RegistryCredentials,
    ) -> Result<(), RegistryError>;

    async fn push_image(
        &self,
        image_ref: &ImageReference,
    ) -> Result<PushResult, RegistryError>;

    async fn push_manifest(
        &self,
        manifest_tag: &ImageReference,
        platform_images: &[PushResult],
    ) -> Result<PushResult, RegistryError>;

    async fn tag_exists(
        &self,
        image_ref: &ImageReference,
    ) -> Result<bool, RegistryError>;
}

// -- src/push/oci/mod.rs (behind feature "registry-oci") --

pub struct OciRegistryClient { /* ... */ }

impl OciRegistryClient {
    /// Create a new OCI registry client.
    ///
    /// # Arguments
    ///
    /// * `config` - Registry configuration from config.toml.
    pub fn new(config: &RegistryConfig) -> Self;
}

impl RegistryClient for OciRegistryClient {
    async fn authenticate(
        &self,
        registry_url: &str,
        credentials: &RegistryCredentials,
    ) -> Result<(), RegistryError>;

    async fn push_image(
        &self,
        image_ref: &ImageReference,
    ) -> Result<PushResult, RegistryError>;

    async fn push_manifest(
        &self,
        manifest_tag: &ImageReference,
        platform_images: &[PushResult],
    ) -> Result<PushResult, RegistryError>;

    async fn tag_exists(
        &self,
        image_ref: &ImageReference,
    ) -> Result<bool, RegistryError>;
}
```

---

## Module interaction diagram

```
+----------+    +----------+    +----------+
| builder  |--->| push     |--->| saver    |
| BuildOut |    |          |    | (persist |
| (image   |    | Registry |    |  result) |
|  refs,   |    | Client   |    +----------+
|  digests)|    | trait     |
+----------+    +----+-----+    +----------+
                     |     +--->| notifier |
                     |          | (report  |
               OCI Distrib.    |  status) |
               API (HTTP)      +----------+
               via reqwest
                     |
                     v
              +------+------+
              | Container   |
              | Registry    |
              | (single     |
              |  registry)  |
              +-------------+

Push flow:

+----------+                +----------+
| .env     |                | config   |
| REGISTRY |                | .toml    |
| _USER    |                | [registry|
| _PASSWORD|                |  url]    |
+----+-----+                +----+-----+
     |                           |
     v                           v
+----+---------------------------+-----+
|           push module                |
|                                      |
|  0. Pre-build: tag_exists check      |
|     (skip if exists, unless --force) |
|                                      |
|  1. Resolve PushMode                 |
|     (LocalOnly or Registry)          |
|                                      |
|  2. If Registry:                     |
|     a. Authenticate (OCI token flow) |
|     b. Export images from daemon     |
|     c. Push blobs (layers + config)  |
|     d. Push platform manifests       |
|     e. Push manifest index           |
|     f. Tag "latest" if highest semver|
|                                      |
|  3. Return PushResult per image      |
+--------------------------------------+
```

---

## Testing strategy

### Unit tests

| Test | What it validates |
| :--- | :---------------- |
| `ImageReference::full_ref()` | Correct "registry/repo:tag" format |
| `ImageReference::without_tag()` | Correct "registry/repo" format |
| `PushMode` resolution: local-only flag | `--local-only` -> `PushMode::LocalOnly` |
| `PushMode` resolution: empty registry URL | No URL -> `PushMode::LocalOnly` |
| `PushMode` resolution: URL present + creds present | -> `PushMode::Registry` |
| `PushMode` resolution: URL present + creds missing | -> `RegistryError::MissingCredentials` |
| `RegistryConfig` deserialization | Correct parsing of config.toml `[registry]` section |
| Tag strategy: version tag construction | Recipe tag template resolved correctly |
| Tag strategy: platform suffix | Correct `-linux-amd64`/`-linux-arm64` suffix appended |
| Tag strategy: semver latest logic | "latest" only when version > all existing tags |
| Tag strategy: prerelease never latest | Pre-release versions never tagged as "latest" |
| Tag strategy: no existing tags | First version always gets "latest" |
| `RegistryCredentials` Debug redaction | `secrecy::SecretString` does not leak in Debug output |

### Integration tests (mocked OCI HTTP API)

| Test | What it validates |
| :--- | :---------------- |
| `OciRegistryClient::authenticate` happy path | Token exchange via WWW-Authenticate challenge flow |
| `OciRegistryClient::authenticate` failure | Returns `RegistryError::Auth` on 401 |
| `OciRegistryClient::push_image` happy path | Blob upload + manifest PUT, digest returned |
| `OciRegistryClient::push_image` blob exists | Blob skipped (HEAD returns 200), only manifest pushed |
| `OciRegistryClient::push_manifest` happy path | OCI Image Index PUT with correct platform references |
| `OciRegistryClient::tag_exists` true | HEAD /v2/.../manifests/<tag> returns 200 |
| `OciRegistryClient::tag_exists` false | HEAD /v2/.../manifests/<tag> returns 404 |
| Pre-build skip: tag exists, no --force | Build is skipped entirely (info log, no push) |
| Pre-build force: tag exists, --force set | Build proceeds, existing tag overwritten |
| Push rejection (403) | Returns `RegistryError::Rejected` with status code |
| Network error during push | Returns `RegistryError::Network` |
| Upload failure (digest mismatch) | Returns `RegistryError::UploadFailed` |
| Local-only mode | No HTTP calls to registry; images only loaded locally |
| Full push flow (multi-arch) | End-to-end: auth -> push blobs -> push amd64 manifest -> push arm64 manifest -> push index -> latest |
| Semver latest: new highest version | "latest" tag pushed |
| Semver latest: older version | "latest" tag NOT pushed |
| Semver latest: prerelease version | "latest" tag NOT pushed |

HTTP mocking: use a mock server (e.g., `wiremock` or `mockito`) to simulate
OCI Distribution API responses. Delegate crate choice to @lead-dev.

### What NOT to test in this module

- Image building (owned by builder module).
- Build state persistence (owned by saver module).
- Notification sending (owned by notifier module).
- Actual registry behavior (E2E tests against a real or test registry).

---

## Resolved questions

| ID | Question | Decision | Status |
| :- | :------- | :------- | :----- |
| P1 | When should "latest" tag be applied? | Applied by semver check: "latest" is set only if the current build version is the highest semver among all existing tags in the registry for this image name. Pre-release versions never get "latest". | RESOLVED |
| P2 | Should the push module use Docker CLI or implement the OCI Distribution API directly via HTTP? | Push via direct OCI Distribution API (HTTP) using `reqwest`. No Docker CLI dependency for push operations. This provides full control over upload behavior and enables direct tag_exists checks. | RESOLVED |
| P3 | Should `tag_exists` check be mandatory before every push, or opt-in? | Mandatory pre-build check. If tag already exists in registry, the build is skipped entirely (not just the push -- the build itself is not executed). Build only if `--force` is specified. This saves compute by avoiding redundant builds. | RESOLVED |
| P4 | Should the push module support multiple registries or a single registry? | Single registry support only. No multi-registry push. One registry URL in `config.toml [registry] url`. | RESOLVED |
| P5 | How should the platform suffix be formatted? | `-linux-amd64` and `-linux-arm64`. Both platforms are built by default. | RESOLVED |
