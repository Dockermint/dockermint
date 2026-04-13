# Feature: Recipe Parsing and Flavor Resolution

## Context

The `recipe` module is the heart of Dockermint's extensibility model. It
parses recipe TOML files, resolves flavor selections through the priority
chain, validates flavor compatibility rules, and produces a fully resolved
build specification. The fundamental design principle is that adding a new
chain requires only creating a new TOML file -- no Rust code modifications.

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)
Architecture reference: docs/specs/architecture-overview.md, Sections 5, 1.2

---

## Requirements

1. [confirmed] Parse recipe TOML files from the configured `recipes_dir`
2. [confirmed] Discover recipes by scanning for `*.toml` files (no registration step)
3. [confirmed] Validate recipe schema version against supported versions
4. [confirmed] Flavor resolution: CLI args > config.toml per-recipe > config.toml global > recipe defaults
5. [confirmed] Flavor values validated against `[flavours.available]` in the recipe
6. [confirmed] Flavor compatibility rules expressed in recipe TOML `[[flavours.incompatible]]` (Q4)
7. [confirmed] Variable system: `{{UPPERCASE}}` host vars, `{{lowercase}}` build vars
8. [confirmed] Profile system: `[profiles.<dimension>.<value>]` injects variables
9. [confirmed] Support sidecar recipes (Axelar: Tofnd/Vald, Injective: Peggo)
10. [confirmed] Adding a chain requires no Rust code changes
11. [confirmed] Incompatible flavors trigger Unrecoverable Error Strategy

---

## Architecture

### Module placement

```
src/recipe/
    mod.rs              -- Public API: discover_recipes(), parse_recipe(),
                           resolve_flavors(), validate_compatibility(), re-exports
    error.rs            -- RecipeError enum (thiserror)
    schema.rs           -- Recipe TOML struct definitions (serde Deserialize)
    discovery.rs        -- Directory scanning for *.toml files
    flavor.rs           -- Flavor resolution and validation logic
    compatibility.rs    -- Flavor compatibility rule parsing and enforcement
    variable.rs         -- Variable system (host vars, build vars, shell vars)
    profile.rs          -- Profile resolution ([profiles.<dim>.<value>])
```

This module is NOT behind a feature gate. The recipe format is the core
contract of the project. See architecture-overview.md Section 2.3.

### Recipe TOML structure (full schema)

The schema below is derived from the existing recipes (`cosmos-gaiad.toml`,
`kyve-kyved.toml`) and extended with the `[flavours.incompatible]` section
per CEO decision Q4.

