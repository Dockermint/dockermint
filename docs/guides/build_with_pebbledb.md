# Build with PebbleDB

This guide explains how a Cosmos SDK chain gains PebbleDB support: when it is
available natively, how to patch older chains that do not support it out of the
box, and how to enable it at build time. It targets two audiences:

- **Node operators** who want to understand what Dockermint does under the
  hood when they select the `db_backend = "pebbledb"` flavour in a recipe.
- **Dockermint contributors** working on the `builder` module or on new
  recipes that need to expose PebbleDB as a flavour.

## 1. Native support

PebbleDB is supported natively starting with
`github.com/cometbft/cometbft-db` **v0.10.0**.

In practice, this corresponds to chains built on top of:

- CometBFT **v0.38.0** or newer, and
- Cosmos SDK **v0.50** or newer.

Any chain whose `go.mod` already pins `cometbft-db >= v0.10.0` can be compiled
with PebbleDB without any source-level patching: only the appropriate build
tags and `ldflags` are required (see section 3).

Any chain pinning an older version of `cometbft-db`, or still relying on the
legacy `github.com/tendermint/tm-db`, does **not** support PebbleDB natively
and must be patched before compilation (see section 2).

## 2. Patching older chains

For chains that predate `cometbft-db v0.10.0`, PebbleDB support is available
through community-maintained forks published by `notional-labs`. These forks
are drop-in replacements, injected via `go mod edit -replace`.

Run the following commands from the chain's Go module root (the directory
containing `go.mod`) **before** running `go build`:

```bash
# Always required for chains below cometbft-db v0.10.0
go mod edit -replace \
    github.com/cometbft/cometbft-db=github.com/notional-labs/cometbft-db@pebble

# Required only if the chain still imports the legacy tm-db
go mod edit -replace \
    github.com/tendermint/tm-db=github.com/notional-labs/tm-db@v0.6.8-pebble

# Required only if the chain imports github.com/cosmos/cosmos-db
go mod edit -replace \
    github.com/cosmos/cosmos-db=github.com/notional-labs/cosmos-db@pebble

# Resolve the new dependency graph
go mod tidy
```

Notes:

- The three `replace` directives are independent. Apply only the ones that
  match modules actually imported by the chain. A quick check:

  ```bash
  grep -E 'cometbft-db|tm-db|cosmos-db' go.mod
  ```

- The `@pebble` and `@v0.6.8-pebble` identifiers are the branches and tags
  maintained by `notional-labs` that carry the PebbleDB backend.
- If a specific revision is required (for reproducibility or to pick up a
  fix), replace `@pebble` with an explicit commit hash or tag, for example
  `@v0.11.0-pebble-1`.
- Expect some trial and error. The `notional-labs` forks are community
  maintained and ship several branches and tags that are not always mutually
  compatible with every chain version. If `go mod tidy` or the build fails
  with resolution or API errors, try another tag (for example swap `@pebble`
  for `@v0.11.0-pebble-1`, `@v0.9.1-pebble`, or a specific commit hash) until
  a combination compiles cleanly against the target chain.
- All of these forks live under the `notional-labs` GitHub organization at
  <https://github.com/notional-labs>. Browse the repositories directly to
  discover the available branches and tags for each module.

Once the `replace` directives are in place and `go mod tidy` has succeeded,
the chain can be compiled exactly as a natively-supported chain.

## 3. Enabling PebbleDB at build time

PebbleDB is gated behind a build tag. It must be enabled explicitly by
passing both a build tag and an `ldflag` to `go build`:

```bash
go build \
    -tags 'ledger,pebbledb' \
    -ldflags "-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb" \
    -o /path/to/output/binary \
    ./cmd/<chain-daemon>
```

What each flag does:

- `-tags 'ledger,pebbledb'` compiles the PebbleDB code path into the
  binary. The `ledger` tag is kept to preserve hardware wallet support; add
  any other tags the chain normally requires.
- `-ldflags "-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb"` sets
  PebbleDB as the default backend baked into the binary, so a freshly
  initialised node uses PebbleDB without any additional configuration.

A more complete invocation, mirroring what is typically wanted for a release
build, looks like:

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

After installing the resulting binary, confirm that PebbleDB is compiled in:

```bash
<daemon-name> version --long | grep -i build_tags
```

The output should contain `ledger,pebbledb`. Existing nodes that are
switching from `goleveldb` must also update their `config.toml`
(`db_backend = "pebbledb"`) and either re-sync, import a PebbleDB snapshot,
or convert an existing `goleveldb` database with
[Pebblify](https://github.com/Dockermint/pebblify), since the two backends
are not on-disk compatible.

## 4. Mapping to Dockermint

All the steps above are performed by Dockermint's `builder` module whenever
a recipe exposes `pebbledb` in its `[flavours.available]` section and the
user selects it, either through CLI arguments, `config.toml`, or the recipe
defaults:

```toml
[flavours.available]
db_backend = ["goleveldb", "pebbledb"]

[flavours.default]
db_backend = "goleveldb"
```

At build time, Dockermint resolves the selected `db_backend` flavour,
injects the required `go mod edit -replace` directives for chains that
predate native support, and forwards the `pebbledb` build tag along with the
`-X github.com/cosmos/cosmos-sdk/types.DBBackend=pebbledb` linker variable
through the recipe's `[build.linker.flags]` and `[build.linker.variables]`
tables.

## Summary

1. Native support starts at `cometbft-db v0.10.0`
   (CometBFT v0.38 / Cosmos SDK v0.50).
2. Older chains must inject the `notional-labs` forks through
   `go mod edit -replace` followed by `go mod tidy`.
3. Enabling the backend is done at build time by adding the `pebbledb` build
   tag and setting `types.DBBackend=pebbledb` via `-ldflags`.
4. In Dockermint, all of the above is driven by selecting the `pebbledb`
   value of the `db_backend` flavour in a recipe.
