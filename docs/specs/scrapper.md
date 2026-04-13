# Feature: Scrapper (VCS Client)

## Context

The `scrapper` module is the VCS integration layer of the Dockermint pipeline.
It sits between recipe/flavor resolution (step 4) and the builder (step 6) in
the core build pipeline (see architecture-overview.md, Section 1.2, steps 5).

Its job is twofold:

1. **VCS API scraping** -- query a version control hosting service for available
   tags and releases, apply include/exclude glob filters from the recipe, and
   produce a list of versions that should be built.
2. **Source acquisition** -- clone the repository source code at a target version
   for the builder to compile.

The scrapper module owns the recipe `[scrapper]` TOML section, which describes
both the API scraping configuration and source cloning behavior (builder image,
install commands, env vars, clone method, directory).

In CLI mode, scraping is a one-shot fetch; in daemon mode it is a polling loop
that detects new releases since the last poll. Source cloning is always a
single operation per build (clone at the target tag/version).

Roadmap entry: Phase 0 -- Foundation, spec "VCS client (GitHub) and
tag/release scraping -- scrapper" (docs/ROADMAP.md)

---

## Requirements

1. [confirmed] Provide a `VcsClient` trait that abstracts VCS operations, with
   a default GitHub implementation behind the `vcs-github` feature gate.
2. [confirmed] Fetch tags and releases from the GitHub REST API for a given
   repository URL.
3. [confirmed] Apply include/exclude glob patterns (from recipe `[header]`
   `include_patterns` / `exclude_patterns`) to filter tags.
4. [confirmed] Authenticate using a GitHub Personal Access Token loaded from
   `.env` (`GH_PAT`). Support unauthenticated mode with degraded rate limits.
5. [confirmed] Handle GitHub API rate limiting: detect 429 responses and
   `X-RateLimit-Remaining` headers, back off and retry.
6. [confirmed] Handle pagination: follow GitHub `Link` header for multi-page
   results.
7. [confirmed] In daemon mode, support polling for new releases by comparing
   fetched tags against previously-built versions (queried from `BuildStore`
   via the pipeline orchestrator -- not directly by this module).
8. [confirmed] In CLI mode, perform a single fetch and return all matching
   versions (or a specific version if the user targets one).
9. [confirmed] Secrets are exclusively in `.env` (per CEO decision). No PAT
   or credentials in config.toml.
10. [confirmed] Produce a well-defined `VersionInfo` data model that downstream
    modules (builder, push, saver) consume.
11. [confirmed] Own the recipe `[scrapper]` TOML section, which describes both
    API scraping config and source acquisition (clone method, builder image,
    install commands, env vars, directory).
12. [confirmed] Clone the repository source at the target version for the builder
    module. The builder receives already-cloned source from the scrapper.

---

## Architecture

### Module placement

```
src/scrapper/
    mod.rs              -- VcsClient trait definition + feature-gated re-exports
    error.rs            -- VcsError enum (thiserror)
    types.rs            -- VersionInfo, TagFilter, FetchOptions, CloneOptions structs
    clone.rs            -- Source cloning logic (Dockerfile RUN instructions)
    github/
        mod.rs          -- GithubClient implementation of VcsClient
        api.rs          -- Low-level GitHub REST API request/response types
        pagination.rs   -- Link header parser, page iterator
        rate_limit.rs   -- Rate limit tracking and backoff logic
```

### Trait design

#### VcsClient trait

The primary abstraction. Any VCS backend (GitHub, GitLab, Gitea, etc.) must
implement this trait. Selected at compile time via feature gate.

Design constraints:
- Async (network I/O bound).
- `Send + Sync` -- required for daemon mode shared state.
- Does not depend on any downstream module (saver, builder). The pipeline
  orchestrator is responsible for comparing fetched versions against stored
  state.
- `fetch_versions` returns tags AND releases merged into a single unified
  return type (`Vec<VersionInfo>`). There is no separate method for tags vs
  releases (CEO decision S1).