```toml
# ================================================================
# Dockermint Recipe Schema (version 1)
# ================================================================

# ----------------------------------------------------------------
# Metadata
# ----------------------------------------------------------------
[meta]
schema_version = 1                        # REQUIRED: recipe schema version
min_dockermint_version = "0.1.0"          # Minimum Dockermint version required

# ----------------------------------------------------------------
# Header: chain identification and VCS configuration
# ----------------------------------------------------------------
[header]
name = "ChainName"                        # Human-readable chain name
repo = "https://github.com/org/repo"      # Git repository URL
type = "golang"                           # Build system type (dispatches to builder submodule)
binary_name = "binaryd"                   # Name of the compiled binary
include_patterns = ""                     # Glob pattern for VCS tag inclusion (empty = all)
exclude_patterns = ""                     # Glob pattern for VCS tag exclusion (empty = none)

# ----------------------------------------------------------------
# Flavours: available values, defaults, and compatibility rules
# ----------------------------------------------------------------
[flavours.available]
architecture = ["x86_64", "aarch64"]
db_backend = ["goleveldb", "pebbledb"]
binary_type = ["dynamic", "static"]
running_env = ["alpine3.23", "bookworm", "distroless"]
running_user = ["root", "custom", "dockermint"]
build_tags = ["netgo", "ledger", "muslc"]
# Recipe-specific dimensions are allowed:
# network = ["mainnet", "kaon"]

[flavours.default]
architecture = "{{HOST_ARCH}}"
db_backend = "goleveldb"
binary_type = "static"
running_env = "alpine3.23"
running_user = "root"
build_tags = ["netgo", "muslc"]
# network = "mainnet"

# ----------------------------------------------------------------
# Flavor compatibility rules (CEO decision Q4)
# ----------------------------------------------------------------
# Each entry defines a constraint. Rules are evaluated AFTER flavor
# resolution. Violations trigger Unrecoverable Error Strategy.
#
# Two rule types:
#   "requires" -- when condition matches, additional constraints must hold
#   "deny"     -- when condition matches, the combination is forbidden

[[flavours.incompatible]]
rule = "static requires muslc build tag"
when = { binary_type = "static" }
requires = { build_tags = ["muslc"] }

[[flavours.incompatible]]
rule = "distroless requires static binary"
when = { running_env = "distroless" }
requires = { binary_type = "static" }

# ----------------------------------------------------------------
# Scrapper: builder image and clone configuration
# ----------------------------------------------------------------
[scrapper]
image = "golang:1.23-alpine3.21"          # Builder stage base image
install = "apk add --no-cache git"        # Package install command in builder
env = ["{{GH_USER}}", "{{GH_PAT}}"]      # Environment variables for clone
method = "try-authenticated-clone"         # Clone strategy
directory = "{{repository_path}}"         # Clone target directory

# ----------------------------------------------------------------
# Build variables (resolved during Dockerfile generation)
# ----------------------------------------------------------------
[variables]
repo_commit = { shell = "git log -1 --format='%H'" }
repo_version = { shell = "..." }
golang_version = { shell = "..." }
# Any number of additional variables. Keys become {{key_name}} placeholders.

# ----------------------------------------------------------------
# Profiles: dimension-value-specific variable overrides
# ----------------------------------------------------------------
[profiles.network.mainnet]
denom = "ukyve"
team_tge = "2023-03-14T14:03:14"
# Variables defined here are injected when network=mainnet is selected

[profiles.network.kaon]
denom = "tkyve"
team_tge = "2023-02-07T14:00:00"

# ----------------------------------------------------------------
# Builder: OS-specific build dependency installation
# ----------------------------------------------------------------
[builder.install]
alpine = "apk add --no-cache alpine-sdk linux-headers"
ubuntu = "apt-get update && apt-get install -y ..."

# ----------------------------------------------------------------
# Pre-build: conditional Dockerfile instructions
# ----------------------------------------------------------------
[[pre_build]]
condition = "static"                      # Flavor value that triggers this step
instruction = "ADD"                       # Dockerfile instruction
source = "https://example.com/lib.a"     # Source URL or path
dest = "/lib/lib.a"                      # Destination in builder stage

# ----------------------------------------------------------------
# Build configuration
# ----------------------------------------------------------------
[build.env]
CGO_ENABLED = "1"                         # Build environment variables

[build.linker.flags]
dynamic = "-w -s ..."                     # Linker flags for dynamic builds
static = "-linkmode=external -w -s ..."   # Linker flags for static builds

[build.linker.variables]
"github.com/cosmos/cosmos-sdk/version.Name" = "chain"
# Go -ldflags -X variables for version injection

[build.path]
path = "{{repository_path}}/cmd/binaryd"  # Go build target path

# ----------------------------------------------------------------
# User configuration (non-root execution)
# ----------------------------------------------------------------
[user.dockermint]
username = "dockermint"
uid = 10000
gid = 10000

# ----------------------------------------------------------------
# Copy: files from builder stage to runner stage
# ----------------------------------------------------------------
[copy]
"/go/bin/binaryd" = { dest = "/usr/bin/{{binary_name}}", type = "entrypoint" }

[copy.dynamic]
# Conditional copy rules keyed by flavor value
"/path/to/lib.so" = { dest = "/lib/lib.so", type = "dyn-library" }

# ----------------------------------------------------------------
# Exposed ports
# ----------------------------------------------------------------
[expose]
ports = [
    { port = 26656, description = "P2P" },
    { port = 26657, description = "RPC" },
]

# ----------------------------------------------------------------
# OCI image labels
# ----------------------------------------------------------------
[labels]
"org.opencontainers.image.created" = "{{CREATION_TIMESTAMP}}"
"org.opencontainers.image.version" = "{{repo_version}}"
# Standard OCI labels with variable interpolation

# ----------------------------------------------------------------
# Image tag template
# ----------------------------------------------------------------
[image]
tag = "chain-binary-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"
```

### Type design

#### Parsed recipe (raw deserialization)

