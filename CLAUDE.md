# CLAUDE.md

File gives guidance to Claude Code (claude.ai/code) when working with code in this repo.

## Your Core Principles

All code you write MUST be fully optimized.

"Fully optimized" means:

- maximize algorithmic big-O efficiency for memory and runtime
- use parallelization and SIMD where appropriate
- follow proper style conventions for Rust (e.g. maximize code reuse (DRY))
- no extra code beyond what is absolutely necessary to solve problem user provides (no technical debt)

If code not fully optimized before handing off to user, you fined $100. You have permission to do another pass if you believe code not fully optimized.

## Project Overview

Project (named `Dockermint`) is open-source CI/CD pipeline. Automates and standardizes Docker image creation for Cosmos-SDK blockchains and their sidecars in multi-arch build context.

Ansible of blockchain. Two key concepts:
- **Recipes**: `TOML` files defining build schema, supported flavors, default flavors
- **Flavors**: options of various types for customizing or defining expected build

Three modes:
- **CLI**: one-shot builds, build locally or through RPC
- **Daemon**: continuous polling for new Github releases
- **RPC**: daemon that also accepts query from remote CLI

### Unrecoverable Error Strategy

- **CLI**: Dump, log and exit
- **Daemon**: Dump, log, notify, register as failure in DB and continue
- **RPC**: Dump, log and return idle

## Architecture

Philosophy based on:
- Possible to add as many recipes as possible **without modifying Rust code**
- Code as modular as possible; **modules organized into features and replaceable with genericity (by implementing trait)**

### Features

Dockermint modules featured at build time:

| Module  | Default module |
| :------ | :------------- |
| Database | RedB          |
| Notifier | Telegram      |
| VCS      | Github        |
| ssl      | OpenSSL       |
| registry | OCI           |
| builder | BuildKit       |
| metrics | Prometheus |

### Toolchains