- The trait includes source cloning methods because the scrapper module owns
  both VCS API fetching and source acquisition (CEO decision S2/S3).

```rust
/// A client for fetching version information from a version control
/// hosting service and cloning source code.
///
/// Implementations are selected at compile time via feature gates.
/// The default implementation is `GithubClient` (feature `vcs-github`).
///
/// # Trait bounds
///
/// `Send + Sync` is required because the daemon shares the client
/// across async tasks.
///
/// # Scope
///
/// This trait covers two responsibilities:
/// 1. VCS API scraping: fetching tags/releases.
/// 2. Source acquisition: producing clone instructions for Dockerfile
///    generation. The scrapper does not execute `git clone` on the host;
///    it produces the Dockerfile instructions (RUN git clone ...) that
///    the builder emits into the builder stage.
pub trait VcsClient: Send + Sync {
    /// Fetch all tags and releases (merged) for a repository, applying
    /// glob filters.
    ///
    /// Tags and releases are unified into a single `Vec<VersionInfo>`.
    /// Bare tags (without a corresponding release) and releases are both
    /// included. Duplicates (a tag that is also a release) are
    /// deduplicated by tag name, with the release metadata taking
    /// precedence.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Full repository URL (e.g., "https://github.com/cosmos/gaia")
    /// * `options` - Fetch configuration: filters, auth, pagination limits
    ///
    /// # Returns
    ///
    /// A vector of `VersionInfo` structs representing matching
    /// tags/releases, ordered newest first.
    ///
    /// # Errors
    ///
    /// Returns `VcsError` for network failures, authentication errors,
    /// rate limit exhaustion, or response parse failures.
    async fn fetch_versions(
        &self,
        repo_url: &str,
        options: &FetchOptions,
    ) -> Result<Vec<VersionInfo>, VcsError>;

    /// Fetch a single specific tag/release by exact name.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Full repository URL
    /// * `tag_name` - Exact tag name (e.g., "v19.0.0")
    ///
    /// # Returns
    ///
    /// The matching `VersionInfo`, or `VcsError::TagNotFound` if it does
    /// not exist.
    async fn fetch_version(
        &self,
        repo_url: &str,
        tag_name: &str,
    ) -> Result<VersionInfo, VcsError>;

    /// Produce Dockerfile clone instructions for the builder stage.
    ///
    /// Returns the Dockerfile `RUN` instructions that clone the repository
    /// at the target version inside the builder container. The scrapper
    /// does NOT execute `git clone` on the host -- it produces instructions
    /// that the builder emits into the Dockerfile.
    ///
    /// The clone method, environment variables, and directory are read from
    /// the recipe `[scrapper]` section.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Full repository URL
    /// * `clone_options` - Clone configuration from recipe `[scrapper]` section
    /// * `version` - Target version/tag to clone
    ///
    /// # Returns
    ///
    /// A `CloneInstructions` struct containing Dockerfile lines for the
    /// builder stage.
    ///
    /// # Errors
    ///
    /// Returns `VcsError::InvalidUrl` if the URL is malformed.
    /// Returns `VcsError::InvalidCloneMethod` if the method is unsupported.
    fn clone_instructions(
        &self,
        repo_url: &str,
        clone_options: &CloneOptions,
        version: &str,
    ) -> Result<CloneInstructions, VcsError>;
}
```

#### Future-proofing

Alternative VCS backends (GitLab, Gitea) can be added as:
1. New submodule under `src/scrapper/<backend>/`.
2. New feature gate (e.g., `vcs-gitlab`).
3. Implement `VcsClient` for the new backend.
4. No modification to existing code.

### Type design

#### VersionInfo

The data model passed downstream from scrapper to builder/push/saver.