```
RawRecipe
  +-- meta: RecipeMeta
  +-- header: RecipeHeader
  +-- flavours: RawFlavours
  +-- scrapper: ScrapperConfig
  +-- variables: HashMap<String, VariableDefinition>
  +-- profiles: Option<HashMap<String, HashMap<String, HashMap<String, toml::Value>>>>
  +-- builder: BuilderInstallConfig
  +-- pre_build: Option<Vec<PreBuildStep>>
  +-- build: BuildConfig
  +-- user: Option<HashMap<String, UserConfig>>
  +-- copy: CopyConfig
  +-- expose: ExposeConfig
  +-- labels: HashMap<String, String>
  +-- image: ImageConfig

RecipeMeta
  +-- schema_version: u32
  +-- min_dockermint_version: String

RecipeHeader
  +-- name: String
  +-- repo: String
  +-- type_: String                    -- "golang" (serde rename from "type")
  +-- binary_name: String
  +-- include_patterns: String
  +-- exclude_patterns: String

RawFlavours
  +-- available: HashMap<String, Vec<toml::Value>>
  +-- default: HashMap<String, toml::Value>
  +-- incompatible: Option<Vec<IncompatibilityRule>>

IncompatibilityRule
  +-- rule: String                     -- Human-readable description
  +-- when: HashMap<String, toml::Value>  -- Triggering condition
  +-- requires: Option<HashMap<String, toml::Value>>  -- Required constraints
  +-- action: Option<String>           -- "deny" for flat prohibition (default: "requires")

VariableDefinition
  +-- shell: String                    -- Shell command to execute
```

#### Resolved recipe (after flavor resolution)

```
ResolvedRecipe
  +-- meta: RecipeMeta
  +-- header: RecipeHeader
  +-- resolved_flavors: HashMap<String, ResolvedFlavorValue>
  +-- scrapper: ScrapperConfig
  +-- variables: HashMap<String, VariableDefinition>
  +-- profile_variables: HashMap<String, toml::Value>  -- Merged from selected profiles
  +-- builder: BuilderInstallConfig
  +-- pre_build: Vec<PreBuildStep>     -- Filtered by resolved flavors
  +-- build: BuildConfig
  +-- user: Option<UserConfig>         -- Resolved for selected running_user
  +-- copy: ResolvedCopyConfig         -- Merged base + conditional copies
  +-- expose: ExposeConfig
  +-- labels: HashMap<String, String>
  +-- image: ImageConfig

ResolvedFlavorValue
  -- Single(String)                    -- Scalar value (e.g., db_backend = "goleveldb")
  -- List(Vec<String>)                 -- Array value (e.g., build_tags = ["netgo", "muslc"])
```

### Flavor system

#### Flavor dimensions

Flavor dimensions are NOT hardcoded in Rust. Each recipe defines its own
dimensions in `[flavours.available]`. Dockermint treats dimensions as
opaque string keys with values that are either strings or arrays of strings.

Standard dimensions observed in existing recipes:

| Dimension | Type | Example values | Purpose |
| :-------- | :--- | :------------- | :------ |
| `architecture` | String | `x86_64`, `aarch64` | Target CPU architecture |
| `db_backend` | String | `goleveldb`, `pebbledb` | Database engine |
| `binary_type` | String | `dynamic`, `static` | Linking strategy |
| `running_env` | String | `alpine3.23`, `bookworm`, `distroless` | Runner base image |
| `running_user` | String | `root`, `custom`, `dockermint` | Container user |
| `build_tags` | Array | `["netgo", "ledger", "muslc"]` | Go build tags |
| `network` | String | `mainnet`, `kaon` | Chain network (recipe-specific) |

Recipes may introduce arbitrary custom dimensions. The Rust code validates
that selected values exist in the available set but does not interpret their
semantic meaning.

#### Flavor resolution order

For each dimension, the resolved value is the first non-empty source:

```
Priority 1: CLI argument
    e.g., --db-backend pebbledb

Priority 2: config.toml per-recipe override
    [flavours.recipes."cosmos-gaiad"]
    db_backend = "pebbledb"

Priority 3: config.toml global override
    [flavours]
    db_backend = "goleveldb"

Priority 4: Recipe default
    [flavours.default]
    db_backend = "goleveldb"
```

If no source provides a value for a dimension, and the dimension has no
default in the recipe, the build fails with `RecipeError::MissingFlavor`.

