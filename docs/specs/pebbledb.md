# Feature: PebbleDB Flavor Support (Go Builder)

## Context

PebbleDB is an alternative database backend for Cosmos SDK chains. When a
recipe exposes `pebbledb` in its `[flavours.available] db_backend` list and the
user selects it (via CLI, `config.toml`, or recipe defaults), the builder module
must know how to compile the chain binary with PebbleDB support. This involves
detecting whether the chain supports PebbleDB natively, applying source-level
patches for older chains, and passing the correct build tags and linker flags.

This spec documents the domain knowledge that the Go recipe builder
(`builder/go`) relies on when generating Dockerfiles for PebbleDB builds. It is
a reference companion to `docs/specs/builder.md` (Subsystem 3: Go Recipe
Builder) and `docs/specs/recipe.md` (flavor system).

Roadmap entry: Phase 0 -- Foundation (docs/ROADMAP.md)

---

## Requirements

1. [confirmed] The `db_backend` flavor dimension is recipe-defined in
   `[flavours.available]` and may include `"pebbledb"` as an option.
2. [confirmed] PebbleDB support is natively available starting at
   `github.com/cometbft/cometbft-db` v0.10.0 (CometBFT v0.38.0+ /
   Cosmos SDK v0.50+).
3. [confirmed] Chains whose `go.mod` pins `cometbft-db >= v0.10.0` require
   only build tags and ldflags -- no source-level patching.
4. [confirmed] Chains pinning an older `cometbft-db`, or still using the
   legacy `github.com/tendermint/tm-db`, do NOT support PebbleDB natively
   and must be patched before compilation.
5. [confirmed] Patching uses `go mod edit -replace` with community-maintained
   forks from the `notional-labs` GitHub organization.
6. [confirmed] Three independent replace directives exist, applied only when
   the chain imports the corresponding module:
   - `github.com/cometbft/cometbft-db` -> `github.com/notional-labs/cometbft-db@pebble`
   - `github.com/tendermint/tm-db` -> `github.com/notional-labs/tm-db@v0.6.8-pebble`
   - `github.com/cosmos/cosmos-db` -> `github.com/notional-labs/cosmos-db@pebble`
7. [confirmed] After replace directives, `go mod tidy` must run to resolve
   the new dependency graph before `go build`.
8. [confirmed] PebbleDB is enabled at build time via the `pebbledb` build tag
   (`-tags '...,pebbledb'`).
9. [confirmed] The default database backend is baked into the binary via the
   linker variable
   `-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb`.
10. [confirmed] The `notional-labs` forks maintain multiple branches and tags
    (`@pebble`, `@v0.11.0-pebble-1`, `@v0.9.1-pebble`, specific commit
    hashes). Not all are mutually compatible with every chain version.
11. [confirmed] Existing nodes switching from `goleveldb` to `pebbledb` must
    re-sync, import a PebbleDB snapshot, or convert using the
    `Dockermint/pebblify` tool. The two backends are not on-disk compatible.

---

## Architecture

### Module placement

This spec does not introduce a new module. PebbleDB support is expressed
entirely through:

- **Recipe TOML** -- `[flavours.available]`, `[flavours.default]`,
  `[build.linker.flags]`, `[build.linker.variables]`, and a new
  `[[pre_build]]` section for patching.
- **Go builder** (`src/builder/go/`) -- the existing Dockerfile generation
  logic already handles build tags, linker flags, linker variables, and
  conditional `[[pre_build]]` steps. PebbleDB support requires no new Rust
  code paths.
- **Recipe module** (`src/recipe/`) -- flavor resolution, validation, and
  compatibility rules already support `db_backend` as an opaque dimension.

### How PebbleDB maps to existing abstractions

