# Configuration Reference

Dockermint is configured through two inputs:

- **`config.toml`** — all non-secret settings (paths, intervals, overrides).
- **`.env`** — secrets (API tokens, credentials). Never committed to version
  control.

CLI arguments override values from `config.toml` for the fields they share.

---

## Locating `config.toml`

```bash
# Default (current working directory):
dockermint build --recipe recipes/cosmos-gaiad.toml --tag v21.0.1

# Explicit path via flag:
dockermint --config /etc/dockermint/config.toml build ...

# Explicit path via environment variable:
DOCKERMINT_CONFIG=/etc/dockermint/config.toml dockermint build ...
```

If `--config` is absent and `DOCKERMINT_CONFIG` is unset, Dockermint runs
without a config file — all fields have built-in defaults and no file is
required.

---

## Full `config.toml` Reference

```toml
version     = 1
recipes_dir = "recipes"

[log]
level       = "info"
directory   = "/var/log/dockermint"
file_prefix = "dockermint"
json        = false

[daemon]
poll_interval_secs   = 60
max_builds_per_cycle = 1

[rpc]
bind = "127.0.0.1:9100"

[docker]
socket_uri     = "unix:///var/run/docker.sock"
builder_prefix = "dockermint"

[database]
path = "data/dockermint.redb"

[notifier]
enabled = false

[vcs]
max_concurrent_requests = 4

[registry]
url = "ghcr.io"

[metrics]
enabled = false
bind    = "127.0.0.1:9200"

[flavours.cosmos-gaiad]
db_backend = "pebbledb"

[flavours.kyve-kyved]
build_tags = ["netgo", "muslc", "ledger"]
network    = "mainnet"
```

---

## Top-Level Fields

| Field         | Type   | Default     | Description                                  |
| :------------ | :----- | :---------- | :------------------------------------------- |
| `version`     | `u32`  | `1`         | Config schema version. Must equal `1`.       |
| `recipes_dir` | path   | `"recipes"` | Directory scanned for `.toml` recipe files   |

---

## `[log]`

| Field         | Type   | Default        | Description                                                        |
| :------------ | :----- | :------------- | :----------------------------------------------------------------- |
| `level`       | string | `"info"`       | Minimum log level: `trace`, `debug`, `info`, `warn`, `error`       |
| `directory`   | path   | absent         | If set, rotate daily log files here; otherwise all output to stdout|
| `file_prefix` | string | `"dockermint"` | Log file name prefix (e.g. `dockermint.2026-04-12`)                |
| `json`        | bool   | `false`        | Emit structured JSON logs (useful for log aggregators)             |

The `RUST_LOG` environment variable overrides `level` when present (standard
`tracing_subscriber` behaviour).

---

## `[daemon]`

Only relevant when running `dockermint daemon`. The section is optional — when
absent the daemon uses the defaults shown below.

| Field                  | Type  | Default | Validation  | Description                                     |
| :--------------------- | :---- | :------ | :---------- | :---------------------------------------------- |
| `poll_interval_secs`   | `u64` | `60`    | must be > 0 | Seconds between VCS polling cycles              |
| `max_builds_per_cycle` | `u32` | `1`     | must be > 0 | Maximum tags built per recipe per polling cycle |

CLI overrides take precedence over these values:

```bash
dockermint daemon --poll-interval 30 --max-builds 2
```

---

## `[rpc]`

Controls the optional HTTP server that can run alongside the daemon. The server
is only started when `dockermint daemon --rpc` is passed. The `[rpc]` section
is optional.

| Field  | Type        | Default          | Description                          |
| :----- | :---------- | :--------------- | :----------------------------------- |
| `bind` | socket addr | `127.0.0.1:9100` | Address and port for the RPC server  |

CLI override:

```bash
# Enable RPC and override the bind address:
dockermint daemon --rpc --rpc-bind 0.0.0.0:9100

# Enable RPC using the address from config.toml [rpc]:
dockermint daemon --rpc
```

The `--rpc-bind` CLI flag always takes precedence over the `[rpc].bind`
config value when `--rpc` is active.

