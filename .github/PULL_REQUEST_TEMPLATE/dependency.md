## Description

A clear and concise description of the dependency change.

- Summary of changes
- Reasoning
- Additional context

Closes {LINK TO GH ISSUE}

## Type of change

- [ ] Upgrade (bump an existing dependency)
- [ ] Replace (swap a dependency for another)
- [ ] Remove (drop a dependency)
- [ ] Add (introduce a new dependency)

## Dependency details

- Crate: `{CRATE_NAME}`
- Previous version: `{SEMVER}` (if applicable)
- New version: `{SEMVER}` (if applicable)

## Checks to complete

- [ ] Branch is up-to-date with origin
- [ ] I have performed a self-review of my changes
- [ ] PR title is in a conventional commit style
- [ ] All my commits are GPG signed
- [ ] `CHANGELOG.md` has been updated
- [ ] New dependency is license-compatible
- [ ] Deny passes (`cargo deny check --all-features`)
- [ ] Audit passes (`cargo audit`)
- [ ] All tests pass (`cargo test`)
- [ ] No compiler warnings (`cargo build`)
- [ ] Clippy passes (`cargo clippy -- -D warnings`)

**MUST** build on all **MANDATORY** toolchains:
- [ ] `x86_64-unknown-linux-gnu`
- [ ] `x86_64-unknown-linux-musl`
- [ ] `aarch64-unknown-linux-gnu`
- [ ] `aarch64-unknown-linux-musl`
- [ ] `aarch64-apple-darwin`