| Concern | Dockermint abstraction | Recipe TOML location |
| :------ | :--------------------- | :------------------- |
| Flavor declaration | `[flavours.available]` | `db_backend = ["goleveldb", "pebbledb"]` |
| Default value | `[flavours.default]` | `db_backend = "goleveldb"` |
| Build tag injection | `build_tags` flavor array | `build_tags = ["netgo", "ledger", "pebbledb"]` when selected |
| Linker variable | `[build.linker.variables]` | `"github.com/cosmos/cosmos-sdk/types.DBBackend" = "{{db_backend}}"` |
| Source patching | `[[pre_build]]` with condition | `condition = "pebbledb"` triggers `go mod edit -replace` |
| Image tag differentiation | `[image] tag` template | Tag includes `{{db_backend}}` for disambiguation |

### Native support detection

The Go builder does not perform runtime detection of `cometbft-db` versions.
Instead, the recipe author encodes knowledge of native support into the recipe:

- **Chains with native support** (cometbft-db >= v0.10.0): the recipe omits
  `[[pre_build]]` patching steps for the `pebbledb` condition. Only build tags
  and linker flags are needed.
- **Chains without native support** (cometbft-db < v0.10.0 or legacy tm-db):
  the recipe includes `[[pre_build]]` steps conditioned on `pebbledb` that emit
  the `go mod edit -replace` directives and `go mod tidy`.

This keeps detection logic in the recipe (data) rather than in Rust code,
consistent with the project philosophy.

### Patching strategy (pre-build steps)

For older chains, the recipe includes conditional pre-build steps:

```toml
# Applied ONLY when db_backend = "pebbledb" is selected
[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod edit -replace github.com/cometbft/cometbft-db=github.com/notional-labs/cometbft-db@pebble"

[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod edit -replace github.com/tendermint/tm-db=github.com/notional-labs/tm-db@v0.6.8-pebble"

[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod edit -replace github.com/cosmos/cosmos-db=github.com/notional-labs/cosmos-db@pebble"

[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod tidy"
```

Notes on the patching directives:

- The three `go mod edit -replace` commands are independent. A recipe should
  only include the ones that match modules actually imported by the chain.
  Determining which modules a chain imports is the recipe author's
  responsibility (typically checked via `grep -E 'cometbft-db|tm-db|cosmos-db'
  go.mod`).
