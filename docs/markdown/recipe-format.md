# Recipe Format Reference

A Dockermint **recipe** is a TOML file that fully specifies how to build a
Docker image for a single blockchain binary. No Rust code changes are needed
to add a new chain — only a new recipe file.

Recipes live in the `recipes/` directory by default (configurable via
`config.toml`).

---

## Variable Reference

Recipe strings support two classes of template variables, distinguished by
naming convention. Both use `{{name}}` syntax but are resolved at different
times.

### Host-time variables (`{{UPPERCASE}}`)

Resolved at Dockermint startup by `recipe::host_vars::collect` before any
Docker build begins. These values come from the local system environment and
the tag being built.

| Variable              | Value                                                         |
| :-------------------- | :------------------------------------------------------------ |
| `{{HOST_ARCH}}`       | Host CPU architecture (`x86_64` or `aarch64`)                |
| `{{CREATION_TIMESTAMP}}` | UTC ISO 8601 build timestamp (`YYYY-MM-DDTHH:MM:SSZ`)     |
| `{{SEMVER_TAG}}`      | The Git tag being built (e.g. `v21.0.1`)                     |
| `{{BUILD_TAGS_COMMA_SEP}}` | `build_tags` flavor values joined with commas           |
| `{{repository_path}}` | Fixed as `/workspace` — the default clone destination        |

Additional process environment variables (e.g. `{{GH_USER}}`, `{{GH_PAT}}`)
can be forwarded by `host_vars::extend_from_env` and are then available as
`ARG` declarations in the Dockerfile.

Flavor default values may reference host-time variables. When
`architecture = "{{HOST_ARCH}}"` is the default, the `architecture` variable
in `resolved_variables` is already set to `"x86_64"` (or `"aarch64"`) before
the builder runs.

### Build-time variables (`{{lowercase}}`)

Defined in `[variables]` as shell commands. These are **not** resolved by
Dockermint — they become shell variable assignments inside the Dockerfile
`RUN` instruction and are resolved by the container shell at build time.

```toml
[variables]
repo_version = { shell = "git describe --exact-match 2>/dev/null || ..." }
repo_commit  = { shell = "git log -1 --format='%H'" }
```

In linker variable templates, Dockermint first expands any host-time
references with `TemplateEngine::render`, then converts surviving
`{{lowercase}}` placeholders to `$name` for shell interpolation:

```toml
# In recipe:
"github.com/.../version.Version" = "{{repo_version}}"

# In generated Dockerfile RUN script:
# -X 'github.com/.../version.Version=$repo_version'
```

### Profile variables

Profile variables (from `[profiles]`) are injected into the resolved variable
map at parse time alongside host-time variables. They can be referenced as
`{{lowercase}}` in linker variable templates and other recipe strings.

---

## Schema Version

Every recipe must declare a schema version. The current maximum is `1`.

```toml
[meta]
schema_version = 1
min_dockermint_version = "0.1.0"
```

| Field                   | Type   | Description                                    |
| :---------------------- | :----- | :--------------------------------------------- |
| `schema_version`        | `u32`  | Recipe file schema version. Must be `<= 1`.    |
| `min_dockermint_version`| string | Minimum Dockermint release required to parse.  |

---

## `[header]`

Project identity for the recipe.

```toml
[header]
name = "Cosmos"
repo = "https://github.com/cosmos/gaia"
type = "golang"
binary_name = "gaiad"
include_patterns = ""
exclude_patterns = ""
```

| Field              | Type   | Required | Description                                               |
| :----------------- | :----- | :------- | :-------------------------------------------------------- |
| `name`             | string | yes      | Human-readable project name                               |
| `repo`             | string | yes      | Full git repository URL                                   |
| `type`             | string | yes      | Build system type (e.g. `"golang"`)                       |
| `binary_name`      | string | yes      | Produced binary name (also available as `{{binary_name}}`)|
| `include_patterns` | string | no       | Comma-separated glob patterns — only matching tags built  |
| `exclude_patterns` | string | no       | Comma-separated glob patterns — matching tags skipped     |

---