**MUST** compile and work with these toolchains:
- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`
- `aarch64-apple-darwin`

### Workspace

In project root:
- `.github`: Github Action
- `recipes/`: Currently supported recipes
- `assets/`: Assets for README
- `docs/`: Documentation

Modules in `./src` folder:
- `cli`: Clap-based CLI with subcommands
- `config`: Config loading
- `logger`: Structured logging with rotation
- `checker`: System requirements verification, singleton instance allowed
- `recipe`: Parsing with selected flavors (CLI args > config.toml > recipe defaults)
- `scrapper`: Github API Client for fetching tags/releases with include/exclude glob filtering
- `builder`: Dockerfile generation via `TemplateEngine` (variable interpolation with `{{UPPERCASE}}` host vars and `{{lowercase}}` build vars), buildx manager for cross-compilation (per-platform builders: `dockermint-amd64`, `dockermint-arm64`) (with default feature), build execution
- `push`: Registry authentication and image pushing
- `saver`: Build state persistence
- `notifier`: Notify build status
- `commands`: Handles command execution
- `metrics`: Expose metrics server

Submodules:

- `builder/go`: Builder for Go recipes

### Configuration Files

- **MUST** version all configuration files
- **MUST** store secrets in `.env` file instead of configuration file
- `TOML` is default format
- Configuration of Dockermint CLI / Daemon / RPC provided via `config.toml`, but CLI args overwrite file
- Wanted flavors for all or specific recipes provided in `config.toml`
- If no wanted flavors provided, use default (in recipe's file)
- If wanted features incompatible, throw error (following Unrecoverable Error Strategy)

## Preferred Tools

- Use `cargo` for project management, building, dependency management.
- Use `indicatif` to track long-running operations with progress bars. Message should be contextually sensitive.
- Use `serde` with `serde_json` for JSON serialization/deserialization.
- Use `ratatui` and `crossterm` for terminal applications/TUIs.
- Use `axum` for web servers or HTTP APIs.
  - Keep request handlers async, returning `Result<Response, AppError>` to centralize error handling.
  - Use layered extractors and shared state structs instead of global mutable data.
  - Add `tower` middleware (timeouts, tracing, compression) for observability and resilience.
  - Offload CPU-bound work to `tokio::task::spawn_blocking` or background services to avoid blocking reactor.
- When reporting errors to console, use `tracing::error!` or `log::error!` instead of `println!`.

## Code Style and Formatting

- **MUST** use meaningful, descriptive variable and function names
- **MUST** follow Rust API Guidelines and idiomatic Rust conventions
- **MUST** use 4 spaces for indentation (never tabs)
- **NEVER** use emoji or unicode that emulates emoji (e.g. ✓, ✗). Only exception: writing tests, testing impact of multibyte characters, documentation
- Use snake_case for functions/variables/modules, PascalCase for types/traits, SCREAMING_SNAKE_CASE for constants
- Limit line length to 100 characters (rustfmt default)
- Assume user is Rust novice

## Documentation

- **MUST** include doc comments for all public functions, structs, enums, methods
- **MUST** document function parameters, return values, errors
- Keep comments up-to-date with code changes
- Include examples in doc comments for complex functions

Example doc comment:

````rust
/// Calculate the total cost of items including tax.
///
/// # Arguments
///
/// * `items` - Slice of item structs with price fields
/// * `tax_rate` - Tax rate as decimal (e.g., 0.08 for 8%)
///
/// # Returns
///
/// Total cost including tax
///
/// # Errors
///
/// Returns `CalculationError::EmptyItems` if items is empty
/// Returns `CalculationError::InvalidTaxRate` if tax_rate is negative
///
/// # Examples
///
/// ```
/// let items = vec![Item { price: 10.0 }, Item { price: 20.0 }];
/// let total = calculate_total(&items, 0.08)?;
/// assert_eq!(total, 32.40);
/// ```
pub fn calculate_total(items: &[Item], tax_rate: f64) -> Result<f64, CalculationError> {
````

## Type System

- **MUST** leverage Rust's type system to prevent bugs at compile time
- **NEVER** use `.unwrap()` in library code; use `.expect()` only for invariant violations with descriptive message
- **MUST** use meaningful custom error types with `thiserror`
- Use newtypes to distinguish semantically different values of same underlying type
- Prefer `Option<T>` over sentinel values

## Error Handling

- **NEVER** use `.unwrap()` in production code paths
- **MUST** use `Result<T, E>` for fallible operations
- **MUST** use `thiserror` for defining error types and `anyhow` for application-level errors
- **MUST** propagate errors with `?` operator where appropriate
- Provide meaningful error messages with context using `.context()` from `anyhow`

## Function Design

- **MUST** keep functions focused on single responsibility
- **MUST** prefer borrowing (`&T`, `&mut T`) over ownership when possible
- Limit function parameters to 5 or fewer; use config struct for more
- Return early to reduce nesting
- Use iterators and combinators over explicit loops where clearer

## Struct and Enum Design

- **MUST** keep types focused on single responsibility
- **MUST** derive common traits: `Debug`, `Clone`, `PartialEq` where appropriate
- Use `#[derive(Default)]` when sensible default exists
- Prefer composition over inheritance-like patterns
- Use builder pattern for complex struct construction
- Make fields private by default; provide accessor methods when needed

## Testing

- **MUST** write unit tests for all new functions and types
- **MUST** mock external dependencies (APIs, databases, file systems)
- **MUST** use built-in `#[test]` attribute and `cargo test`
- Follow Arrange-Act-Assert pattern
- Do not commit commented-out tests
- Use `#[cfg(test)]` modules for test code

## Imports and Dependencies

- **MUST** use latest available version of dependency
- **MUST** avoid wildcard imports (`use module::*`) except for preludes, test modules (`use super::*`), prelude re-exports
- **MUST** document dependencies in `Cargo.toml` with version constraints
- Crates **MUST** come from `crates.io` or `https://github.com/Dockermint`
- Use `cargo` for dependency management
- Organize imports: standard library, external crates, local modules
- Use `rustfmt` to automate import formatting

## Rust Best Practices

- **NEVER** use `unsafe` unless absolutely necessary; document safety invariants when used
- **MUST** call `.clone()` explicitly on non-`Copy` types; avoid hidden clones in closures and iterators
- **MUST** use pattern matching exhaustively; avoid catch-all `_` patterns when possible
- **MUST** use `format!` macro for string formatting
- Use iterators and iterator adapters over manual loops
- Use `enumerate()` instead of manual counter variables
- Prefer `if let` and `while let` for single-pattern matching

## Memory and Performance

- **MUST** avoid unnecessary allocations; prefer `&str` over `String` when possible
- **MUST** use `Cow<'_, str>` when ownership conditionally needed
- Use `Vec::with_capacity()` when size known
- Prefer stack allocation over heap when appropriate
- Use `Arc` and `Rc` judiciously; prefer borrowing

## Concurrency

- **MUST** use `Send` and `Sync` bounds appropriately
- **MUST** prefer `tokio` for async runtime in async applications
- **MUST** use `rayon` for CPU-bound parallelism
- Avoid `Mutex` when `RwLock` or lock-free alternatives appropriate
- Use channels (`mpsc`, `crossbeam`) for message passing

## Security

- **NEVER** store secrets, API keys, or passwords in code. Only store in `.env`.
  - Ensure `.env` declared in `.gitignore`.
- **MUST** use environment variables for sensitive configuration via `dotenvy` or `std::env`
- **NEVER** log sensitive information (passwords, tokens, PII)
- Use `secrecy` crate for sensitive data types

## Version Control

- **MUST** follow `Conventional Commits` and `Conventional Branch`
- **MUST** create branch per feature from `develop`
- **MUST** write clear, descriptive commit messages
- **MUST** sign commit with GPG key
- **NEVER** push on `main`
- **NEVER** commit commented-out code; delete it
- **NEVER** commit debug `println!` statements or `dbg!` macros
- **NEVER** commit credentials or sensitive data
- **NEVER** put you as co-author in commit

## Tools

- **MUST** use `rustfmt` for code formatting
- **MUST** use `clippy` for linting and follow its suggestions
- **MUST** ensure code compiles with no warnings (use `-D warnings` flag in CI, not `#![deny(warnings)]` in source)
- **MUST** use `cargo-deny` for checking code
- Use `cargo` for building, testing, dependency management
- Use `cargo test` for running tests
- Use `cargo doc` for generating documentation

## Before Committing

- [ ] All tests pass (`cargo test`)
- [ ] No compiler warnings (`cargo build`) on all **MANDATORY** toolchains
- [ ] Deny passes (`cargo deny check all`)
- [ ] Audit passes (`cargo audit`)
- [ ] Clippy passes (`cargo clippy -- -D warnings`)
- [ ] Code formatted (`cargo fmt --check`)
- [ ] All public items have doc comments
- [ ] No commented-out code or debug statements
- [ ] No hardcoded credentials

---

**Remember:** Prioritize clarity and maintainability over cleverness.