- The `@pebble` and `@v0.6.8-pebble` identifiers refer to branches and tags
  maintained by the `notional-labs` GitHub organization
  (https://github.com/notional-labs).
- For reproducibility or to pick up a specific fix, the recipe author may
  replace `@pebble` with an explicit commit hash or tag (e.g.,
  `@v0.11.0-pebble-1`).
- The `notional-labs` forks are community maintained. Multiple branches and
  tags exist that are not always mutually compatible with every chain version.
  If `go mod tidy` or the build fails, the recipe author should try alternative
  tags until a combination compiles cleanly.

The Go builder emits these `[[pre_build]]` steps into the Dockerfile builder
stage exactly as documented in `docs/specs/builder.md` (Subsystem 3,
"Conditional pre-build steps"), before the `go build` command.

### Build-time flags

When `db_backend = "pebbledb"` is the resolved flavor, the Go builder
constructs the `go build` command with:

1. **Build tags**: the `pebbledb` tag is included in the `-tags` flag.
   Typically combined with other tags the chain requires (e.g., `ledger`,
   `netgo`, `muslc`). The `build_tags` flavor array in the recipe controls
   this. When `pebbledb` is selected as `db_backend`, the recipe's conditional
   logic (or the user's `build_tags` selection) ensures `pebbledb` is present
   in the tags list.

2. **Linker variable**: the linker variable
   `-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb` is set via
   `[build.linker.variables]` in the recipe. The template engine resolves
   `{{db_backend}}` to `"pebbledb"`, which gets interpolated into the ldflags
   string.

A minimal build command:

```bash
go build \
    -tags 'ledger,pebbledb' \
    -ldflags "-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb" \
    -o /go/bin/<daemon> \
    ./cmd/<daemon>
```

A complete release build:

```bash
go build \
    -mod=readonly \
    -trimpath \
    -tags 'ledger,pebbledb' \
    -ldflags "\
        -X github.com/cosmos/cosmos-sdk/version.Name=<chain-name> \
        -X github.com/cosmos/cosmos-sdk/version.AppName=<daemon-name> \
        -X github.com/cosmos/cosmos-sdk/version.Version=<git-tag> \
        -X github.com/cosmos/cosmos-sdk/version.Commit=<git-commit> \
        -X 'github.com/cosmos/cosmos-sdk/version.BuildTags=ledger,pebbledb' \
        -X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb" \
    -o /usr/local/bin/<daemon-name> \
    ./cmd/<daemon-name>
```

### Recipe integration

#### Declaring PebbleDB as a flavor

In a recipe TOML, PebbleDB availability is declared as:

```toml
[flavours.available]
db_backend = ["goleveldb", "pebbledb"]

[flavours.default]
db_backend = "goleveldb"
```

#### Linker variable wiring

The recipe's `[build.linker.variables]` section uses the `{{db_backend}}`
template variable so that the correct backend is baked into the binary
regardless of which `db_backend` flavor is selected:

```toml
[build.linker.variables]
"github.com/cosmos/cosmos-sdk/types.DBBackend" = "{{db_backend}}"
```

When `db_backend = "goleveldb"`, the linker variable resolves to `goleveldb`.
When `db_backend = "pebbledb"`, it resolves to `pebbledb`. No conditional
logic in Rust is needed.

#### Build tag inclusion

The `pebbledb` build tag can be handled in two ways depending on recipe design:

1. **Always in the tag list**: the recipe lists `pebbledb` in the default
   `build_tags` array and relies on Go's compiler to ignore unused build-tag
   guarded code when the corresponding module is not replaced.
2. **Conditional via flavor compatibility rules**: the recipe uses
   `[[flavours.incompatible]]` rules to enforce that `pebbledb` is in
   `build_tags` when `db_backend = "pebbledb"`, and the user must include it
   explicitly.

Approach (1) is simpler. The recipe author chooses based on chain requirements.

#### Image tag differentiation

The recipe `[image] tag` template typically includes `{{db_backend}}` so that
images built with different backends produce distinct tags:

```toml
[image]
tag = "chain-binary-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"
```

### Verification

After a PebbleDB build, the resulting binary can be verified:

```bash
<daemon-name> version --long | grep -i build_tags
```

The output should contain `pebbledb` in the build tags list (e.g.,
`ledger,pebbledb`).

### On-disk incompatibility

PebbleDB and goleveldb are NOT on-disk compatible. Existing nodes switching
from goleveldb to pebbledb must either:

1. Re-sync from genesis or a recent block height.
2. Import a PebbleDB-format snapshot.
3. Convert the existing goleveldb database using
   [Pebblify](https://github.com/Dockermint/pebblify).

This is an operational concern for node operators. Dockermint does not perform
database conversion -- it only produces the binary with the correct backend
compiled in. However, recipe documentation or image labels may note this
incompatibility.

### Error scenarios

| Scenario | Cause | Handling |
| :------- | :---- | :------- |
| `pebbledb` selected but not in `[flavours.available]` | User selected unavailable flavor | `RecipeError::InvalidFlavorValue` (recipe module) |
| `pebbledb` build tag missing from `build_tags` | Misconfigured recipe or user override | Build may succeed but binary will not include PebbleDB code paths. If `[[flavours.incompatible]]` rules enforce tag presence, caught as `RecipeError::IncompatibleFlavors`. |
| `go mod edit -replace` fails (wrong fork tag) | `notional-labs` fork tag incompatible with chain version | `BuilderError::BuildFailed` -- build stage command exits non-zero. Error message includes `go mod` output. Recipe author should try alternative fork tags. |
| `go mod tidy` fails after replace | Dependency graph cannot be resolved with the selected fork | `BuilderError::BuildFailed` -- same as above. Indicates fork tag / chain version mismatch. |
| `go build` fails with PebbleDB tag | Code-level incompatibility between fork and chain | `BuilderError::BuildFailed`. May indicate need for a different fork tag or that the chain version is too old for PebbleDB support. |

### Dependencies

No new Rust crate dependencies. PebbleDB support is entirely recipe-driven,
using existing builder and recipe module capabilities.

---

## Recipe example (complete)

A chain with native PebbleDB support (cometbft-db >= v0.10.0):

```toml
[flavours.available]
db_backend = ["goleveldb", "pebbledb"]

[flavours.default]
db_backend = "goleveldb"

[build.linker.variables]
"github.com/cosmos/cosmos-sdk/types.DBBackend" = "{{db_backend}}"

# No [[pre_build]] patching needed -- native support
```

A chain without native PebbleDB support (older cometbft-db or legacy tm-db):

```toml
[flavours.available]
db_backend = ["goleveldb", "pebbledb"]

[flavours.default]
db_backend = "goleveldb"

# Patching for PebbleDB on older chains
[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod edit -replace github.com/cometbft/cometbft-db=github.com/notional-labs/cometbft-db@pebble"

[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod edit -replace github.com/tendermint/tm-db=github.com/notional-labs/tm-db@v0.6.8-pebble"

[[pre_build]]
condition = "pebbledb"
instruction = "RUN"
command = "go mod tidy"

[build.linker.variables]
"github.com/cosmos/cosmos-sdk/types.DBBackend" = "{{db_backend}}"
```

---

## Interaction with other specs

| Spec | Relationship |
| :--- | :----------- |
| `docs/specs/builder.md` | PebbleDB uses the Go builder's existing Dockerfile generation flow: conditional `[[pre_build]]` steps, build tag assembly, linker variable resolution. No new builder code paths needed. |
| `docs/specs/recipe.md` | PebbleDB is a value of the `db_backend` flavor dimension. Flavor resolution, validation, and compatibility rules apply normally. |
| `docs/specs/architecture-overview.md` | Confirms the design principle: new flavor values require no Rust code changes, only recipe TOML edits. |

---

## Testing strategy

PebbleDB support does not introduce new Rust code, so testing applies to the
recipe and builder modules that already handle it:

- **Unit tests (recipe module)**: Verify that a recipe with `db_backend =
  ["goleveldb", "pebbledb"]` parses correctly. Verify flavor resolution
  selects `pebbledb` when requested via CLI or config.toml. Verify
  `InvalidFlavorValue` when `pebbledb` is selected but not in available list.
- **Unit tests (Go builder)**: Verify that when `db_backend = "pebbledb"` is
  resolved, the generated Dockerfile includes the `pebbledb` build tag in
  `-tags` and the `types.DBBackend=pebbledb` linker variable in `-ldflags`.
- **Unit tests (Go builder, pre_build)**: Verify that `[[pre_build]]` steps
  with `condition = "pebbledb"` are emitted into the Dockerfile when
  `db_backend = "pebbledb"` and omitted when `db_backend = "goleveldb"`.
- **Integration tests**: End-to-end Dockerfile generation for a recipe with
  PebbleDB flavor, confirming the complete Dockerfile includes patching steps,
  correct build tags, and correct linker flags.

---

## Summary

1. Native PebbleDB support starts at `cometbft-db v0.10.0` (CometBFT v0.38 /
   Cosmos SDK v0.50).
2. Older chains must be patched via `go mod edit -replace` with
   `notional-labs` forks, followed by `go mod tidy`.
3. PebbleDB is enabled at build time by adding the `pebbledb` build tag and
   setting `types.DBBackend=pebbledb` via `-ldflags`.
4. In Dockermint, all of the above is driven by recipe TOML: flavor
   declaration, conditional pre-build patching, linker variable templates,
   and build tag inclusion. No Rust code changes are required.

---

## Open questions

None. This spec is a reference document describing how existing Dockermint
abstractions handle PebbleDB. All design decisions are inherited from the
builder and recipe specs.