## `[flavours]`

Declares the available build dimensions and their defaults.

```toml
[flavours.available]
architecture = ["x86_64", "aarch64"]
db_backend   = ["goleveldb", "pebbledb"]
binary_type  = ["dynamic", "static"]
running_env  = ["alpine3.23", "bookworm", "distroless"]
running_user = ["root", "custom", "dockermint"]
build_tags   = ["netgo", "ledger", "muslc"]

[flavours.default]
architecture = "{{HOST_ARCH}}"
db_backend   = "goleveldb"
binary_type  = "static"
running_env  = "alpine3.23"
running_user = "root"
build_tags   = ["netgo", "muslc"]
```

**`flavours.available`** — maps each dimension name to its allowed values.
Any selection not in this list causes an `IncompatibleFlavour` error.

**`flavours.default`** — maps each dimension to its default selection. Defaults
are overridden by `config.toml` overrides and then by CLI `--flavor` arguments.

A default value can be a single string or an array:

```toml
db_backend  = "goleveldb"           # single selection
build_tags  = ["netgo", "muslc"]    # multi-value selection
```

Template variables like `{{HOST_ARCH}}` are allowed in defaults and are
resolved at startup. They skip validation against `available` lists.

---

## `[scrapper]`

Defines the Docker image and method used to clone the source repository.

```toml
[scrapper]
image     = "golang:1.23-alpine3.21"
install   = "apk add --no-cache git"
env       = ["{{GH_USER}}", "{{GH_PAT}}"]
method    = "try-authenticated-clone"
directory = "{{repository_path}}"
```

| Field       | Type       | Required | Description                                              |
| :---------- | :--------- | :------- | :------------------------------------------------------- |
| `image`     | string     | yes      | Docker image for the source-cloning stage; also used to auto-detect the builder install command |
| `install`   | string     | no       | Shell command to install deps inside the scrapper image  |
| `env`       | `[string]` | no       | Environment variable templates forwarded as `ARG` in the Dockerfile (e.g. `"{{GH_USER}}"`) |
| `method`    | string     | yes      | Clone strategy; `"try-authenticated-clone"` attempts authenticated clone then falls back to public |
| `directory` | string     | yes      | Working directory inside the container; `{{repository_path}}` resolves to `/workspace` |

---

## `[variables]`

Shell commands whose stdout is captured as named build variables. These
variables can then be referenced as `{{variable_name}}` in any recipe string.

```toml
[variables]
repo_commit      = { shell = "git log -1 --format='%H'" }
repo_version     = { shell = "git describe --exact-match 2>/dev/null || echo \"$(git rev-parse --abbrev-ref HEAD)-$(git log -1 --format='%H')\"" }
golang_version   = { shell = "grep -E '^(toolchain|go) ' go.mod | head -1 | sed 's/^toolchain go//;s/^go //'" }
wasmvm_version   = { shell = "go list -m github.com/CosmWasm/wasmvm/v2 2>/dev/null | sed 's:.* ::'" }
cometbft_version = { shell = "go list -m github.com/cometbft/cometbft | sed 's:.* ::'" }
```

Each entry is a TOML inline table with a single `shell` key. The command runs
inside the build context and its trimmed stdout becomes the variable value.

---

## `[builder]`

Platform-specific package installation for the builder stage.

```toml
[builder.install]
alpine = "apk add --no-cache alpine-sdk linux-headers"
ubuntu = "apt-get update && apt-get install -y --no-install-recommends make gcc build-essential linux-headers-generic"
```

Keys are distribution family names. The Dockerfile generator selects the
appropriate command automatically by matching the **longest** key that appears
as a substring of the scrapper image name. For example, if the scrapper image
is `golang:1.23-alpine3.21`, the key `alpine` matches and its command is
appended (with `&&`) to the scrapper `install` command in the same `RUN`
instruction. Adding a new entry (e.g. `fedora = "dnf install ..."`) is
sufficient — no Rust code change is needed.

---

## `[[pre_build]]`

