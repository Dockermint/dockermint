//! OCI-compatible registry backend using Docker CLI.
//!
//! Delegates to `docker login`, `docker push`, and
//! `docker manifest inspect` for registry operations.

use crate::error::{CommandError, RegistryError};
use crate::push::RegistryClient;

/// OCI registry client that delegates to the Docker CLI.
#[derive(Debug)]
pub struct OciRegistry {
    /// Docker daemon socket URI.
    docker_host: String,
    /// Optional registry URL override (e.g. `ghcr.io`).
    /// `None` means Docker Hub.
    registry_url: Option<String>,
}

impl OciRegistry {
    /// Create a new OCI registry client.
    ///
    /// # Arguments
    ///
    /// * `docker_host` - Docker daemon socket URI
    /// * `registry_url` - Override registry URL (`None` for Docker Hub)
    pub fn new(docker_host: String, registry_url: Option<String>) -> Self {
        Self {
            docker_host,
            registry_url,
        }
    }

    /// Build the environment map for Docker CLI calls.
    fn docker_env(&self) -> std::collections::HashMap<String, String> {
        let mut env = std::collections::HashMap::new();
        env.insert("DOCKER_HOST".to_owned(), self.docker_host.clone());
        env
    }
}

/// Validate that registry credentials are either both present or both absent.
///
/// # Arguments
///
/// * `user` - Result from reading `REGISTRY_USER` environment variable
/// * `password` - Result from reading `REGISTRY_PASSWORD` environment variable
///
/// # Returns
///
/// * `Ok(Some((user, password)))` when both credentials are present
/// * `Ok(None)` when both credentials are absent
///
/// # Errors
///
/// Returns [`RegistryError::Auth`] if only one of the two credentials is provided.
fn validate_credentials(
    user: Result<String, std::env::VarError>,
    password: Result<String, std::env::VarError>,
) -> Result<Option<(String, String)>, RegistryError> {
    match (user, password) {
        (Ok(u), Ok(p)) => Ok(Some((u, p))),
        (Err(_), Err(_)) => Ok(None),
        _ => Err(RegistryError::Auth(
            "both REGISTRY_USER and REGISTRY_PASSWORD must be set".to_owned(),
        )),
    }
}

