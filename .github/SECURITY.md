# Security Policy

## Supported Versions

| Version | Supported |
| :------ | :-------- |
| latest  | Yes       |

Only the latest release receives security updates. We recommend always running the most recent version.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Please report vulnerabilities privately by emailing **it@dockermint.io** with the following details:

- Description of the vulnerability
- Steps to reproduce
- Affected version(s) and toolchain(s)
- Impact assessment (what an attacker could achieve)
- Any suggested fix, if applicable

You can expect an initial acknowledgement within **48 hours** and a detailed response within **7 business days**. We will coordinate disclosure timing with you and credit reporters unless anonymity is requested.

## Scope

The following are in scope for security reports:

- Dockermint CLI, Daemon, and RPC components
- Recipe parsing and Dockerfile generation (`TemplateEngine` variable interpolation)
- Registry authentication and image push workflows
- BuildKit builder management and cross-compilation pipelines
- Configuration loading and secret handling
- Dependencies shipped in release binaries

The following are **out of scope**:

- Vulnerabilities in upstream Cosmos SDK chains or their node software
- Docker Engine or BuildKit vulnerabilities (report these to Docker)
- Social engineering attacks against maintainers

## Security Practices

Dockermint follows these security principles across the codebase:

- **No secrets in code or configuration files.** All sensitive values are stored in `.env` (excluded via `.gitignore`) and loaded through `dotenvy` or `std::env`.
- **No logging of sensitive data.** Passwords, tokens, API keys, and PII are never written to logs.
- **Sensitive types use the `secrecy` crate** to prevent accidental exposure through `Debug`, `Display`, or serialization.
- **No `unsafe` code** unless strictly necessary, with documented safety invariants.
- **Dependency auditing.** All dependencies are checked with `cargo audit` and `cargo deny` before each release.
- **GPG-signed commits.** All contributions must be signed to ensure authorship integrity.
- **Rootless images.** Recipe validation requires successful running in a rootless container context.

## Disclosure Policy

We follow a coordinated disclosure process:

1. Reporter submits vulnerability privately via email.
2. Maintainers acknowledge, triage, and begin working on a fix.
3. A patched release is prepared and tested across all mandatory toolchains.
4. The fix is released alongside a security advisory on GitHub.
5. Public disclosure occurs after the fix is available, or after 90 days, whichever comes first.

## Contact

For any security-related questions or concerns, reach out to **contact@dockermint.io**.