```rust
/// Information about a single version (tag or release) fetched from VCS.
///
/// This is the primary output of the scrapper module and the input to
/// the builder module.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionInfo {
    /// The tag name as it appears in VCS (e.g., "v19.0.0").
    pub tag_name: String,

    /// Semver-parsed version, if the tag follows semver conventions.
    /// `None` if the tag is not a valid semver string.
    pub semver: Option<semver::Version>,

    /// The full commit SHA that the tag points to.
    pub commit_sha: String,

    /// Whether this is a GitHub "release" (as opposed to a bare tag).
    pub is_release: bool,

    /// Whether the release is marked as a pre-release on GitHub.
    pub is_prerelease: bool,

    /// Publication timestamp (tag creation or release publish date).
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,

    /// The tarball download URL (for source archive).
    pub tarball_url: Option<String>,
}
```

#### TagFilter

```rust
/// Glob-based include/exclude filter for tags.
///
/// Both fields come from the recipe `[header]` section:
/// `include_patterns` and `exclude_patterns`.
///
/// Filtering logic:
/// 1. If `include` is non-empty, only tags matching at least one include
///    pattern are kept.
/// 2. If `exclude` is non-empty, tags matching any exclude pattern are
///    removed.
/// 3. Exclude takes precedence over include.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagFilter {
    /// Glob patterns for tags to include. Empty means "include all".
    pub include: Vec<String>,

    /// Glob patterns for tags to exclude. Empty means "exclude none".
    pub exclude: Vec<String>,
}
```

#### FetchOptions

```rust
/// Configuration for a VCS fetch operation.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Tag include/exclude glob filters.
    pub filter: TagFilter,

    /// Maximum number of tags to fetch. `None` means no limit (fetch all).
    /// Useful for CLI mode when only the latest N are needed.
    pub max_results: Option<usize>,
}
```

#### CloneOptions

```rust
/// Configuration for source cloning, deserialized from the recipe
/// `[scrapper]` TOML section.
///
/// The scrapper module owns this section. It describes how the source
/// is obtained inside the Dockerfile builder stage.
#[derive(Debug, Clone)]
pub struct CloneOptions {
    /// Clone method (e.g., "try-authenticated-clone", "clone").
    /// Determines whether GH_PAT is used for authentication.
    pub method: String,

    /// Directory inside the builder container where the repo is cloned.
    /// (e.g., "/workspace")
    pub directory: String,

    /// Environment variables to set before cloning (from recipe
    /// `[scrapper] env`). These are emitted as Dockerfile `ENV`
    /// instructions.
    pub env: Vec<(String, String)>,
}
```

#### CloneInstructions

```rust
/// Dockerfile instructions produced by the scrapper for the builder
/// stage's source acquisition step.
///
/// The builder module embeds these instructions into the Dockerfile
/// without modification.
#[derive(Debug, Clone)]
pub struct CloneInstructions {
    /// Ordered list of Dockerfile lines (RUN, ENV, WORKDIR, ARG, etc.)
    /// that clone the repository at the target version.
    pub lines: Vec<String>,

    /// The working directory after clone (where the builder should
    /// continue from).
    pub workdir: String,
}
```

#### Derive strategy

- `VersionInfo`: `Debug`, `Clone`, `PartialEq`. Not `Default` (no meaningful
  default). `Eq` not derived because `chrono::DateTime` does not implement
  `Eq` on all platforms.
- `TagFilter`: `Debug`, `Clone`, `Default`, `PartialEq`.
- `FetchOptions`: `Debug`, `Clone`. Not `Default` (filter must be explicitly
  constructed from recipe).
- `CloneOptions`: `Debug`, `Clone`. Not `Default` (must be explicitly
  constructed from recipe `[scrapper]` section).
- `CloneInstructions`: `Debug`, `Clone`.

#### Visibility

- `VcsClient` trait: `pub` (used by pipeline orchestrator and binaries).
- `VersionInfo`, `TagFilter`, `FetchOptions`: `pub` (shared data model).
- `GithubClient`: `pub` (re-exported from `mod.rs` behind feature gate).
- Internal GitHub API types (`api.rs`): `pub(crate)` or `pub(super)`.

### Feature gate

| Feature flag | Module | Default |
| :----------- | :----- | :------ |
| `vcs-github` | scrapper | Yes |

From architecture-overview.md Section 3.1. Compile-time enforcement:

```
#[cfg(not(feature = "vcs-github"))]
compile_error!("At least one VCS backend must be enabled (e.g., vcs-github)");
```

### Configuration

The scrapper module does not have its own section in `config.toml`. Its inputs
come from the recipe TOML and `.env`:

| Input | Source | Notes |
| :---- | :----- | :---- |
| Repository URL | Recipe `[header] repo` | Per-recipe |
| Include patterns | Recipe `[header] include_patterns` | Per-recipe, glob syntax |
| Exclude patterns | Recipe `[header] exclude_patterns` | Per-recipe, glob syntax |
| GitHub PAT | `.env` `GH_PAT` | Loaded via `dotenvy`, wrapped in `secrecy::SecretString` |
| GitHub user | `.env` `GH_USER` | Loaded via `dotenvy`, wrapped in `secrecy::SecretString` |
| Builder image | Recipe `[scrapper] image` | Base image for the Dockerfile builder stage (e.g., "golang:1.23-alpine3.21") |
| Install commands | Recipe `[scrapper] install` | OS dependencies to install in builder stage |
| Clone method | Recipe `[scrapper] method` | Clone strategy (e.g., "try-authenticated-clone") |
| Clone directory | Recipe `[scrapper] directory` | Destination directory inside builder container |
| Clone env vars | Recipe `[scrapper] env` | Environment variables for clone step |

The recipe `[scrapper]` section is consumed by the scrapper module. It describes
both API scraping config and source acquisition (clone method, builder image,
install commands, env vars). The builder module does NOT consume `[scrapper]`
directly -- it receives `CloneInstructions` from the scrapper.

No new config.toml keys required. No new CLI flags required (the pipeline
orchestrator passes recipe-derived values into `FetchOptions` and
`CloneOptions`).

### Error types

```rust
/// Errors originating from VCS fetch operations.
///
/// Owned by the `scrapper` module. Defined in `src/scrapper/error.rs`.
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    /// HTTP request failed (network error, DNS failure, timeout).
    #[error("VCS network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Authentication failed (invalid or expired PAT).
    #[error("VCS authentication failed: {message}")]
    Auth {
        message: String,
    },

    /// GitHub API rate limit exhausted. Includes reset time.
    #[error("VCS rate limit exceeded, resets at {reset_at}")]
    RateLimit {
        reset_at: chrono::DateTime<chrono::Utc>,
    },

    /// Requested tag does not exist in the repository.
    #[error("tag not found: {tag_name} in {repo_url}")]
    TagNotFound {
        repo_url: String,
        tag_name: String,
    },

    /// Failed to parse VCS API response.
    #[error("VCS response parse error: {message}")]
    Parse {
        message: String,
        #[source]
        source: Option<serde_json::Error>,
    },

    /// Repository URL is malformed or unsupported.
    #[error("invalid repository URL: {url}")]
    InvalidUrl {
        url: String,
    },

    /// Glob pattern in include/exclude filter is invalid.
    #[error("invalid glob pattern: {pattern}")]
    InvalidGlob {
        pattern: String,
        #[source]
        source: glob::PatternError,
    },

    /// Clone method specified in recipe `[scrapper]` is unsupported.
    #[error("unsupported clone method: {method}")]
    InvalidCloneMethod {
        method: String,
    },
}
```

Error mapping to Unrecoverable Error Strategy
(architecture-overview.md Section 6.3):

| VcsError variant | CLI | Daemon | RPC |
| :--------------- | :-- | :----- | :-- |
| Network | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| Auth | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| RateLimit | Log + exit (with retry hint) | Log + backoff + retry on next poll | Log + return idle |
| TagNotFound | Log + exit | Log + skip (not a failure) | Log + return idle |
| Parse | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| InvalidUrl | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| InvalidGlob | Log + exit | Log + notify + persist failure + continue | Log + return idle |
| InvalidCloneMethod | Log + exit | Log + notify + persist failure + continue | Log + return idle |

### Dependencies