impl RegistryClient for OciRegistry {
    /// Authenticate with the registry using `docker login`.
    ///
    /// Reads `REGISTRY_USER` and `REGISTRY_PASSWORD` from the process
    /// environment.  If both are absent, authentication is skipped
    /// (public push or pre-authenticated daemon).
    async fn authenticate(&self) -> Result<(), RegistryError> {
        let creds = validate_credentials(
            std::env::var("REGISTRY_USER"),
            std::env::var("REGISTRY_PASSWORD"),
        )?;

        let (user, password) = match creds {
            Some(c) => c,
            None => {
                tracing::debug!("no registry credentials, skipping login");
                return Ok(());
            },
        };

        let registry_arg = self.registry_url.as_deref().unwrap_or("");

        tracing::info!(registry = registry_arg, "authenticating with registry");

        let mut args = vec!["login", "--username", &user, "--password-stdin"];
        if !registry_arg.is_empty() {
            args.push(registry_arg);
        }

        // Pipe password via stdin to avoid it appearing in process list
        let env = self.docker_env();
        let output = tokio::process::Command::new("docker")
            .args(&args)
            .envs(&env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| RegistryError::Auth(format!("failed to spawn docker login: {e}")))?;

        use tokio::io::AsyncWriteExt;
        let mut child = output;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(password.as_bytes())
                .await
                .map_err(|e| RegistryError::Auth(format!("stdin write: {e}")))?;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| RegistryError::Auth(format!("wait: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RegistryError::Auth(format!(
                "docker login to '{}' failed (exit {}): {}",
                registry_arg,
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        Ok(())
    }

    /// Push a local image to the registry using `docker push`.
    ///
    /// # Arguments
    ///
    /// * `image` - Full image reference (e.g. `cosmos-gaiad-goleveldb`)
    /// * `tag` - Tag to push (e.g. `v21.0.1-alpine3.23`)
    async fn push_image(&self, image: &str, tag: &str) -> Result<(), RegistryError> {
        let full_ref = format!("{image}:{tag}");
        tracing::info!(image = full_ref, "pushing image");

        let env = self.docker_env();
        crate::commands::execute_with_env("docker", &["push", &full_ref], &env)
            .await
            .map_err(|e| match e {
                CommandError::NonZeroExit { stderr, .. } => RegistryError::Push {
                    image: image.to_owned(),
                    tag: tag.to_owned(),
                    reason: stderr,
                },
                other => RegistryError::Push {
                    image: image.to_owned(),
                    tag: tag.to_owned(),
                    reason: other.to_string(),
                },
            })?;

        Ok(())
    }

    /// Check whether a tag already exists in the registry.
    ///
    /// Uses `docker manifest inspect` which queries the registry
    /// without pulling the image.
    async fn tag_exists(&self, image: &str, tag: &str) -> Result<bool, RegistryError> {
        let full_ref = format!("{image}:{tag}");
        let env = self.docker_env();

        let output = tokio::process::Command::new("docker")
            .args(["manifest", "inspect", &full_ref])
            .envs(&env)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| RegistryError::Query(format!("spawn: {e}")))?;

        if output.status.success() {
            return Ok(true);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();

        let is_infrastructure_error = stderr_lower.contains("unauthorized")
            || stderr_lower.contains("authentication")
            || stderr_lower.contains("denied")
            || stderr_lower.contains("dns")
            || stderr_lower.contains("timeout")
            || stderr_lower.contains("connection refused")
            || stderr_lower.contains("network");

        if is_infrastructure_error {
            return Err(RegistryError::Query(format!(
                "manifest inspect for '{}' failed: {}",
                full_ref,
                stderr.trim()
            )));
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oci_registry_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OciRegistry>();
    }

    #[test]
    fn new_with_custom_registry() {
        let r = OciRegistry::new(
            "unix:///var/run/docker.sock".to_owned(),
            Some("ghcr.io".to_owned()),
        );
        assert_eq!(r.registry_url.as_deref(), Some("ghcr.io"));
    }

    #[test]
    fn new_default_registry() {
        let r = OciRegistry::new("unix:///var/run/docker.sock".to_owned(), None);
        assert!(r.registry_url.is_none());
    }

    #[test]
    fn validate_credentials_both_set() {
        let result = validate_credentials(Ok("alice".to_owned()), Ok("secret".to_owned()));
        let creds = result.expect("should return Ok").expect("should be Some");
        assert_eq!(creds.0, "alice");
        assert_eq!(creds.1, "secret");
    }

    #[test]
    fn validate_credentials_neither_set() {
        let result = validate_credentials(
            Err(std::env::VarError::NotPresent),
            Err(std::env::VarError::NotPresent),
        );
        assert!(result.expect("should be Ok").is_none());
    }

    #[test]
    fn validate_credentials_only_user_errors() {
        let err = validate_credentials(Ok("alice".to_owned()), Err(std::env::VarError::NotPresent))
            .unwrap_err();
        match err {
            RegistryError::Auth(msg) => {
                assert!(
                    msg.contains("both"),
                    "error message should mention 'both', got: {msg}",
                );
                assert!(
                    msg.contains("REGISTRY_USER"),
                    "error message should mention REGISTRY_USER, got: {msg}",
                );
                assert!(
                    msg.contains("REGISTRY_PASSWORD"),
                    "error message should mention REGISTRY_PASSWORD, got: {msg}",
                );
            },
            other => panic!("expected Auth error, got: {other:?}"),
        }
    }

    #[test]
    fn validate_credentials_only_password_errors() {
        let err =
            validate_credentials(Err(std::env::VarError::NotPresent), Ok("secret".to_owned()))
                .unwrap_err();
        match err {
            RegistryError::Auth(msg) => {
                assert!(
                    msg.contains("both"),
                    "error message should mention 'both', got: {msg}",
                );
                assert!(
                    msg.contains("REGISTRY_USER"),
                    "error message should mention REGISTRY_USER, got: {msg}",
                );
                assert!(
                    msg.contains("REGISTRY_PASSWORD"),
                    "error message should mention REGISTRY_PASSWORD, got: {msg}",
                );
            },
            other => panic!("expected Auth error, got: {other:?}"),
        }
    }

    #[test]
    fn docker_env_contains_docker_host_key() {
        let r = OciRegistry::new("tcp://10.0.0.1:2376".to_owned(), Some("ghcr.io".to_owned()));
        let env = r.docker_env();
        assert_eq!(env.len(), 1);
        assert_eq!(
            env.get("DOCKER_HOST").map(String::as_str),
            Some("tcp://10.0.0.1:2376"),
        );
    }

    #[test]
    fn docker_env_uses_exact_host_value() {
        let r = OciRegistry::new("unix:///var/run/docker.sock".to_owned(), None);
        let env = r.docker_env();
        assert_eq!(
            env.get("DOCKER_HOST").map(String::as_str),
            Some("unix:///var/run/docker.sock"),
        );
    }

    #[test]
    fn new_stores_docker_host() {
        let r = OciRegistry::new(
            "tcp://remote:2376".to_owned(),
            Some("registry.example.com".to_owned()),
        );
        assert_eq!(r.docker_host, "tcp://remote:2376");
    }

    #[test]
    fn new_stores_registry_url() {
        let r = OciRegistry::new(
            "unix:///var/run/docker.sock".to_owned(),
            Some("registry.example.com".to_owned()),
        );
        assert_eq!(r.registry_url.as_deref(), Some("registry.example.com"),);
    }

    #[test]
    fn debug_impl_shows_fields() {
        let r = OciRegistry::new(
            "unix:///var/run/docker.sock".to_owned(),
            Some("ghcr.io".to_owned()),
        );
        let debug = format!("{r:?}");
        assert!(
            debug.contains("OciRegistry"),
            "Debug output should contain type name",
        );
        assert!(
            debug.contains("docker_host"),
            "Debug output should contain field name 'docker_host'",
        );
    }
}
