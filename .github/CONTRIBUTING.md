# Contributing to Dockermint

Thank you for your interest in contributing to Dockermint! This document outlines the guidelines and processes to follow when contributing to this project.

## Table of Contents

- [Project Overview](#project-overview)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Code Standards](#code-standards)
- [Pull Request Process](#pull-request-process)
- [Recipe Contributions](#recipe-contributions)

---

## Project Overview

Dockermint is an open-source CI/CD pipeline that automates and standardizes the creation of Docker images for Cosmos-SDK blockchains and their sidecars in a multi-arch build context. Think of it as the Ansible of the blockchain.

The two key concepts are:

- **Recipes**: TOML files that define the build schema, supported flavors, and default flavors.
- **Flavors**: Options of various types, allowing you to customize or define the expected build.

## Getting Started

### Prerequisites

Ensure you have the following tools installed:

- **Rust toolchain** (latest stable) via `rustup`
- **cargo** and associated tools: `clippy`, `rustfmt`, `cargo-deny`, `cargo-audit`
- **Docker** with BuildKit support
- **GPG** for commit signing

### Mandatory Toolchains

All code **must** compile and work on the following targets:

- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`
- `aarch64-apple-darwin`

Install them via:

```bash
rustup target add x86_64-unknown-linux-gnu x86_64-unknown-linux-musl \
  aarch64-unknown-linux-gnu aarch64-unknown-linux-musl aarch64-apple-darwin
```

### Repository Structure

- `.github/` — GitHub Actions workflows
- `recipes/` — Supported chain recipes (TOML)
- `assets/` — README assets
- `docs/` — Documentation
- `src/` — Rust source code

## Development Workflow

### Branching Strategy

- Follow [Conventional Branch](https://conventional-branch.github.io/) naming (e.g., `feat/add-osmosis-recipe`, `fix/builder-timeout`).
- Always branch from `develop`.
- **Never** push directly to `main`.

### Commit Guidelines

- Follow [Conventional Commits](https://www.conventionalcommits.org/) (e.g., `feat:`, `fix:`, `docs:`, `chore:`).
- All commits **must** be GPG-signed.
- Write clear, descriptive commit messages.
- **Never** commit commented-out code, `println!`/`dbg!` debug statements, or credentials.
- **Never** commit AI-related `.md` files (e.g., `CLAUDE.md`).

### Configuration

- All configuration files **must** be versioned.
- Secrets go in `.env` (which **must** be in `.gitignore`), never in configuration files or source code.
- TOML is the default configuration format.

## Code Standards

### General Principles

- Prioritize **clarity and maintainability** over cleverness.
- Code must be fully optimized: efficient algorithms, proper parallelization, DRY, and no unnecessary code.
- Never use emoji or unicode that emulates emoji in code (e.g., checkmarks, crosses). The only exception is in tests verifying multibyte character handling.

### Rust Style

- Use `snake_case` for functions, variables, and modules; `PascalCase` for types and traits; `SCREAMING_SNAKE_CASE` for constants.
- 4 spaces for indentation, never tabs.
- Line length limit: 100 characters.
- Organize imports in order: standard library, external crates, local modules. Use `rustfmt` for formatting.
- Avoid wildcard imports (`use module::*`) except for preludes and `use super::*` in test modules.

### Error Handling

- **Never** use `.unwrap()` in production code. Use `.expect()` only for invariant violations with a descriptive message.
- Use `Result<T, E>` for all fallible operations.
- Define error types with `thiserror`; use `anyhow` for application-level errors.
- Propagate errors with `?` and provide context via `.context()`.

### Type System and Safety

- Leverage Rust's type system to prevent bugs at compile time.
- Use newtypes to distinguish semantically different values.
- Prefer `Option<T>` over sentinel values.
- **Never** use `unsafe` unless absolutely necessary, and document safety invariants when used.
- No `unwrap()` except in tests.

### Memory and Performance

- Avoid unnecessary allocations; prefer `&str` over `String` when possible.
- Use `Cow<'_, str>` when ownership is conditionally needed.
- Use `Vec::with_capacity()` when the size is known.
- Prefer borrowing (`&T`, `&mut T`) over ownership.

### Concurrency

- Use `tokio` for async runtime and `rayon` for CPU-bound parallelism.
- Prefer `RwLock` or lock-free alternatives over `Mutex` when appropriate.
- Use channels (`mpsc`, `crossbeam`) for message passing.
- Ensure `Send` and `Sync` bounds are applied correctly.

### Security

- **Never** store secrets, API keys, or passwords in code, use `.env` and `dotenvy`/`std::env`.
- **Never** log sensitive information (passwords, tokens, PII).
- Use the `secrecy` crate for sensitive data types.

### Documentation

- All public functions, structs, enums, and methods **must** have doc comments.
- Document parameters, return values, and possible errors.
- Include usage examples in doc comments for complex functions.

Example:

```rust
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
///
/// # Examples
///
/// ```
/// let items = vec![Item { price: 10.0 }, Item { price: 20.0 }];
/// let total = calculate_total(&items, 0.08)?;
/// assert_eq!(total, 32.40);
/// ```
pub fn calculate_total(items: &[Item], tax_rate: f64) -> Result<f64, CalculationError> {
    // ...
}
```

### Testing

- Write unit tests for all new functions and types.
- Mock external dependencies (APIs, databases, file systems).
- Use `#[cfg(test)]` modules and the Arrange-Act-Assert pattern.
- Never commit commented-out tests.

### Dependencies

- Use the latest available version of dependencies.
- Crates **must** come from `crates.io` or `https://github.com/Dockermint`.
- Document all dependencies in `Cargo.toml` with version constraints.
- New dependencies must be license-compatible.

## Pull Request Process

Use the appropriate template when creating your PR:

- **Code** (bug fix, feature, breaking, refactor, security): loaded automatically
- **[Recipe](https://github.com/Dockermint/dockermint/compare/develop...HEAD?quick_pull=1&template=recipe.md)**
- **[Documentation](https://github.com/Dockermint/dockermint/compare/develop...HEAD?quick_pull=1&template=documentation.md)**
- **[Dependency](https://github.com/Dockermint/dockermint/compare/develop...HEAD?quick_pull=1&template=dependency.md)**
- **[Workflow](https://github.com/Dockermint/dockermint/compare/develop...HEAD?quick_pull=1&template=workflow.md)**

### Before Submitting

Run through the full checklist:

```bash
cargo fmt --check
cargo build          # on all mandatory toolchains, no warnings
cargo test
cargo clippy -- -D warnings
cargo deny check --all-features
cargo audit
cargo mutants --check
```

Also verify:

- All public items have doc comments.
- No `unsafe` blocks (unless justified and documented).
- No `unwrap()` in non-test code.
- No commented-out code or debug statements.
- No hardcoded credentials.
- `CHANGELOG.md` has been updated.

### PR Guidelines

- PR title **must** follow Conventional Commits style.
- Provide a clear description: summary of changes, reasoning, and additional context.
- Link the related GitHub issue.
- Perform a self-review before requesting review.
- Ensure the branch is up-to-date with `origin/develop`.

### Types of Changes

When opening a PR, indicate the type:

- **Bug fix** (non-breaking change that fixes an issue)
- **New feature**  (non-breaking change that adds functionality)
- **Breaking change**  (fix or feature that causes existing functionality to break (describe migration path))
- **Recipe change**  (add or edit a recipe file)
- **Workflow**  (add or edit a CI/CD pipeline)
- **Documentation**  (improve or modify documentation)
- **Security** (fix a security-related issue)
- **Dependency** (upgrade, replace or remove a dependency)
- **Refactor** (modifying code that does not involve changing functionality or fixing bugs)

### Build Verification

The PR must build successfully on all mandatory toolchains:

- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`
- `aarch64-apple-darwin`

## Recipe Contributions

If your PR adds or modifies a recipe, additional validation is required.

### Recipe Validation Checklist

- Recipe is linted and validated against the schema.
- Build a chain node (specify related sidecars).
- Build a sidecar (specify related node chain).

### Chain Node Verification

- Recipe builds with default flavors in a rootless context.
- Node successfully syncs with the default-flavors-built image.
- **No** wrong app hash or consensus failure.

### Required Metadata

When submitting a recipe PR, specify:

- **Tested chain version** (semver)
- **Cosmos SDK version** (semver)
- **CometBFT version** (semver)
- **Sync method**: genesis, native snapshot, custom snapshot, or state sync
- **Running environment**: OS, CPU, RAM, disk, bandwidth

### Evidence

Include sync logs, block height reached, sync duration, or screenshots as evidence in a collapsible details block in your PR description.

---

## Preferred Libraries

For consistency across the codebase, use the following libraries:

| Purpose | Library |
|---|---|
| CLI | `clap` |
| Progress bars | `indicatif` |
| JSON | `serde` + `serde_json` |
| TUI | `ratatui` + `crossterm` |
| HTTP server | `axum` + `tower` |
| Error types | `thiserror` |
| App errors | `anyhow` |
| Async runtime | `tokio` |
| CPU parallelism | `rayon` |
| Secrets | `secrecy` |
| Env loading | `dotenvy` |
| Console errors | `tracing` or `log` (never `println!`) |

---

## Questions?

If you have questions or need guidance, feel free to open a GitHub Discussion or reach out to the maintainers. We appreciate every contribution!