An array of conditional Dockerfile instructions executed before the main build
step. Each entry is included in the Dockerfile when
`SelectedFlavours::has_value(step.condition)` returns `true`, meaning the
condition string matches any active single-value flavor selection or appears
in any active multi-value selection.

```toml
[[pre_build]]
condition   = "static"
instruction = "ADD"
source      = "https://github.com/CosmWasm/wasmvm/releases/download/{{wasmvm_version}}/libwasmvm_muslc.{{HOST_ARCH}}.a"
dest        = "/lib/libwasmvm_muslc.{{HOST_ARCH}}.a"
```

The example above fires when `binary_type = "static"` is active. `{{wasmvm_version}}` in `source` is a build-time variable (resolved by the shell inside the container); `{{HOST_ARCH}}` is a host-time variable (expanded inline by Dockermint before the Dockerfile is written).

| Field         | Type   | Required | Description                                          |
| :------------ | :----- | :------- | :--------------------------------------------------- |
| `condition`   | string | yes      | Flavor value that activates this step; evaluated with `has_value` |
| `instruction` | string | yes      | Dockerfile instruction (`ADD`, `RUN`, `COPY`, etc.)  |
| `source`      | string | no       | Source argument (URL, path, or template string)      |
| `dest`        | string | no       | Destination path inside the image                    |

Supported instructions and how they map to Dockerfile output:

| `instruction` | Generated Dockerfile line                     |
| :------------ | :-------------------------------------------- |
| `ADD`         | `ADD <source> <dest>`                         |
| `RUN`         | `RUN <source>`                                |
| `COPY`        | `COPY --from=builder <source> <dest>`         |
| other         | `<INSTRUCTION> <source> <dest>` (raw passthrough) |

---

## `[build]`

Build environment, linker configuration, and build path.

### `[build.env]`

Environment variables set during compilation.

```toml
[build.env]
CGO_ENABLED = "1"
```

### `[build.linker.flags]`

Maps `binary_type` flavor values to their linker flag strings.

```toml
[build.linker.flags]
dynamic = "-w -s -extldflags '-z noexecstack'"
static  = "-linkmode=external -w -s -extldflags '-Wl,-z,muldefs -static -z noexecstack'"
```

### `[build.linker.variables]`

Maps Go import paths to values embedded via `-X`. Template variables are
expanded before the flags are passed to `go build`.

```toml
[build.linker.variables]
"github.com/cosmos/cosmos-sdk/version.Name"    = "gaia"
"github.com/cosmos/cosmos-sdk/version.Version" = "{{repo_version}}"
"github.com/cosmos/cosmos-sdk/version.Commit"  = "{{repo_commit}}"
```

### `[build.path]`

Path to the Go package to build. Template variables are supported.

```toml
[build.path]
path = "{{repository_path}}/cmd/gaiad"
```

---

## `[user]`

Per-user-type configuration, keyed by user type name. Used when
`running_user = "dockermint"` (or another custom user type).

```toml
[user.dockermint]
username = "dockermint"
uid      = 10000
gid      = 10000
```

| Field      | Type   | Description                        |
| :--------- | :----- | :--------------------------------- |
| `username` | string | Username inside the container      |
| `uid`      | `u32`  | Numeric user ID                    |
| `gid`      | `u32`  | Numeric group ID                   |

---

## `[copy]`

Specifies files to copy from the builder stage to the runner stage.

Top-level entries are **always** copied:

```toml
[copy]
"/go/bin/gaiad" = { dest = "/usr/bin/{{binary_name}}", type = "entrypoint" }
```

Sub-tables keyed by a `binary_type` flavor value are **conditional** — they
are only included when that flavor is active:

```toml
[copy.dynamic]
"/go/pkg/mod/github.com/!cosm!wasm/wasmvm/v2@{{wasmvm_version}}/internal/api/libwasmvm.{{HOST_ARCH}}.so" = { dest = "/lib/libwasmvm.{{HOST_ARCH}}.so", type = "dyn-library" }
```

Each entry value is an inline table:

| Field  | Type   | Description                                      |
| :----- | :----- | :----------------------------------------------- |
| `dest` | string | Destination path in the runner image             |
| `type` | string | Artifact kind: `"entrypoint"`, `"dyn-library"`, etc. |