External crates needed:

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `reqwest` | HTTP client for GitHub REST API | Already used by other modules. TLS backend via ssl feature gate |
| `serde` + `serde_json` | Deserialize GitHub API JSON responses | Already a project dependency |
| `secrecy` | Wrap `GH_PAT` to prevent accidental logging | Already in architecture-overview.md conventions |
| `dotenvy` | Load `.env` file for secrets | Already in architecture-overview.md conventions |
| `chrono` | Parse and represent timestamps from API responses | Needed for `published_at` field |
| `semver` | Parse semver tags for sorting and comparison | Needed for `VersionInfo.semver` |
| `glob` | Compile and match include/exclude patterns | Needed for `TagFilter` |
| `tracing` | Structured logging for API calls, rate limits, retries | Already a project convention |

Delegate to @lead-dev: evaluate `semver` for semver parsing, check latest
version, API surface, musl/aarch64 compatibility.

Delegate to @lead-dev: evaluate `glob` for glob pattern matching, check latest
version, API surface, musl/aarch64 compatibility.

Delegate to @lead-dev: evaluate `chrono` for timestamp handling, check latest
version, musl/aarch64 compatibility. Note: `chrono` has had past issues with
musl; verify current status.

---

## Interface contract

```rust
// -- src/scrapper/mod.rs --

pub trait VcsClient: Send + Sync {
    async fn fetch_versions(
        &self,
        repo_url: &str,
        options: &FetchOptions,
    ) -> Result<Vec<VersionInfo>, VcsError>;

    async fn fetch_version(
        &self,
        repo_url: &str,
        tag_name: &str,
    ) -> Result<VersionInfo, VcsError>;

    fn clone_instructions(
        &self,
        repo_url: &str,
        clone_options: &CloneOptions,
        version: &str,
    ) -> Result<CloneInstructions, VcsError>;
}

// -- src/scrapper/github/mod.rs (behind feature "vcs-github") --

pub struct GithubClient { /* ... */ }

impl GithubClient {
    /// Create a new GithubClient.
    ///
    /// # Arguments
    ///
    /// * `pat` - Optional GitHub Personal Access Token wrapped in SecretString.
    ///           If `None`, unauthenticated mode is used (lower rate limits).
    /// * `http_client` - Shared reqwest::Client (connection pooling).
    pub fn new(
        pat: Option<secrecy::SecretString>,
        http_client: reqwest::Client,
    ) -> Self;
}

impl VcsClient for GithubClient {
    async fn fetch_versions(
        &self,
        repo_url: &str,
        options: &FetchOptions,
    ) -> Result<Vec<VersionInfo>, VcsError>;

    async fn fetch_version(
        &self,
        repo_url: &str,
        tag_name: &str,
    ) -> Result<VersionInfo, VcsError>;

    fn clone_instructions(
        &self,
        repo_url: &str,
        clone_options: &CloneOptions,
        version: &str,
    ) -> Result<CloneInstructions, VcsError>;
}
```

---

## GitHub API integration details

### Endpoints used

| Endpoint | Purpose |
| :------- | :------ |
| `GET /repos/{owner}/{repo}/releases` | Fetch releases (includes tag, commit, prerelease flag, publish date) |
| `GET /repos/{owner}/{repo}/tags` | Fetch bare tags (for repos that use tags without releases) |
| `GET /repos/{owner}/{repo}/releases/tags/{tag}` | Fetch a single release by tag name |
| `GET /repos/{owner}/{repo}/git/refs/tags/{tag}` | Resolve a tag to a commit SHA |

### Authentication

```
Authorization: Bearer <GH_PAT>
```

When `GH_PAT` is not set in `.env`, the client operates without the
`Authorization` header. Rate limits are significantly lower (60 req/hour vs
5000 req/hour).

The PAT is stored in a `secrecy::SecretString` and is never logged, serialized,
or included in error messages.

### Rate limit handling

GitHub returns rate limit information in response headers:

```
X-RateLimit-Limit: 5000
X-RateLimit-Remaining: 4998
X-RateLimit-Reset: 1713062400
```