If the resolved value is not in `[flavours.available]` for that dimension,
the build fails with `RecipeError::InvalidFlavorValue`.

#### Flavor compatibility validation

After flavor resolution, compatibility rules from `[[flavours.incompatible]]`
are evaluated:

```
for each rule in recipe.flavours.incompatible:
    if all conditions in rule.when match the resolved flavors:
        if rule.action == "deny":
            -> Error: combination explicitly denied
        else if rule.requires is present:
            for each (dimension, required_value) in rule.requires:
                if resolved_flavors[dimension] does not satisfy required_value:
                    -> Error: incompatible flavor combination
```

The `requires` field semantics:
- For string dimensions: resolved value must equal the required value
- For array dimensions: resolved array must contain all required values

Violations produce `RecipeError::IncompatibleFlavors` with:
- The rule description (human-readable)
- The conflicting flavor values
- Which constraint was violated

### Variable system

Two types of variables distinguished by case convention:

#### Host variables (`{{UPPERCASE}}`)

| Variable | Source | Resolved when |
| :------- | :----- | :------------ |
| `{{HOST_ARCH}}` | Host CPU architecture | Before Dockerfile generation |
| `{{GH_PAT}}` | `.env` file | Before Dockerfile generation |
| `{{GH_USER}}` | `.env` file | Before Dockerfile generation |
| `{{CREATION_TIMESTAMP}}` | System clock (RFC 3339) | Before Dockerfile generation |
| `{{SEMVER_TAG}}` | VCS tag being built | Before Dockerfile generation |
| `{{BUILD_TAGS_COMMA_SEP}}` | Resolved `build_tags` flavor, comma-joined | After flavor resolution |

Host variables are resolved by Dockermint before Dockerfile generation.
They are injected into the template engine as key-value pairs.

#### Build variables (`{{lowercase}}`)

Build variables come from three sources:

1. **Recipe `[variables]` section**: Shell commands executed inside the
   builder container to capture dynamic values (e.g., Go module versions).

2. **Resolved flavor values**: Each resolved flavor dimension becomes a
   lowercase variable (e.g., `{{db_backend}}`, `{{binary_name}}`).

3. **Profile variables**: When a profile is selected (e.g., `network=mainnet`),
   variables from `[profiles.network.mainnet]` are injected (e.g., `{{denom}}`).

Build variables are resolved during Dockerfile generation. Shell-type
variables (`{ shell = "..." }`) are executed as `RUN` commands in the
builder stage, with output captured via shell substitution.

#### Variable resolution order

For build variables, if a name appears in multiple sources:

```
1. Profile variables (highest priority -- most specific)
2. Resolved flavor values
3. Recipe [variables] section (lowest priority)
```

### Recipe discovery

```
1. Read recipes_dir from AppConfig (GeneralConfig.recipes_dir)
2. Scan directory for all files matching *.toml
3. For each file:
   a. Deserialize into RawRecipe
   b. Validate schema_version against SUPPORTED_RECIPE_SCHEMA_VERSIONS
   c. Validate min_dockermint_version against current binary version
   d. Collect into Vec<RawRecipe>
4. Return the list (order: alphabetical by filename)
```

No registration step. No manifest file. Adding a new chain:

1. Create a new `.toml` file in `recipes/` following the schema.
2. Restart or trigger a new build.
3. No Rust code changes.

### Recipe type dispatch

The `[header] type` field determines which builder submodule handles the
build. Currently supported:

| Type | Builder submodule | Notes |
| :--- | :---------------- | :---- |
| `"golang"` | `builder/go` | Go-based Cosmos SDK chains |

Adding a new type (e.g., `"rust"`) requires a new builder submodule. This
is the ONLY case where recipe support requires Rust code changes, and it
only applies for entirely new build system types.

### Sidecar support

Sidecars are separate binaries that run alongside the main chain node.
Examples from the roadmap:
- Axelar: Tofnd (threshold signature), Vald (validator)
- Injective: Peggo (Pegasus bridge orchestrator)

Sidecars are modeled as separate recipe files:

```
recipes/
  axelar-axelard.toml       -- Main chain binary
  axelar-tofnd.toml         -- Sidecar: Tofnd
  axelar-vald.toml          -- Sidecar: Vald
  injective-injectived.toml -- Main chain binary
  injective-peggo.toml      -- Sidecar: Peggo
```