---

## `[docker]`

Controls the Docker daemon connection and the names of buildx builder instances
created by `BuildKitBuilder`.

| Field            | Type   | Default                          | Description                                                         |
| :--------------- | :----- | :------------------------------- | :------------------------------------------------------------------ |
| `socket_uri`     | string | `"unix:///var/run/docker.sock"`  | Docker daemon socket URI. Use `tcp://host:port` for remote daemons. |
| `builder_prefix` | string | `"dockermint"`                   | Prefix for buildx builder names. Builders are `{prefix}-amd64` and `{prefix}-arm64`. |

`socket_uri` is used in two places:

1. **System requirements check** (`checker::verify_requirements`) — at startup,
   `docker version` and `docker buildx version` are probed via
   `DOCKER_HOST=<socket_uri>`. This means a misconfigured `socket_uri` is
   caught immediately, before any build is attempted, and the error message
   includes the URI to make remote daemon issues obvious.
2. **Build execution** (`BuildKitBuilder`) — all `docker buildx` commands during
   setup, build, and cleanup are routed through the same URI.

Example — remote Docker daemon:

```toml
[docker]
socket_uri     = "tcp://192.168.1.10:2376"
builder_prefix = "myproject"
```

This creates builders named `myproject-amd64` and `myproject-arm64` on the
remote daemon. The startup check confirms the remote daemon is reachable before
any build work begins.

---

## `[database]`

| Field  | Type | Default                  | Description                      |
| :----- | :--- | :----------------------- | :------------------------------- |
| `path` | path | `"data/dockermint.redb"` | Path to the redb database file   |

---

## `[notifier]`

| Field     | Type | Default | Description                          |
| :-------- | :--- | :------ | :----------------------------------- |
| `enabled` | bool | `false` | Whether to send build notifications  |