Strategy:
1. After each API call, parse and store the rate limit headers.
2. If `X-RateLimit-Remaining` drops below a threshold (e.g., 10), log a
   warning via `tracing::warn!`.
3. On a 403 with `X-RateLimit-Remaining: 0` or a 429 response, calculate
   the wait duration from `X-RateLimit-Reset` and:
   - **CLI mode**: return `VcsError::RateLimit` with the reset time. The CLI
     handler logs the time and exits.
   - **Daemon mode**: the orchestrator can choose to sleep until reset or
     skip to the next poll cycle.

### Pagination

GitHub paginates list endpoints (default 30, max 100 per page). The `Link`
response header contains `rel="next"` URLs.

Strategy:
1. Request with `per_page=100` to minimize round-trips.
2. Parse the `Link` header after each response.
3. If a `rel="next"` link exists, follow it.
4. Accumulate results across pages.
5. Stop when there is no `next` link, or when `max_results` is reached.

### Repository URL parsing

The `repo_url` from recipe `[header] repo` is a full HTTPS URL like
`https://github.com/cosmos/gaia`. The GitHub client must parse out the
`owner` and `repo` components:

```
https://github.com/{owner}/{repo}  ->  owner = "cosmos", repo = "gaia"
```

If the URL does not match the GitHub pattern, return `VcsError::InvalidUrl`.

---

## CLI vs Daemon mode behavior

The `VcsClient` trait itself is mode-agnostic. The mode-specific behavior is
handled by the pipeline orchestrator, not by the scrapper module.

| Concern | CLI | Daemon |
| :------ | :-- | :----- |
| Invocation | Single call to `fetch_versions` or `fetch_version` | Periodic calls on each poll tick |
| Version filtering | All matching versions, or a single specified version | Pipeline orchestrator compares against `BuildStore` to find new versions |
| Rate limit handling | Error -> exit | Error -> backoff or skip to next cycle |
| Result set | Full list returned to caller | Full list returned; orchestrator diffs against DB |

The daemon orchestrator is responsible for:
1. Calling `fetch_versions` on each poll tick.
2. Querying `BuildStore` for already-built versions.
3. Computing the diff (new versions to build).
4. Feeding new versions into the builder.

The scrapper module does NOT depend on the saver module. This keeps the
dependency graph acyclic (architecture-overview.md Appendix A).

---

## Glob filtering algorithm

```
Input: Vec<VersionInfo> (all fetched tags), TagFilter

1. If filter.include is non-empty:
     keep only tags where tag_name matches at least one include pattern
2. If filter.exclude is non-empty:
     remove tags where tag_name matches any exclude pattern
3. Return remaining tags

Matching uses standard glob syntax (*, ?, [abc]).
Example: include = ["v*"], exclude = ["*-rc*", "*-beta*"]
  -> keeps "v19.0.0", removes "v19.0.0-rc1", "v19.0.0-beta2"
```

Filtering is applied inside `fetch_versions` after all pages have been
accumulated and before the result is returned to the caller.

---

## Module interaction diagram

```
                     +-------------+
                     |   recipe    |
                     | (parsed)    |
                     +------+------+
                            |
                  [header]: repo_url, include/exclude patterns
                  [scrapper]: image, install, method, env, directory
                            |
                            v
+----------+       +--------+--------+
| .env     |------>|    scrapper     |
| GH_PAT   |  pat |                 |
| GH_USER  |      | VcsClient trait |
+----------+       +---+-------+----+
                       |       |
          fetch_versions()   clone_instructions()
                       |       |
                       v       v
              Vec<VersionInfo>  CloneInstructions
                       |       |
                       v       v
                   +---+-------+----+
                   |    builder     |
                   | (embeds clone  |
                   |  instructions  |
                   |  into          |
                   |  Dockerfile)   |
                   +----------------+
                            
          GitHub REST API (reqwest HTTP)
                       |
                       v
              +--------+--------+
              | api.github.com  |
              +-----------------+

Daemon orchestrator (not part of scrapper):

+----------+       +----------+       +----------+
| scrapper |------>| diff     |------>| builder  |
| fetch    | all   | against  | new   | pipeline |
| versions | tags  | saver DB | only  |          |
+----------+       +----------+       +----------+
```