---

## `[expose]`

Ports to declare with `EXPOSE` in the final image.

```toml
[expose]
ports = [
    { port = 26656, description = "P2P" },
    { port = 26657, description = "RPC" },
    { port = 26660, description = "Prometheus metrics" },
    { port = 1317,  description = "REST API" },
    { port = 9090,  description = "gRPC" },
    { port = 9091,  description = "gRPC-Web" },
]
```

| Field         | Type   | Description                  |
| :------------ | :----- | :--------------------------- |
| `port`        | `u16`  | Port number                  |
| `description` | string | Human-readable purpose       |

---

## `[labels]`

OCI image labels applied to the final image. Template variables are supported.

```toml
[labels]
"org.opencontainers.image.created"       = "{{CREATION_TIMESTAMP}}"
"org.opencontainers.image.version"       = "{{repo_version}}"
"org.opencontainers.image.revision"      = "{{repo_commit}}"
"org.opencontainers.image.title"         = "Gaiad"
"org.opencontainers.image.description"   = "Cosmos Hub Node"
"org.opencontainers.image.licenses"      = "Apache-2.0"
```

---

## `[image]`

The Docker image tag template. All resolved variables are available.

```toml
[image]
tag = "cosmos-gaiad-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"
```

`{{SEMVER_TAG}}` is the Git tag being built (e.g. `v21.0.1`). Because
`running_env` is injected as a variable during `recipe::resolve`, it can be
used in the tag template directly.

---

## `running_env` to Docker Image Mapping

The `running_env` flavor value is converted to a Docker image reference by
`running_env_to_image` in the Dockerfile generator. The conversion rules are:

| `running_env` value | Runner `FROM` image                    |
| :------------------ | :------------------------------------- |
| `alpine3.23`        | `alpine:3.23`                          |
| `ubuntu24.04`       | `ubuntu:24.04`                         |
| `bookworm`          | `debian:bookworm-slim`                 |
| `distroless`        | `gcr.io/distroless/static-debian12`    |
| `ghcr.io/foo:tag`   | `ghcr.io/foo:tag` (passed through)     |

The general rule: if the value contains a digit, split at the first digit
boundary and insert `:`. Named values (`bookworm`, `distroless`) have explicit
mappings. Full image references (containing `/` or `:`) pass through unchanged.

Note: user creation commands (`adduser`/`useradd`) are distro-aware. Distroless
images have no shell, so non-root user configuration is skipped for them.

---

## `[profiles]`

Optional profile tables that inject additional variables based on a flavor
selection. The structure is:

```toml
[profiles.<dimension>.<value>]
key = "value"
```

When the named dimension resolves to the named value, all keys in that profile
table are added to the resolved variable map.

```toml
[profiles.network.mainnet]
denom                    = "ukyve"
team_foundation_address  = "kyve1xjpl57p7f49y5gueu7rlfytaw9ramcn5zhjy2g"
team_allocation          = "165000000000000"

[profiles.network.kaon]
denom                    = "tkyve"
team_foundation_address  = "kyve1vut528et85755xsncjwl6dx8xakuv26hxgyv0n"
team_allocation          = "165000000000000"
```

Profile variables can then be referenced in `[build.linker.variables]` or
other template strings:

```toml
"github.com/KYVENetwork/chain/x/global/types.Denom" = "{{denom}}"
```

---

## Complete Example: `cosmos-gaiad.toml`

See [`recipes/cosmos-gaiad.toml`](../../recipes/cosmos-gaiad.toml) for the
full Cosmos Hub recipe, and [`recipes/kyve-kyved.toml`](../../recipes/kyve-kyved.toml)
for a recipe that uses the `[profiles]` feature.

### Image tag produced by cosmos-gaiad

With default flavors:

```
cosmos-gaiad-goleveldb:v21.0.1-alpine3.23
```

With `--flavor db_backend=pebbledb --flavor running_env=bookworm`:

```
cosmos-gaiad-pebbledb:v21.0.1-bookworm
```
