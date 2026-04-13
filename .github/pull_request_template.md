## Description

A clear and concise description of the PR.
Use this section for review hints, explanations or discussion points/todos.

- Summary of changes
- Reasoning
- Additional context

Closes {LINK TO GH ISSUE}

## Type of change

Check options that apply:

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Security (fix a security-related issue)
- [ ] Refactor (modifying code that does not involve changing functionality or fixing bugs)

### Breaking change details (if applicable)

Describe the breaking change and the migration path for existing users.

## Checks to complete

- [ ] Branch is up-to-date with origin
- [ ] I have performed a self-review of my code
- [ ] PR title is in a conventional commit style
- [ ] My code follows the styleguide of the project
- [ ] All my commits are GPG signed
- [ ] `CHANGELOG.md` has been updated
- [ ] New dependencies are license-compatible

### Rust checks

- [ ] All tests pass (`cargo test`)
- [ ] No compiler warnings (`cargo build`)
- [ ] Deny passes (`cargo deny check all`)
- [ ] Audit passes (`cargo audit`)
- [ ] Mutation tests pass (`cargo mutants --check`)
- [ ] Clippy passes (`cargo clippy -- -D warnings`)
- [ ] Code is formatted (`cargo fmt --check`)
- [ ] All public items have doc comments
- [ ] No `unsafe` blocks
- [ ] No `unwrap()` (except in tests)
- [ ] No commented-out code or debug statements
- [ ] No hardcoded credentials

**MUST** build on all **MANDATORY** toolchains:
- [ ] `x86_64-unknown-linux-gnu`
- [ ] `x86_64-unknown-linux-musl`
- [ ] `aarch64-unknown-linux-gnu`
- [ ] `aarch64-unknown-linux-musl`
- [ ] `aarch64-apple-darwin`

### Documentation

- [ ] I have updated relevant documentation
- [ ] New CLI flags or config options are documented