---

## Testing strategy

### Unit tests

| Test | What it validates |
| :--- | :---------------- |
| `TagFilter` with empty include/exclude | Returns all tags unfiltered |
| `TagFilter` with include only | Keeps only matching tags |
| `TagFilter` with exclude only | Removes matching tags |
| `TagFilter` with both | Exclude takes precedence over include |
| `TagFilter` with invalid glob | Returns `VcsError::InvalidGlob` |
| GitHub URL parsing | Extracts owner/repo from valid URLs |
| GitHub URL parsing (invalid) | Returns `VcsError::InvalidUrl` for non-GitHub URLs |
| `VersionInfo` semver parsing | Correctly parses "v19.0.0", "v1.2.3-rc1", "not-semver" |
| Rate limit header parsing | Extracts remaining count and reset time |
| Link header pagination parsing | Extracts next URL from various `Link` header formats |
| `CloneOptions` from recipe `[scrapper]` | Correct deserialization of method, directory, env |
| `clone_instructions` happy path | Produces correct Dockerfile lines for authenticated clone |
| `clone_instructions` unauthenticated | Produces clone without GH_PAT when method is plain clone |
| `clone_instructions` invalid method | Returns `VcsError::InvalidCloneMethod` |
| `clone_instructions` invalid URL | Returns `VcsError::InvalidUrl` |

### Integration tests (mocked HTTP)

| Test | What it validates |
| :--- | :---------------- |
| `GithubClient::fetch_versions` happy path | Multi-page fetch with pagination, returns correct `VersionInfo` list |
| `GithubClient::fetch_versions` with filters | Glob filtering applied correctly |
| `GithubClient::fetch_version` happy path | Single tag fetch returns correct data |
| `GithubClient::fetch_version` not found | Returns `VcsError::TagNotFound` |
| Rate limit exceeded response | Returns `VcsError::RateLimit` with correct reset time |
| Auth failure (401) | Returns `VcsError::Auth` |
| Unauthenticated mode | Works without PAT, sends no Authorization header |
| Malformed JSON response | Returns `VcsError::Parse` |

HTTP mocking: use a mock server (e.g., `wiremock` or `mockito`) to simulate
GitHub API responses. Delegate crate choice to @lead-dev.

### What NOT to test in this module

- Daemon polling loop logic (owned by the daemon orchestrator).
- BuildStore diffing (owned by saver/orchestrator).
- Network-level TLS behavior (owned by reqwest/ssl feature).

---

## Resolved questions

| ID | Question | Decision | Status |
| :- | :------- | :------- | :----- |
| S1 | Should `fetch_versions` return both tags and releases, or should they be separate methods? | `fetch_versions` returns tags and releases MERGED into a single unified return type (`Vec<VersionInfo>`). Single method, no separate tag-only or release-only methods. Deduplication by tag name, with release metadata taking precedence over bare tags. | RESOLVED |
| S2 | Should the scrapper module also handle cloning the repository (the `method = "try-authenticated-clone"` in recipe `[scrapper]`)? Or is cloning a builder responsibility? | Scrapper owns BOTH VCS API fetching AND source cloning. The scrapper module is responsible for: (1) fetching available versions via GitHub API, (2) producing clone instructions for the builder. The builder receives already-prepared clone instructions from the scrapper via `CloneInstructions`. | RESOLVED |
| S3 | The recipe `[scrapper]` section defines a builder image, install commands, and clone method. Should this spec cover the `[scrapper]` recipe section, or is that section consumed by the builder module? | The recipe `[scrapper]` section is consumed by the scrapper module. It describes both API scraping config and source acquisition (clone method, env vars, builder image, install commands, directory). The builder does NOT consume `[scrapper]` directly. | RESOLVED |