Notifier credentials are stored in `.env` — see the [Secrets](#env--secrets)
section below.

---

## `[vcs]`

| Field                    | Type  | Default | Description                            |
| :----------------------- | :---- | :------ | :------------------------------------- |
| `max_concurrent_requests`| `u32` | `4`     | Maximum parallel GitHub API requests   |

---

## `[registry]`

| Field | Type          | Default | Description                                         |
| :---- | :------------ | :------ | :-------------------------------------------------- |
| `url` | string (opt.) | absent  | Registry URL override. Absent or omitted = Docker Hub |

Registry credentials are stored in `.env`.

---

## `[metrics]`

| Field     | Type        | Default          | Description                            |
| :-------- | :---------- | :--------------- | :------------------------------------- |
| `enabled` | bool        | `false`          | Whether to start the metrics endpoint  |
| `bind`    | socket addr | `127.0.0.1:9200` | Bind address for the Prometheus scrape endpoint |

Scrape endpoint: `http://<bind>/metrics`

---

## `[flavours.<recipe-stem>]`

Per-recipe flavor overrides. The key is the recipe file stem (filename without
`.toml`). These values override recipe defaults but are themselves overridden
by `--flavor` CLI flags.

```toml
[flavours.cosmos-gaiad]
db_backend = "pebbledb"

[flavours.kyve-kyved]
build_tags = ["netgo", "muslc", "ledger"]
network    = "mainnet"
```

Multi-value overrides use TOML arrays:

```toml
[flavours.cosmos-gaiad]
build_tags = ["netgo", "muslc", "ledger"]
```

---

## `.env` — Secrets

**Never commit `.env` to version control.** Ensure it is listed in
`.gitignore`.

Secrets are loaded from `.env` in the working directory via `dotenvy`. Missing
variables silently become `None` — features that require them will fail only
when they try to use them.

A reference file is provided at `.env.example` in the project root.

| Variable            | Used by  | Description                                |
| :------------------ | :------- | :----------------------------------------- |
| `GH_USER`           | scrapper | GitHub username for authenticated clones   |
| `GH_PAT`            | scrapper | GitHub personal access token               |
| `TELEGRAM_TOKEN`    | notifier | Telegram Bot API token                     |
| `TELEGRAM_CHAT_ID`  | notifier | Telegram chat ID for notifications         |
| `REGISTRY_USER`     | push     | Container registry username                |
| `REGISTRY_PASSWORD` | push     | Container registry password or token       |

Example `.env`:

```bash
GH_USER=myuser
GH_PAT=ghp_xxxxxxxxxxxxxxxx
TELEGRAM_TOKEN=123456789:AAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
TELEGRAM_CHAT_ID=-1001234567890
REGISTRY_USER=myuser
REGISTRY_PASSWORD=mytoken
```

All secret values are stored as `SecretString` internally — they will never
appear in logs or error messages.

---

## Validation Rules

`config::validate` is called automatically by `config::load`. It rejects:

| Rule                               | Error message                                              |
| :--------------------------------- | :--------------------------------------------------------- |
| `version` != 1                     | `unsupported config version N, expected 1`                 |
| `recipes_dir` is absolute and does not exist | `recipes directory does not exist: <path>`     |
| `daemon.poll_interval_secs` == 0   | `daemon.poll_interval_secs must be > 0`                    |
| `daemon.max_builds_per_cycle` == 0 | `daemon.max_builds_per_cycle must be > 0`                  |

All failures return `ConfigError::Invalid` and follow the CLI unrecoverable
error strategy (log, dump, exit).

---

## Configuration Loading Pipeline

```
1. CLI parses arguments (clap)
2. config::load(path)  or  config::load_default()
      -> reads file, toml::from_str, validate()
3. config::load_secrets()
      -> dotenvy::dotenv(), reads env vars into Secrets
4. config::apply_daemon_overrides()   [daemon mode only]
      -> CLI flags overwrite daemon.poll_interval_secs,
         daemon.max_builds_per_cycle, rpc.bind
```

The `apply_daemon_overrides` function creates the `[daemon]` section with
defaults (`poll_interval_secs = 60`, `max_builds_per_cycle = 1`) if none was
present in `config.toml`, then applies any CLI overrides on top.

---

## CLI Argument Reference

### Global flags

| Flag             | Env var             | Description                              |
| :--------------- | :------------------ | :--------------------------------------- |
| `--config`, `-c` | `DOCKERMINT_CONFIG` | Path to `config.toml`                    |
| `--log-level`    | —                   | Override log level for this invocation   |

### `build` subcommand

```bash
dockermint build [OPTIONS]

Options:
  -r, --recipe <PATH>          Path to the recipe TOML file
  -t, --tag <TAG>              Git tag to build
  -p, --platform <PLATFORMS>   Target platforms, comma-separated [default: linux/amd64]
  -f, --flavor <KEY=VALUE>     Flavor override (repeatable)
      --push                   Push the image to the registry after building
```

Example:

```bash
dockermint build \
  -r recipes/cosmos-gaiad.toml \
  -t v21.0.1 \
  --flavor db_backend=pebbledb \
  --flavor build_tags=netgo,muslc,ledger \
  --platform linux/amd64,linux/arm64 \
  --push
```

### `daemon` subcommand

```bash
dockermint daemon [OPTIONS]

Options:
  -i, --poll-interval <SECS>   Override polling interval in seconds
  -m, --max-builds <N>         Max builds per cycle per recipe
  -r, --recipes <STEM>...      Watch only these recipe file stems
      --rpc                    Enable the RPC HTTP server alongside the daemon
      --rpc-bind <ADDR>        RPC server bind address [default: 127.0.0.1:9100]
```

RPC examples:

```bash
# Daemon only (no RPC):
dockermint daemon

# Daemon + RPC on default address:
dockermint daemon --rpc

# Daemon + RPC on custom address:
dockermint daemon --rpc --rpc-bind 0.0.0.0:9100
```

---

## Priority Order (Summary)

For flavor selection, from lowest to highest priority:

1. Recipe `[flavours.default]`
2. `config.toml` `[flavours.<recipe-stem>]`
3. `--flavor` CLI flags