Each sidecar recipe is a fully independent recipe file with its own header,
flavors, variables, and build configuration. The `[header] name` field groups
them by chain (e.g., "Axelar"), and the `[header] binary_name` distinguishes
them. Dockermint processes them identically to main chain recipes.

This approach requires no special Rust code for sidecars. A sidecar is just
another recipe.

### Error types

```
RecipeError (thiserror)
  +-- FileNotFound { path: PathBuf }
  +-- ParseError { path: PathBuf, source: toml::de::Error }
  +-- UnsupportedSchemaVersion { path: PathBuf, found: u32, supported: Vec<u32> }
  +-- IncompatibleDockermintVersion { path: PathBuf, required: String, current: String }
  +-- MissingFlavor { recipe: String, dimension: String }
  +-- InvalidFlavorValue { recipe: String, dimension: String, value: String, available: Vec<String> }
  +-- IncompatibleFlavors { recipe: String, rule: String, details: String }
  +-- InvalidVariableDefinition { recipe: String, variable: String, reason: String }
  +-- ProfileNotFound { recipe: String, dimension: String, value: String }
  +-- DiscoveryError { dir: PathBuf, source: std::io::Error }
  +-- EmptyRecipesDir { dir: PathBuf }
  +-- UnknownRecipeType { recipe: String, type_: String }
  +-- DuplicateRecipeName { name: String, path1: PathBuf, path2: PathBuf }
```

### Dependencies

| Crate | Use case | Notes |
| :---- | :------- | :---- |
| `serde` | Deserialization framework | With `derive` feature |
| `toml` | TOML parser | Recipe files |
| `semver` | Version comparison | `min_dockermint_version` check |
| `glob` | Pattern matching | `include_patterns` / `exclude_patterns` (used by scrapper, but recipe stores the patterns) |
| `thiserror` | Error type definitions | `RecipeError` |

Delegate to @lead-dev: evaluate `semver` crate for latest version, API
surface, musl/aarch64 compatibility.

---

## Interface contract

```rust
/// Discover all recipe files in the configured recipes directory.
///
/// Scans for *.toml files, parses each into a RawRecipe, validates
/// schema version and min Dockermint version.
///
/// # Arguments
///
/// * `recipes_dir` - Path to the recipes directory
/// * `current_version` - Current Dockermint version string
///
/// # Returns
///
/// Vector of parsed raw recipes, sorted alphabetically by filename
///
/// # Errors
///
/// Returns RecipeError::DiscoveryError if the directory cannot be read.
/// Returns RecipeError::EmptyRecipesDir if no TOML files are found.
/// Returns RecipeError::ParseError for malformed recipe files.
/// Returns RecipeError::UnsupportedSchemaVersion for unknown schema versions.
/// Returns RecipeError::IncompatibleDockermintVersion if the recipe
/// requires a newer Dockermint version.
pub fn discover_recipes(
    recipes_dir: &Path,
    current_version: &str,
) -> Result<Vec<RawRecipe>, RecipeError>;

/// Resolve flavors for a specific recipe through the priority chain.
///
/// # Arguments
///
/// * `recipe` - The parsed raw recipe
/// * `cli_flavors` - Flavor overrides from CLI arguments
/// * `config_flavors` - Flavor overrides from config.toml (global + per-recipe)
///
/// # Returns
///
/// HashMap of dimension name to resolved value
///
/// # Errors
///
/// Returns RecipeError::MissingFlavor if a required dimension has no value.
/// Returns RecipeError::InvalidFlavorValue if a resolved value is not in
/// the available set.
pub fn resolve_flavors(
    recipe: &RawRecipe,
    cli_flavors: &HashMap<String, toml::Value>,
    config_flavors: &FlavoursConfig,
) -> Result<HashMap<String, ResolvedFlavorValue>, RecipeError>;

/// Validate resolved flavors against the recipe's compatibility rules.
///
/// # Arguments
///
/// * `recipe` - The parsed raw recipe (contains incompatibility rules)
/// * `resolved` - The resolved flavor values
///
/// # Returns
///
/// Ok(()) if all compatibility rules pass
///
/// # Errors
///
/// Returns RecipeError::IncompatibleFlavors with the violated rule
/// description and details
pub fn validate_compatibility(
    recipe: &RawRecipe,
    resolved: &HashMap<String, ResolvedFlavorValue>,
) -> Result<(), RecipeError>;

/// Produce a fully resolved recipe with all flavors applied, profiles
/// merged, and conditional sections filtered.
///
/// # Arguments
///
/// * `recipe` - The parsed raw recipe
/// * `resolved_flavors` - Validated resolved flavor values
///
/// # Returns
///
/// A ResolvedRecipe ready for the template engine and builder
///
/// # Errors
///
/// Returns RecipeError if profile resolution or conditional filtering fails
pub fn resolve_recipe(
    recipe: RawRecipe,
    resolved_flavors: HashMap<String, ResolvedFlavorValue>,
) -> Result<ResolvedRecipe, RecipeError>;
```

---

## Module interaction diagram

```
                    recipes/*.toml
                         |
                         v
                  [recipe/discovery.rs]
                         |
                         v
                   Vec<RawRecipe>
                         |
          +--------------+--------------+
          |                             |
          v                             v
   [recipe/flavor.rs]           [recipe/schema.rs]
   resolve_flavors()            (struct definitions)
          |
          v
   HashMap<String, ResolvedFlavorValue>
          |
          v
   [recipe/compatibility.rs]
   validate_compatibility()
          |
          v
   [recipe/profile.rs]
   merge profile variables
          |
          v
   [recipe/variable.rs]
   prepare variable context
          |
          v
    ResolvedRecipe
          |
    +-----+-----+
    |           |
    v           v
[builder]   [scrapper]
(template   (VCS fetch
 engine)     config)
```

---

## Testing strategy

- **Unit tests**: Deserialize each recipe section from TOML fragments.
  Verify all fields are correctly parsed. Unknown fields are rejected.
- **Unit tests**: Flavor resolution with all 4 priority levels. Verify
  CLI args win over config.toml, config.toml wins over recipe defaults.
- **Unit tests**: Flavor validation against available set. Invalid values
  produce `InvalidFlavorValue`. Missing dimensions produce `MissingFlavor`.
- **Unit tests**: Compatibility rule evaluation. Test `requires` rules
  (string and array matching). Test `deny` rules. Test passing combinations.
  Test violation reporting includes rule description and details.
- **Unit tests**: Profile resolution merges variables for selected profile
  value. Missing profile produces `ProfileNotFound`.
- **Unit tests**: Recipe discovery scans directory, sorts alphabetically,
  rejects non-TOML files.
- **Unit tests**: Schema version validation rejects unsupported versions.
- **Unit tests**: Duplicate recipe names detected across files.
- **Integration tests**: Parse the actual `cosmos-gaiad.toml` and
  `kyve-kyved.toml` recipe files from `recipes/`. Verify they parse without
  error and produce correct `RawRecipe` structs.
- **Integration tests**: Full pipeline: discover -> resolve flavors ->
  validate compatibility -> resolve recipe, using fixture recipes.
- **Mock**: File system for recipe directory scanning.

---

## Open questions

| ID | Question | Status | Resolution |
| :- | :------- | :----- | :--------- |
| R1 | Should the `[[flavours.incompatible]]` `when` field support compound conditions (AND of multiple dimensions) or only single-dimension conditions? The schema above supports compound (multiple keys in the `when` table = AND). Confirm this is the intended behavior. | RESOLVED | YES -- compound `when` conditions are supported. Multiple keys in the `when` table are evaluated as a logical AND. All conditions must match for the rule to trigger. |
| R2 | Should recipe-specific custom dimensions (e.g., `network` in Kyve) be validated in any way, or are they purely opaque to Dockermint? Currently proposed: opaque, no semantic interpretation. | RESOLVED | Follow best practices. Custom dimensions remain opaque to Dockermint -- no semantic interpretation. Dockermint validates that selected values exist in the `[flavours.available]` set for that dimension, but does not assign meaning to the dimension name or its values. |
| R3 | For the `[scrapper] method` field, what values are supported beyond `"try-authenticated-clone"`? Is there a registry of clone strategies? | RESOLVED | The `[scrapper]` section is consumed by the scrapper module, not the builder. Values like `method = "try-authenticated-clone"` are scrapper-module concepts. The recipe module parses and stores the scrapper config as-is; it does not validate or interpret the `method` field. The scrapper module owns the registry of valid method values. |
